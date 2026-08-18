//! The compiled, immutable configuration snapshot the data plane reads on every
//! request. Built by the store from SQLite rows, swapped atomically on change.

use std::collections::HashMap;

use crate::Id;
use crate::matching::{compile_path_regex, host_matches, is_regex_path, path_matches};
use crate::model::*;

/// An immutable view of all routing configuration. Wrapped in an `ArcSwap` by the
/// data plane; rebuilt wholesale on any config change.
#[derive(Debug, Clone, Default)]
pub struct ProxyConfig {
    /// Monotonic version, bumped on each rebuild (used to detect changes).
    pub version: u64,
    /// All enabled routes (matching scans these).
    pub routes: Vec<Route>,
    /// All enabled stream (L4 TCP) routes, looked up live by listen address.
    pub stream_routes: Vec<StreamRoute>,
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
    /// Compiled `~`-prefixed route path patterns, keyed by the raw pattern string.
    /// Patterns that fail to compile are absent and never match.
    pub path_regexes: HashMap<String, regex::Regex>,
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
#[derive(Debug, Clone)]
pub struct RouteMatch<'a> {
    pub route: &'a Route,
    pub service: &'a Service,
    /// The leading portion of the request path that matched (for `strip_path`).
    /// Empty if the route had no path constraints.
    pub matched_prefix: String,
}

impl ProxyConfig {
    /// Compile every `~`-prefixed route path pattern into [`Self::path_regexes`].
    /// Invalid patterns (rejected by API validation, but reachable via import or
    /// direct DB edits) are skipped and never match.
    pub fn compile_path_regexes(&mut self) {
        self.path_regexes.clear();
        for route in &self.routes {
            for p in route.paths.iter().filter(|p| is_regex_path(p)) {
                if self.path_regexes.contains_key(p.as_str()) {
                    continue;
                }
                match compile_path_regex(p) {
                    Ok(re) => {
                        self.path_regexes.insert(p.clone(), re);
                    }
                    Err(e) => {
                        // No tracing dep in core; the API layer validates upfront.
                        eprintln!(
                            "raahi: route '{}' has invalid regex path {p:?}: {e}",
                            route.name
                        );
                    }
                }
            }
        }
    }

    /// The number of leading bytes of `path` matched by one route path pattern,
    /// or `None` if the pattern doesn't match.
    fn path_match_len(&self, pattern: &str, path: &str) -> Option<usize> {
        if is_regex_path(pattern) {
            self.path_regexes
                .get(pattern)
                .and_then(|re| re.find(path))
                .map(|m| m.end())
        } else if path_matches(pattern, path) {
            Some(pattern.trim_end_matches('/').len())
        } else {
            None
        }
    }
    /// Find the best route for `(host, path, method)` plus header conditions.
    ///
    /// `header` looks up a request header by name (implementations must treat the
    /// name case-insensitively) — a closure so this crate stays free of http deps.
    ///
    /// Selection: among enabled routes whose host + method + headers + some path
    /// prefix match, pick the highest `priority`; ties are broken by the longest
    /// matched path prefix, then by lowest route id for determinism.
    pub fn match_route(
        &self,
        host: &str,
        path: &str,
        method: &str,
        header: &dyn Fn(&str) -> Option<String>,
    ) -> Option<RouteMatch<'_>> {
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
                && !route
                    .methods
                    .iter()
                    .any(|m| m.eq_ignore_ascii_case(&method))
            {
                continue;
            }
            // Every header condition must match ("*" = present with any value).
            if !route
                .headers
                .iter()
                .all(|(name, want)| header(name).is_some_and(|got| want == "*" || got == *want))
            {
                continue;
            }

            // Longest match among this route's path patterns — literal prefixes and
            // `~` regexes alike (empty paths => match all, zero length).
            let matched_len: Option<usize> = if route.paths.is_empty() {
                Some(0)
            } else {
                route
                    .paths
                    .iter()
                    .filter_map(|p| self.path_match_len(p, path))
                    .max()
            };

            let Some(prefix_len) = matched_len else {
                continue;
            };
            let Some(service) = self.services.get(&route.service_id) else {
                continue; // dangling service reference
            };

