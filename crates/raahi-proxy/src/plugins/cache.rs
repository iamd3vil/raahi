//! In-memory TTL response cache (proxy-cache plugin).
//!
//! The request phase answers cache hits directly (with an `x-cache: HIT` header)
//! and records a [`CacheIntent`] on misses; the proxy's response phases capture
//! the upstream response and insert it here (see `proxy.rs`). The store is
//! process-global, byte-bounded, and invalidated on configuration reloads.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::{Action, Effects, ReqInput, ShortResp};

/// Soft cap on cached entries: on insert past this, expired entries are pruned;
/// if the map is still full the new entry is skipped (bounded memory).
const MAX_ENTRIES: usize = 10_000;
const MAX_BYTES: usize = 64 * 1024 * 1024;
static GENERATION: AtomicU64 = AtomicU64::new(0);

static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

#[derive(Default)]
struct Cache {
    entries: HashMap<String, CacheEntry>,
    bytes: usize,
}

impl Cache {
    fn insert(&mut self, key: String, entry: CacheEntry, budget: usize) {
        let size = entry.size;
        if size > budget {
            return;
        }
        if let Some(old) = self.entries.remove(&key) {
            self.bytes -= old.size;
        }
        if self.entries.len() >= MAX_ENTRIES || self.bytes.saturating_add(size) > budget {
            let now = Instant::now();
            self.entries.retain(|_, e| {
                if e.expires_at <= now {
                    self.bytes -= e.size;
                    false
                } else {
                    true
                }
            });
        }
        if self.entries.len() < MAX_ENTRIES && self.bytes.saturating_add(size) <= budget {
            self.entries.insert(key, entry);
            self.bytes += size;
        }
    }
}

fn cache() -> &'static Mutex<Cache> {
    CACHE.get_or_init(|| Mutex::new(Cache::default()))
}

struct CacheEntry {
    size: usize,
    expires_at: Instant,
    status: u16,
    stored_at: Instant,
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
    started_at: Instant,
}

impl CacheIntent {
    pub fn accepts_response(&self, status: u16, headers: &http::HeaderMap) -> bool {
        let mut values = Vec::with_capacity(headers.len());
        for (name, value) in headers {
            let Ok(value) = value.to_str() else {
                return false;
            };
            values.push((name.to_string(), value.to_string()));
        }
        status == 200 && response_ttl(self.ttl_secs, &values).is_some()
    }

    /// Insert a completed response into the cache.
    pub fn store(&self, status: u16, headers: Vec<(String, String)>, body: Vec<u8>) {
        if status != 200 || body.len() as u64 > self.max_body_bytes {
            return;
        }
        let Some(ttl) = response_ttl(self.ttl_secs, &headers) else {
            return;
        };
        let now = Instant::now();
        let Some(expires_at) = self.started_at.checked_add(Duration::from_secs(ttl)) else {
            return;
        };
        if expires_at <= now {
            return;
        }
        let size = self.key.len()
            + body.len()
            + headers
                .iter()
                .map(|(k, v)| k.len() + v.len())
                .sum::<usize>();
        if size > MAX_BYTES {
            return;
        }
        cache().lock().unwrap().insert(
            self.key.clone(),
            CacheEntry {
                size,
                expires_at,
                stored_at: self.started_at,
                status,
                headers,
                body,
            },
            MAX_BYTES,
        );
    }
}

