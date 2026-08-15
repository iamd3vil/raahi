//! Runtime view of the config: per-service load balancers with health flags, built
//! from an immutable [`ProxyConfig`] and swapped atomically via [`ConfigHandle`].

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwap;
use raahi_core::{Id, LbAlgorithm, ProxyConfig, Service};

use crate::plugins::PluginSet;

/// A selectable backend with a shared, live-updated health flag.
pub struct BackendRt {
    pub target_id: Id,
    pub host: String,
    pub port: u16,
    pub weight: u32,
    pub healthy: Arc<AtomicBool>,
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
                    let healthy = prev
                        .and_then(|p| p.services.get(&sid))
                        .and_then(|sr| sr.backends.iter().find(|b| b.target_id == t.id))
                        .map(|b| b.healthy.clone())
                        .unwrap_or_else(|| Arc::new(AtomicBool::new(true)));
                    BackendRt {
                        target_id: t.id,
                        host: t.host,
                        port: t.port,
                        weight: t.weight,
                        healthy,
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

        let plugins = PluginSet::build(&data.plugins, prev.map(|p| &p.plugins));

        RuntimeConfig {
            data: Arc::new(data),
            services,
            plugins,
        }
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