            let candidate = (
                route.priority,
                prefix_len,
                route.id,
                RouteMatch {
                    route,
                    service,
                    matched_prefix: path[..prefix_len].to_string(),
                },
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
            headers: Default::default(),
            splits: vec![],
            strip_path: false,
            preserve_host: false,
            enabled: true,
        }
    }

    /// Header lookup over a static list, case-insensitive like a real header map.
    fn hdrs(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name: &str| {
            pairs
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.to_string())
        }
    }

    fn no_hdrs(_: &str) -> Option<String> {
        None
    }

    fn cfg(routes: Vec<Route>, services: Vec<Service>) -> ProxyConfig {
        let mut c = ProxyConfig {
            services: services.into_iter().map(|s| (s.id, s)).collect(),
            routes,
            ..Default::default()
        };
        c.compile_path_regexes();
        c
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
        let m = c
            .match_route("h", "/api/v2/users", "GET", &no_hdrs)
            .unwrap();
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
        let m = c
            .match_route("h", "/api/v2/users", "GET", &no_hdrs)
            .unwrap();
        assert_eq!(m.route.id, 1);
    }

    #[test]
    fn host_constraint() {
        let c = cfg(
            vec![route(1, 1, 0, &["api.example.com"], &[])],
            vec![svc(1)],
        );
        assert!(
            c.match_route("api.example.com", "/x", "GET", &no_hdrs)
                .is_some()
        );
        assert!(c.match_route("other.com", "/x", "GET", &no_hdrs).is_none());
    }

    #[test]
    fn method_constraint() {
        let mut r = route(1, 1, 0, &[], &["/"]);
        r.methods = vec!["POST".into()];
        let c = cfg(vec![r], vec![svc(1)]);
        assert!(c.match_route("h", "/x", "POST", &no_hdrs).is_some());
        assert!(c.match_route("h", "/x", "get", &no_hdrs).is_none());
    }

    #[test]
    fn regex_path_matches_and_reports_matched_portion() {
        let c = cfg(vec![route(1, 1, 0, &[], &[r"~/users/\d+"])], vec![svc(1)]);
        let m = c
            .match_route("h", "/users/42/orders", "GET", &no_hdrs)
            .unwrap();
        assert_eq!(m.route.id, 1);
        // strip_path strips exactly the regex-matched portion.
        assert_eq!(m.matched_prefix, "/users/42");
        assert!(c.match_route("h", "/users/abc", "GET", &no_hdrs).is_none());
        assert!(c.match_route("h", "/other/42", "GET", &no_hdrs).is_none());
    }

    #[test]
    fn regex_is_anchored_at_path_start() {
        let c = cfg(vec![route(1, 1, 0, &[], &[r"~/v\d+"])], vec![svc(1)]);
        assert!(c.match_route("h", "/v2/x", "GET", &no_hdrs).is_some());
        // A mid-path occurrence must not match.
        assert!(c.match_route("h", "/api/v2/x", "GET", &no_hdrs).is_none());
    }

    #[test]
    fn regex_dollar_requires_full_path() {
        let c = cfg(vec![route(1, 1, 0, &[], &[r"~/ping$"])], vec![svc(1)]);
        assert!(c.match_route("h", "/ping", "GET", &no_hdrs).is_some());
        assert!(c.match_route("h", "/ping/deep", "GET", &no_hdrs).is_none());
    }

    #[test]
    fn longer_regex_match_beats_shorter_prefix() {
        let c = cfg(
            vec![
                route(1, 1, 0, &[], &["/api"]),
                route(2, 1, 0, &[], &[r"~/api/v\d+"]),
            ],
            vec![svc(1)],
        );
        let m = c
            .match_route("h", "/api/v2/users", "GET", &no_hdrs)
            .unwrap();
        assert_eq!(m.route.id, 2);
        assert_eq!(m.matched_prefix, "/api/v2");
    }

    #[test]
    fn invalid_regex_never_matches() {
        let c = cfg(vec![route(1, 1, 0, &[], &["~/users/("])], vec![svc(1)]);
        assert!(c.match_route("h", "/users/(", "GET", &no_hdrs).is_none());
    }

    #[test]
    fn no_match_returns_none() {
        let c = cfg(vec![route(1, 1, 0, &[], &["/api"])], vec![svc(1)]);
        assert!(c.match_route("h", "/nope", "GET", &no_hdrs).is_none());
    }

    #[test]
    fn header_constraint_exact_value() {
        let mut r = route(1, 1, 0, &[], &["/"]);
        r.headers = [("x-version".to_string(), "beta".to_string())].into();
        let c = cfg(vec![r], vec![svc(1)]);
        assert!(
            c.match_route("h", "/x", "GET", &hdrs(&[("x-version", "beta")]))
                .is_some()
        );
        // Wrong value or absent header: no match.
        assert!(
            c.match_route("h", "/x", "GET", &hdrs(&[("x-version", "stable")]))
                .is_none()
        );
        assert!(c.match_route("h", "/x", "GET", &no_hdrs).is_none());
    }

    #[test]
    fn header_constraint_presence_wildcard() {
        let mut r = route(1, 1, 0, &[], &["/"]);
        r.headers = [("x-debug".to_string(), "*".to_string())].into();
        let c = cfg(vec![r], vec![svc(1)]);
        assert!(
            c.match_route("h", "/x", "GET", &hdrs(&[("x-debug", "anything")]))
                .is_some()
        );
        assert!(c.match_route("h", "/x", "GET", &no_hdrs).is_none());
    }

    #[test]
    fn header_name_is_case_insensitive() {
        let mut r = route(1, 1, 0, &[], &["/"]);
        r.headers = [("X-Version".to_string(), "beta".to_string())].into();
        let c = cfg(vec![r], vec![svc(1)]);
        // The lookup (like a real header map) resolves names case-insensitively.
        assert!(
            c.match_route("h", "/x", "GET", &hdrs(&[("x-version", "beta")]))
                .is_some()
        );
        // Values stay exact.
        assert!(
            c.match_route("h", "/x", "GET", &hdrs(&[("x-version", "BETA")]))
                .is_none()
        );
    }
}