/// Conservative shared-cache policy: responses requiring variation or revalidation
/// pass through until those semantics are implemented.
fn response_ttl(configured: u64, headers: &[(String, String)]) -> Option<u64> {
    let mut ttl = configured;
    let mut age = 0;
    for (name, value) in headers {
        if ["set-cookie", "vary", "expires"]
            .iter()
            .any(|h| name.eq_ignore_ascii_case(h))
        {
            return None;
        }
        if name.eq_ignore_ascii_case("age") {
            age = value.parse::<u64>().ok()?;
        }
        if name.eq_ignore_ascii_case("cache-control") {
            for directive in value.split(',') {
                let (key, value) = directive
                    .trim()
                    .split_once('=')
                    .unwrap_or((directive.trim(), ""));
                match key.trim().to_ascii_lowercase().as_str() {
                    "private" | "no-store" | "no-cache" => return None,
                    "max-age" | "s-maxage" => {
                        ttl = ttl.min(value.trim().trim_matches('"').parse().ok()?)
                    }
                    _ => {}
                }
            }
        }
    }
    let ttl = ttl.saturating_sub(age);
    (ttl > 0).then_some(ttl)
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
    if input.method != "GET"
        || effects.consumer.is_some()
        || [
            "authorization",
            "proxy-authorization",
            "cookie",
            "range",
            "if-range",
            "if-none-match",
            "if-modified-since",
            "if-match",
            "if-unmodified-since",
            "cache-control",
            "pragma",
        ]
        .iter()
        .any(|h| input.headers.contains_key(*h))
    {
        return Action::Continue;
    }
    if !cfg
        .methods
        .iter()
        .any(|m| m.eq_ignore_ascii_case(input.method))
    {
        return Action::Continue;
    }
    let mut key = format!(
        "{}:{}:{}:{}:{}:{} {}{}",
        GENERATION.load(Ordering::Relaxed),
        input.config_generation,
        input.service_id,
        input.route_id,
        input.is_tls,
        input.method,
        input.host,
        input.path
    );
    if cfg.cache_key_query {
        if let Some(q) = input.query {
            key.push('?');
            key.push_str(q);
        }
    }

    if let Some(e) = cache().lock().unwrap().entries.get(&key) {
        if e.expires_at > Instant::now() {
            let mut headers = e.headers.clone();
            let initial_age = headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("age"))
                .and_then(|(_, v)| v.parse::<u64>().ok())
                .unwrap_or(0);
            headers.retain(|(k, _)| {
                !k.eq_ignore_ascii_case("age") && !k.eq_ignore_ascii_case("x-cache")
            });
            headers.push((
                "age".into(),
                initial_age
                    .saturating_add(e.stored_at.elapsed().as_secs())
                    .to_string(),
            ));
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
        max_body_bytes: cfg.max_body_bytes.min(MAX_BYTES as u64),
        started_at: Instant::now(),
    });
    Action::Continue
}

