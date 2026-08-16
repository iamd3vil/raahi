//! The compiled, immutable configuration snapshot the data plane reads on every
//! request. Built by the store from SQLite rows, swapped atomically on change.

use std::collections::HashMap;

use crate::matching::{host_matches, path_matches};
use crate::model::*;
use crate::Id;

/// An immutable view of all routing configuration. Wrapped in an `ArcSwap` by the
/// data plane; rebuilt wholesale on any config change.
#[derive(Debug, Clone, Default)]
pub struct ProxyConfig {
    /// Monotonic version, bumped on each rebuild (used to detect changes).
    pub version: u64,
    /// All enabled routes (matching scans these).
    pub routes: Vec<Route>,
    pub services: HashMap<Id, Service>,
    /// service_id -> its enabled targets.
    pub targets: HashMap<Id, Vec<Target>>,
    /// All enabled plugins (global + scoped).
    pub plugins: Vec<Plugin>,
    pub consumers: HashMap<Id, Consumer>,
    /// key-auth: API key -> consumer id.
    pub key_index: HashMap<String, Id>,
    /// basic-auth: username -> (consumer id, bcrypt hash).
    pub basic_index: HashMap<String, (Id, String)>,
    /// jwt: key claim value (e.g. `iss`) -> verification material.
    pub jwt_index: HashMap<String, JwtCred>,
    /// WASM plugin modules by name (bytes shared with compiled instances).
    pub wasm_modules: HashMap<String, std::sync::Arc<Vec<u8>>>,
    pub settings: Settings,
}

/// JWT verification material for one consumer credential.
#[derive(Debug, Clone)]
pub struct JwtCred {
    pub consumer_id: Id,
    /// One of HS256 / HS384 / HS512 / RS256.
    pub algorithm: String,
    /// HMAC secret (HS*) or RSA public key PEM (RS256).
    pub secret: String,
}

/// The result of matching a request against the routing table.
#[derive(Debug, Clone, Copy)]
pub struct RouteMatch<'a> {
    pub route: &'a Route,
    pub service: &'a Service,
    /// The path prefix that matched (for `strip_path`). Empty if the route had no
    /// path constraints.
    pub matched_prefix: &'a str,
}

impl ProxyConfig {
    /// Find the best route for `(host, path, method)`.
    ///
    /// Selection: among enabled routes whose host + method + some path prefix match,
    /// pick the highest `priority`; ties are broken by the longest matched path
    /// prefix, then by lowest route id for determinism.
    pub fn match_route(&self, host: &str, path: &str, method: &str) -> Option<RouteMatch<'_>> {
        let method = method.to_ascii_uppercase();
        let mut best: Option<(i32, usize, Id, RouteMatch)> = None;

        for route in &self.routes {
            if !route.enabled {
                continue;
            }
            if !route.hosts.is_empty() && !route.hosts.iter().any(|h| host_matches(h, host)) {
                continue;
            }
            if !route.methods.is_empty()
                && !route.methods.iter().any(|m| m.eq_ignore_ascii_case(&method))
            {
                continue;
            }

            // Longest matching path prefix for this route (empty paths => match all).
            let matched_prefix: Option<&str> = if route.paths.is_empty() {
                Some("")
            } else {
                route
                    .paths
                    .iter()
                    .filter(|p| path_matches(p, path))
                    .map(|p| p.as_str())
                    .max_by_key(|p| p.trim_end_matches('/').len())
            };

            let Some(prefix) = matched_prefix else {
                continue;
            };
            let Some(service) = self.services.get(&route.service_id) else {
                continue; // dangling service reference
            };

            let prefix_len = prefix.trim_end_matches('/').len();
            let candidate = (
                route.priority,
                prefix_len,
                route.id,
                RouteMatch { route, service, matched_prefix: prefix },
            );

            best = Some(match best {
                None => candidate,
                Some(cur) => {
                    // Higher priority wins; then longer prefix; then lower id.
                    let cur_key = (cur.0, cur.1, std::cmp::Reverse(cur.2));
                    let cand_key = (candidate.0, candidate.1, std::cmp::Reverse(candidate.2));
                    if cand_key > cur_key { candidate } else { cur }
                }
            });
        }

