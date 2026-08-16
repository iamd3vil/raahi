//! In-memory TTL response cache (proxy-cache plugin).
//!
//! The request phase answers cache hits directly (with an `x-cache: HIT` header)
//! and records a [`CacheIntent`] on misses; the proxy's response phases capture
//! the upstream response and insert it here (see `proxy.rs`). The store is
//! process-global so entries survive config reloads.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::{Action, Effects, ReqInput, ShortResp};

/// Soft cap on cached entries: on insert past this, expired entries are pruned;
/// if the map is still full the new entry is skipped (bounded memory).
const MAX_ENTRIES: usize = 10_000;

static CACHE: OnceLock<Mutex<HashMap<String, CacheEntry>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<String, CacheEntry>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

struct CacheEntry {
    expires_at: Instant,
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// A miss recorded during the request phase: the proxy stores the upstream
/// response under `key` once it has streamed through (200s only, size-capped).
#[derive(Debug, Clone)]
pub struct CacheIntent {
    pub key: String,
    pub ttl_secs: u64,
    pub max_body_bytes: u64,
}

impl CacheIntent {
    /// Insert a completed response into the cache.
    pub fn store(&self, status: u16, headers: Vec<(String, String)>, body: Vec<u8>) {
        let now = Instant::now();
        let mut map = cache().lock().unwrap();
        if map.len() >= MAX_ENTRIES {
            map.retain(|_, e| e.expires_at > now);
            if map.len() >= MAX_ENTRIES {
                return;
            }
        }
        map.insert(
            self.key.clone(),
            CacheEntry {
                expires_at: now + Duration::from_secs(self.ttl_secs.max(1)),
                status,
                headers,
                body,
            },
        );
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CacheCfg {
    pub ttl_secs: u64,
    pub max_body_bytes: u64,
    /// Methods eligible for caching (case-insensitive).
    pub methods: Vec<String>,
    /// Include the query string in the cache key.
    pub cache_key_query: bool,
}

impl Default for CacheCfg {
    fn default() -> Self {
        CacheCfg {
            ttl_secs: 60,
            max_body_bytes: 1_048_576,
            methods: vec!["GET".to_string()],
            cache_key_query: true,
        }
    }
}

/// Request phase: answer unexpired hits, record a store intent on misses.
pub fn check(cfg: &CacheCfg, input: &ReqInput, effects: &mut Effects) -> Action {
    if !cfg.methods.iter().any(|m| m.eq_ignore_ascii_case(input.method)) {
        return Action::Continue;
    }
    let mut key = format!("{} {}{}", input.method, input.host, input.path);
    if cfg.cache_key_query {
        if let Some(q) = input.query {
            key.push('?');
            key.push_str(q);
        }
    }

    if let Some(e) = cache().lock().unwrap().get(&key) {
        if e.expires_at > Instant::now() {
            let mut headers = e.headers.clone();
            headers.push(("x-cache".into(), "HIT".into()));
            return Action::Respond(ShortResp {
                status: e.status,
                headers,
                body: e.body.clone(),
            });
        }
    }

    effects.cache_store = Some(CacheIntent {
        key,
        ttl_secs: cfg.ttl_secs,
        max_body_bytes: cfg.max_body_bytes,
    });
    Action::Continue
}

/// Drop every cached entry. Returns how many were purged (admin API).
pub fn purge_cache() -> usize {
    let mut map = cache().lock().unwrap();
    let n = map.len();
    map.clear();
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderMap;

    fn input<'a>(headers: &'a HeaderMap, method: &'a str, query: Option<&'a str>) -> ReqInput<'a> {
        ReqInput {
            method,
            path: "/cache-test",
            query,
            host: "h",
            client_ip: Some("1.2.3.4"),
            headers,
            route_id: 1,
        }
    }

    #[test]
    fn miss_then_hit_then_purge() {
        purge_cache();
        let cfg = CacheCfg::default();
        let headers = HeaderMap::new();
        let inp = input(&headers, "GET", None);

        let mut fx = Effects::default();
        assert!(matches!(check(&cfg, &inp, &mut fx), Action::Continue));
        let intent = fx.cache_store.expect("miss records an intent");
        intent.store(200, vec![("content-type".into(), "text/plain".into())], b"hello".to_vec());

        let mut fx = Effects::default();
        match check(&cfg, &inp, &mut fx) {
            Action::Respond(r) => {
                assert_eq!(r.status, 200);
                assert_eq!(r.body, b"hello");
                assert!(r.headers.iter().any(|(k, v)| k == "x-cache" && v == "HIT"));
                assert!(r.headers.iter().any(|(k, _)| k == "content-type"));
            }
            _ => panic!("expected a cache hit"),
        }

        assert!(purge_cache() >= 1);
        let mut fx = Effects::default();
        assert!(matches!(check(&cfg, &inp, &mut fx), Action::Continue));
    }

    #[test]
    fn non_configured_method_is_skipped() {
        purge_cache();
        let cfg = CacheCfg::default();
        let headers = HeaderMap::new();
        let inp = input(&headers, "POST", None);
        let mut fx = Effects::default();
        assert!(matches!(check(&cfg, &inp, &mut fx), Action::Continue));
        assert!(fx.cache_store.is_none());
    }
}
