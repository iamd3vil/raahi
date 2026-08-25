//! Runtime view of the config: per-service load balancers with health flags, built
//! from an immutable [`ProxyConfig`] and swapped atomically via [`ConfigHandle`].

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use arc_swap::ArcSwap;
use raahi_core::{Id, LbAlgorithm, Plugin, ProxyConfig, Service};

use crate::plugins::{PluginSet, type_priority};

/// A selectable backend with a shared, live-updated health flag.
pub struct BackendRt {
    pub target_id: Id,
    pub host: String,
    pub port: u16,
    pub weight: u32,
    pub healthy: Arc<AtomicBool>,
    /// Every address `host` resolved to at snapshot build (empty on resolution
    /// failure). The proxy, the stream splicer, and the health checker all share
    /// this one view — resolving independently let them disagree (e.g. `localhost`
    /// as ::1 for the proxy but 127.0.0.1 for a passing health probe).
    pub addrs: Vec<SocketAddr>,
    /// Index into `addrs` of the address currently used for connects; the health
    /// checker moves it to a working address when the current one fails probes.
    pub active: Arc<AtomicUsize>,
}

impl BackendRt {
    /// The address connects should use right now.
    pub fn active_addr(&self) -> Option<SocketAddr> {
        let i = self.active.load(Ordering::Relaxed);
        self.addrs.get(i).or_else(|| self.addrs.first()).copied()
    }

    /// Display/connect string: the active resolved address, falling back to the
    /// configured `host:port` when resolution failed.
    pub fn addr_string(&self) -> String {
        self.active_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|| format!("{}:{}", self.host, self.port))
    }
}

/// Runtime state for one service: its settings, backends, and a round-robin cursor.
pub struct ServiceRuntime {
    pub service: Service,
    pub backends: Vec<BackendRt>,
    cursor: AtomicUsize,
}

impl ServiceRuntime {
    /// Pick a backend honoring the service's LB algorithm. Prefers healthy backends;
    /// if all are unhealthy, falls back to the full set rather than failing outright.
    pub fn select(&self, client_key: &[u8]) -> Option<&BackendRt> {
        let healthy: Vec<&BackendRt> = self
            .backends
            .iter()
            .filter(|b| b.healthy.load(Ordering::Relaxed))
            .collect();
        let pool: Vec<&BackendRt> = if healthy.is_empty() {
            self.backends.iter().collect()
        } else {
            healthy
        };
        if pool.is_empty() {
            return None;
        }

        match self.service.lb_algorithm {
            LbAlgorithm::RoundRobin => {
                let i = self.cursor.fetch_add(1, Ordering::Relaxed);
                Some(pool[i % pool.len()])
            }
            LbAlgorithm::Random => {
                // Counter-derived pseudo-random index (no rng dependency).
                let i = self.cursor.fetch_add(1, Ordering::Relaxed);
                let scrambled = i.wrapping_mul(2_654_435_761);
                Some(pool[scrambled % pool.len()])
            }
            LbAlgorithm::Weighted => {
                let total: u64 = pool.iter().map(|b| b.weight.max(1) as u64).sum();
                let pos = (self.cursor.fetch_add(1, Ordering::Relaxed) as u64) % total;
                let mut acc = 0u64;
                for b in &pool {
                    acc += b.weight.max(1) as u64;
                    if pos < acc {
                        return Some(b);
                    }
                }
                pool.last().copied()
            }
            LbAlgorithm::Consistent => {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                client_key.hash(&mut h);
                Some(pool[(h.finish() as usize) % pool.len()])
            }
        }
    }
}

/// The full runtime config snapshot. Swapped wholesale on config change.
pub struct RuntimeConfig {
    pub data: Arc<ProxyConfig>,
    pub services: HashMap<Id, Arc<ServiceRuntime>>,
    /// Stateful plugin instances keyed by plugin id (rate-limit counters, etc.).
    pub plugins: PluginSet,
    /// Plugin execution chains precompiled per (route, effective service) — scope
    /// resolution and ordering (type priority, then `ordering`, then id) are done
    /// once per snapshot instead of on every request. Split routes get one chain
    /// per split service.
    chains: HashMap<(Id, Id), Arc<Vec<Plugin>>>,
}

