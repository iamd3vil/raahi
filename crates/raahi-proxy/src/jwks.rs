//! JWKS fetching for the jwt plugin's `jwks_url` mode (validating tokens from an
//! identity provider — Auth0, Keycloak, Google — without per-consumer credentials).
//!
//! A background service refreshes each configured JWKS document periodically into a
//! process-global cache; the (synchronous) request path only ever reads the cache.
//! RS256 keys only, matched by `kid` (falling back to a lone key).

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use pingora::server::ShutdownWatch;
use pingora::services::background::BackgroundService;
use raahi_core::PluginType;

use crate::runtime::ConfigHandle;

const REFRESH_INTERVAL: Duration = Duration::from_secs(30);

/// RSA public key components (base64url), as jsonwebtoken wants them.
#[derive(Clone, Debug)]
pub struct JwkKey {
    pub n: String,
    pub e: String,
}

type Cache = HashMap<String, HashMap<String, JwkKey>>; // url -> kid -> key

fn cache() -> &'static ArcSwap<Cache> {
    static CACHE: OnceLock<ArcSwap<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| ArcSwap::from_pointee(HashMap::new()))
}

/// Look up a verification key for `url`. `kid = None` matches a lone key.
pub fn lookup(url: &str, kid: Option<&str>) -> Option<JwkKey> {
    let cache = cache().load();
    let keys = cache.get(url)?;
    match kid {
        Some(kid) => keys.get(kid).cloned(),
        None if keys.len() == 1 => keys.values().next().cloned(),
        None => None,
    }
}

fn parse_jwks(doc: &serde_json::Value) -> HashMap<String, JwkKey> {
    let mut out = HashMap::new();
    let Some(keys) = doc["keys"].as_array() else {
        return out;
    };
    for (i, k) in keys.iter().enumerate() {
        if k["kty"].as_str() != Some("RSA") {
            continue;
        }
        let (Some(n), Some(e)) = (k["n"].as_str(), k["e"].as_str()) else {
            continue;
        };
        let kid = k["kid"].as_str().map(String::from).unwrap_or_else(|| format!("_{i}"));
        out.insert(kid, JwkKey { n: n.to_string(), e: e.to_string() });
    }
    out
}

/// Background refresher: fetches every `jwks_url` referenced by an enabled jwt
/// plugin in the live config.
pub struct JwksService {
    pub config: ConfigHandle,
}

impl JwksService {
    fn configured_urls(&self) -> Vec<String> {
        let rc = self.config.load();
        rc.data
            .plugins
            .iter()
            .filter(|p| p.enabled && p.plugin_type == PluginType::Jwt)
            .filter_map(|p| p.config["jwks_url"].as_str())
            .filter(|u| !u.is_empty())
            .map(String::from)
            .collect()
    }

    async fn refresh(&self, client: &reqwest::Client) {
        let urls = self.configured_urls();
        if urls.is_empty() {
            if !cache().load().is_empty() {
                cache().store(std::sync::Arc::new(HashMap::new()));
            }
            return;
        }

        let mut next: Cache = HashMap::new();
        for url in urls {
            match client
                .get(&url)
                .timeout(Duration::from_secs(10))
                .send()
                .await
                .and_then(|r| r.error_for_status())
            {
                Ok(resp) => match resp.json::<serde_json::Value>().await {
                    Ok(doc) => {
                        let keys = parse_jwks(&doc);
                        if keys.is_empty() {
                            tracing::warn!("jwks: {url} returned no usable RSA keys");
                        }
                        next.insert(url, keys);
                    }
                    Err(e) => {
                        tracing::warn!("jwks: {url} returned invalid JSON: {e}");
                        keep_stale(&mut next, &url);
                    }
                },
                Err(e) => {
                    tracing::warn!("jwks: fetch {url} failed: {e}");
                    keep_stale(&mut next, &url);
                }
            }
        }
        cache().store(std::sync::Arc::new(next));
    }
}

/// On fetch failure, keep serving the previously cached keys rather than dropping
/// them (an IdP blip must not lock every token out).
fn keep_stale(next: &mut Cache, url: &str) {
    if let Some(old) = cache().load().get(url) {
        next.insert(url.to_string(), old.clone());
    }
}

#[async_trait]
impl BackgroundService for JwksService {
    async fn start(&self, mut shutdown: ShutdownWatch) {
        let client = reqwest::Client::new();
        let mut tick = tokio::time::interval(REFRESH_INTERVAL);
        // First tick fires immediately, so keys are available shortly after boot.
        loop {
            tokio::select! {
                _ = shutdown.changed() => break,
                _ = tick.tick() => self.refresh(&client).await,
            }
        }
    }
}