/// Drop every cached entry. Returns how many were purged (admin API).
pub fn purge_cache() -> usize {
    let mut map = cache().lock().unwrap();
    GENERATION.fetch_add(1, Ordering::Relaxed);
    let n = map.entries.len();
    map.entries.clear();
    map.bytes = 0;
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderMap;
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn input<'a>(headers: &'a HeaderMap, method: &'a str, query: Option<&'a str>) -> ReqInput<'a> {
        ReqInput {
            method,
            path: "/cache-test",
            query,
            host: "h",
            is_tls: false,
            client_ip: Some("1.2.3.4"),
            headers,
            route_id: 1,
            service_id: 1,
            config_generation: 0,
        }
    }

    #[test]
    fn miss_then_hit_then_purge() {
        let _guard = TEST_LOCK.lock().unwrap();
        purge_cache();
        let cfg = CacheCfg::default();
        let headers = HeaderMap::new();
        let inp = input(&headers, "GET", None);

        let mut fx = Effects::default();
        assert!(matches!(check(&cfg, &inp, &mut fx), Action::Continue));
        let intent = fx.cache_store.expect("miss records an intent");
        intent.store(
            200,
            vec![("content-type".into(), "text/plain".into())],
            b"hello".to_vec(),
        );

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
        let cfg = CacheCfg::default();
        let headers = HeaderMap::new();
        let inp = input(&headers, "POST", None);
        let mut fx = Effects::default();
        assert!(matches!(check(&cfg, &inp, &mut fx), Action::Continue));
        assert!(fx.cache_store.is_none());
    }
    #[test]
    fn personalized_and_conditional_requests_bypass_existing_entries() {
        let _guard = TEST_LOCK.lock().unwrap();
        purge_cache();
        let cfg = CacheCfg::default();
        let headers = HeaderMap::new();
        let mut fx = Effects::default();
        check(&cfg, &input(&headers, "GET", None), &mut fx);
        fx.cache_store
            .unwrap()
            .store(200, vec![], b"public".to_vec());
        for name in [
            "authorization",
            "cookie",
            "range",
            "if-none-match",
            "cache-control",
            "pragma",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(
                http::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                "value".parse().unwrap(),
            );
            let mut fx = Effects::default();
            assert!(matches!(
                check(&cfg, &input(&headers, "GET", None), &mut fx),
                Action::Continue
            ));
            assert!(fx.cache_store.is_none(), "{name}");
        }
        let mut fx = Effects {
            consumer: Some((1, "alice".into())),
            ..Default::default()
        };
        assert!(matches!(
            check(&cfg, &input(&headers, "GET", None), &mut fx),
            Action::Continue
        ));
        assert!(fx.cache_store.is_none());
    }

    #[test]
    fn isolates_routes_services_protocol_and_config_revisions() {
        let _guard = TEST_LOCK.lock().unwrap();
        purge_cache();
        let cfg = CacheCfg::default();
        let headers = HeaderMap::new();
        let mut fx = Effects::default();
        check(&cfg, &input(&headers, "GET", None), &mut fx);
        fx.cache_store.unwrap().store(200, vec![], b"one".to_vec());
        for dimension in 0..4 {
            let mut inp = input(&headers, "GET", None);
            match dimension {
                0 => inp.route_id += 1,
                1 => inp.service_id += 1,
                2 => inp.is_tls = true,
                _ => inp.config_generation += 1,
            }
            assert!(matches!(
                check(&cfg, &inp, &mut Effects::default()),
                Action::Continue
            ));
        }
    }

    #[test]
    fn response_policy_rejects_private_varying_and_stale_responses() {
        for (name, value) in [
            ("Set-Cookie", "session=abc"),
            ("Vary", "Accept-Language"),
            ("Cache-Control", "public, private"),
            ("Cache-Control", "no-cache"),
            ("Cache-Control", "no-store"),
            ("Cache-Control", "max-age=0"),
            ("Cache-Control", "max-age=invalid"),
            ("Expires", "yesterday"),
        ] {
            assert_eq!(
                response_ttl(60, &[(name.into(), value.into())]),
                None,
                "{name}: {value}"
            );
        }
        assert_eq!(
            response_ttl(
                60,
                &[
                    ("cache-control".into(), "max-age=20, s-maxage=10".into()),
                    ("age".into(), "3".into())
                ]
            ),
            Some(7)
        );
        assert_eq!(response_ttl(60, &[("age".into(), "60".into())]), None);
    }

    #[test]
    fn byte_budget_accounts_for_replacement_and_expiry() {
        let entry = |size, expired| CacheEntry {
            size,
            expires_at: Instant::now() + Duration::from_secs(if expired { 0 } else { 60 }),
            status: 200,
            stored_at: Instant::now(),
            headers: vec![],
            body: vec![],
        };
        let mut cache = Cache::default();
        cache.insert("a".into(), entry(6, false), 10);
        cache.insert("b".into(), entry(6, false), 10);
        assert_eq!(cache.bytes, 6);
        assert!(!cache.entries.contains_key("b"));
        cache.insert("a".into(), entry(3, true), 10);
        assert_eq!(cache.bytes, 3);
        cache.insert("c".into(), entry(8, false), 10);
        assert_eq!(cache.bytes, 8);
        assert!(!cache.entries.contains_key("a"));
    }

    #[test]
    fn purge_does_not_allow_inflight_response_to_repopulate_live_namespace() {
        let _guard = TEST_LOCK.lock().unwrap();
        purge_cache();
        let headers = HeaderMap::new();
        let cfg = CacheCfg::default();
        let mut fx = Effects::default();
        check(&cfg, &input(&headers, "GET", None), &mut fx);
        purge_cache();
        fx.cache_store.unwrap().store(200, vec![], b"old".to_vec());
        assert!(matches!(
            check(&cfg, &input(&headers, "GET", None), &mut Effects::default()),
            Action::Continue
        ));
    }
}