impl RuntimeConfig {
    /// Build runtime state from a config snapshot. When `prev` is given, health flags
    /// for targets that still exist are carried over so reloads don't reset health.
    pub fn build(data: ProxyConfig, prev: Option<&RuntimeConfig>) -> RuntimeConfig {
        let mut services = HashMap::new();
        for (&sid, service) in &data.services {
            let targets = data.targets.get(&sid).cloned().unwrap_or_default();
            let backends = targets
                .into_iter()
                .map(|t| {
                    // Carry health (and the elected address, if the target is
                    // unchanged) across reloads so config edits don't reset state.
                    let carried = prev
                        .and_then(|p| p.services.get(&sid))
                        .and_then(|sr| sr.backends.iter().find(|b| b.target_id == t.id));
                    let healthy = carried
                        .map(|b| b.healthy.clone())
                        .unwrap_or_else(|| Arc::new(AtomicBool::new(true)));

                    let addrs: Vec<SocketAddr> = match (t.host.as_str(), t.port).to_socket_addrs() {
                        Ok(it) => it.collect(),
                        Err(e) => {
                            tracing::warn!(
                                "target {}:{} does not resolve: {e}; it will fail health checks",
                                t.host,
                                t.port
                            );
                            Vec::new()
                        }
                    };
                    let active = carried
                        .filter(|b| b.host == t.host && b.port == t.port)
                        .map(|b| b.active.clone())
                        .unwrap_or_default();
                    // Clamp a carried index that no longer fits the new list.
                    if active.load(Ordering::Relaxed) >= addrs.len() {
                        active.store(0, Ordering::Relaxed);
                    }

                    BackendRt {
                        target_id: t.id,
                        host: t.host,
                        port: t.port,
                        weight: t.weight,
                        healthy,
                        addrs,
                        active,
                    }
                })
                .collect();
            services.insert(
                sid,
                Arc::new(ServiceRuntime {
                    service: service.clone(),
                    backends,
                    cursor: AtomicUsize::new(0),
                }),
            );
        }

        let plugins = PluginSet::build(&data, prev.map(|p| &p.plugins));

        let mut chains: HashMap<(Id, Id), Arc<Vec<Plugin>>> = HashMap::new();
        for route in &data.routes {
            let mut sids: Vec<Id> = vec![route.service_id];
            sids.extend(
                route
                    .splits
                    .iter()
                    .map(|sp| sp.service_id)
                    .filter(|sid| data.services.contains_key(sid)),
            );
            for sid in sids {
                chains
                    .entry((route.id, sid))
                    .or_insert_with(|| Arc::new(ordered_plugins(&data, route.id, sid)));
            }
        }

        RuntimeConfig {
            data: Arc::new(data),
            services,
            plugins,
            chains,
        }
    }

    /// The precompiled, execution-ordered plugin chain for a matched (route,
    /// service) pair. Falls back to computing it (never to skipping plugins —
    /// that would bypass auth) if the pair is somehow absent from the map.
    pub fn plugins_ordered(&self, route_id: Id, service_id: Id) -> Arc<Vec<Plugin>> {
        match self.chains.get(&(route_id, service_id)) {
            Some(c) => c.clone(),
            None => Arc::new(ordered_plugins(&self.data, route_id, service_id)),
        }
    }
}

/// Scope-resolve and execution-order the plugins for one (route, service) pair.
fn ordered_plugins(data: &ProxyConfig, route_id: Id, service_id: Id) -> Vec<Plugin> {
    let mut v: Vec<Plugin> = data
        .plugins_for(route_id, service_id)
        .into_iter()
        .cloned()
        .collect();
    v.sort_by_key(|p| (type_priority(p.plugin_type), p.ordering, p.id));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend(addrs: Vec<SocketAddr>, active: usize) -> BackendRt {
        BackendRt {
            target_id: 1,
            host: "example.internal".into(),
            port: 9000,
            weight: 1,
            healthy: Arc::new(AtomicBool::new(true)),
            addrs,
            active: Arc::new(AtomicUsize::new(active)),
        }
    }

    #[test]
    fn active_addr_uses_elected_index_and_clamps_out_of_range() {
        let b = backend(
            vec![
                "127.0.0.1:1".parse().unwrap(),
                "127.0.0.1:2".parse().unwrap(),
            ],
            1,
        );
        assert_eq!(b.addr_string(), "127.0.0.1:2");
        b.active.store(7, Ordering::Relaxed); // stale index: fall back to first
        assert_eq!(b.addr_string(), "127.0.0.1:1");
    }

    #[test]
    fn addr_string_falls_back_to_host_port_when_unresolved() {
        assert_eq!(backend(vec![], 0).addr_string(), "example.internal:9000");
    }
}

/// Cheaply-cloneable handle to the live config. The proxy reads via [`load`], the
/// admin side swaps via [`store`].
#[derive(Clone)]
pub struct ConfigHandle {
    inner: Arc<ArcSwap<RuntimeConfig>>,
}

impl ConfigHandle {
    pub fn new(initial: RuntimeConfig) -> Self {
        ConfigHandle {
            inner: Arc::new(ArcSwap::from_pointee(initial)),
        }
    }

    pub fn load(&self) -> arc_swap::Guard<Arc<RuntimeConfig>> {
        self.inner.load()
    }

    /// Rebuild the runtime from a new data snapshot and swap it in atomically.
    pub fn store(&self, data: ProxyConfig) {
        let prev = self.inner.load();
        let next = RuntimeConfig::build(data, Some(&prev));
        self.inner.store(Arc::new(next));
    }
}