        best.map(|(_, _, _, m)| m)
    }

    /// Plugins that apply to a matched route, ordered for execution.
    ///
    /// Resolution: a route-scoped plugin overrides a service-scoped one of the same
    /// type, which overrides a global one — except `wasm` plugins, which all apply
    /// (users can stack several custom plugins on one route). Within the resolved
    /// set, sort by `ordering` then `id`.
    pub fn plugins_for(&self, route_id: Id, service_id: Id) -> Vec<&Plugin> {
        use std::collections::HashMap as Map;
        // Best plugin per type, by scope precedence (route > service > global).
        let mut chosen: Map<PluginType, &Plugin> = Map::new();
        let mut wasm: Vec<&Plugin> = Vec::new();
        let rank = |p: &Plugin| match p.scope {
            PluginScope::Route => 3,
            PluginScope::Service => 2,
            PluginScope::Global => 1,
        };
        for p in &self.plugins {
            if !p.enabled {
                continue;
            }
            let applies = match p.scope {
                PluginScope::Global => true,
                PluginScope::Service => p.service_id == Some(service_id),
                PluginScope::Route => p.route_id == Some(route_id),
            };
            if !applies {
                continue;
            }
            if p.plugin_type == PluginType::Wasm {
                wasm.push(p);
                continue;
            }
            chosen
                .entry(p.plugin_type)
                .and_modify(|cur| {
                    if rank(p) > rank(cur) {
                        *cur = p;
                    }
                })
                .or_insert(p);
        }
        let mut out: Vec<&Plugin> = chosen.into_values().collect();
        out.extend(wasm);
        out.sort_by_key(|p| (p.ordering, p.id));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn svc(id: Id) -> Service {
        Service {
            id,
            name: format!("svc{id}"),
            protocol: Protocol::Http,
            connect_timeout_ms: 1000,
            read_timeout_ms: 1000,
            write_timeout_ms: 1000,
            retries: 0,
            lb_algorithm: LbAlgorithm::RoundRobin,
            tls_sni: None,
            health_path: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn route(id: Id, service_id: Id, priority: i32, hosts: &[&str], paths: &[&str]) -> Route {
        Route {
            id,
            name: format!("route{id}"),
            service_id,
            priority,
            hosts: hosts.iter().map(|s| s.to_string()).collect(),
            paths: paths.iter().map(|s| s.to_string()).collect(),
            methods: vec![],
            strip_path: false,
            preserve_host: false,
            enabled: true,
        }
    }

    fn cfg(routes: Vec<Route>, services: Vec<Service>) -> ProxyConfig {
        ProxyConfig {
            services: services.into_iter().map(|s| (s.id, s)).collect(),
            routes,
            ..Default::default()
        }
    }

    #[test]
    fn longest_prefix_wins_within_priority() {
        let c = cfg(
            vec![
                route(1, 1, 0, &[], &["/api"]),
                route(2, 1, 0, &[], &["/api/v2"]),
            ],
            vec![svc(1)],
        );
        let m = c.match_route("h", "/api/v2/users", "GET").unwrap();
        assert_eq!(m.route.id, 2);
        assert_eq!(m.matched_prefix, "/api/v2");
    }

    #[test]
    fn higher_priority_beats_longer_prefix() {
        let c = cfg(
            vec![
                route(1, 1, 10, &[], &["/api"]),
                route(2, 1, 0, &[], &["/api/v2"]),
            ],
            vec![svc(1)],
        );
        let m = c.match_route("h", "/api/v2/users", "GET").unwrap();
        assert_eq!(m.route.id, 1);
    }

    #[test]
    fn host_constraint() {
        let c = cfg(
            vec![route(1, 1, 0, &["api.example.com"], &[])],
            vec![svc(1)],
        );
        assert!(c.match_route("api.example.com", "/x", "GET").is_some());
        assert!(c.match_route("other.com", "/x", "GET").is_none());
    }

    #[test]
    fn method_constraint() {
        let mut r = route(1, 1, 0, &[], &["/"]);
        r.methods = vec!["POST".into()];
        let c = cfg(vec![r], vec![svc(1)]);
        assert!(c.match_route("h", "/x", "POST").is_some());
        assert!(c.match_route("h", "/x", "get").is_none());
    }

    #[test]
    fn no_match_returns_none() {
        let c = cfg(vec![route(1, 1, 0, &[], &["/api"])], vec![svc(1)]);
        assert!(c.match_route("h", "/nope", "GET").is_none());
    }
}
