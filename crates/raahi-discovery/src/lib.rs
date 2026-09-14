//! Target discovery providers and the background reconciliation loop.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{TimeDelta, Utc};
use hickory_resolver::TokioResolver;
use hickory_resolver::config::{NameServerConfig, ResolverConfig};
use hickory_resolver::name_server::TokioConnectionProvider;
use hickory_resolver::proto::xfer::Protocol;
use pingora::server::ShutdownWatch;
use pingora::services::background::BackgroundService;
use raahi_core::{DiscoveredEndpoint, DiscoverySource};
use raahi_proxy::ConfigHandle;
use raahi_store::Store;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{Mutex, Notify, RwLock};

const MIN_INTERVAL_MS: u64 = 1_000;
const DEFAULT_INTERVAL_MS: u64 = 30_000;
const DEFAULT_TIMEOUT_MS: u64 = 5_000;
const MAX_ENDPOINTS: usize = 10_000;

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("invalid config: {0}")]
    Invalid(String),
    #[error("provider failed: {0}")]
    Provider(String),
    #[error("store failed: {0}")]
    Store(#[from] raahi_store::StoreError),
}

#[derive(Debug, Clone)]
pub struct ProviderResult {
    pub endpoints: Vec<DiscoveredEndpoint>,
    pub revision: Option<String>,
    pub valid_for: Option<Duration>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderDescriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub supplies_port: bool,
    pub supplies_weight: bool,
    pub supplies_priority: bool,
    pub config_schema: Value,
    pub defaults: Value,
}

#[async_trait]
pub trait DiscoveryProvider: Send + Sync {
    fn descriptor(&self) -> ProviderDescriptor;
    fn validate(&self, config: &Value) -> Result<(), DiscoveryError>;
    async fn discover(&self, config: &Value) -> Result<ProviderResult, DiscoveryError>;
}

#[derive(Clone)]
pub struct ConfigPublisher {
    store: Store,
    config: ConfigHandle,
    lock: Arc<Mutex<()>>,
}

impl ConfigPublisher {
    pub fn new(store: Store, config: ConfigHandle) -> Self {
        Self {
            store,
            config,
            lock: Arc::new(Mutex::new(())),
        }
    }

    pub async fn publish(&self) -> Result<(), DiscoveryError> {
        let _guard = self.lock.lock().await;
        let snapshot = self.store.build_snapshot().await?;
        self.config.store(snapshot);
        Ok(())
    }
}

#[derive(Clone)]
pub struct DiscoveryHandle {
    notify: Arc<Notify>,
    requested: Arc<RwLock<Option<i64>>>,
}

impl DiscoveryHandle {
    fn new() -> Self {
        Self {
            notify: Arc::new(Notify::new()),
            requested: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn trigger(&self, source_id: Option<i64>) {
        *self.requested.write().await = source_id;
        self.notify.notify_one();
    }
}

#[derive(Clone)]
pub struct DiscoveryRegistry {
    providers: Arc<BTreeMap<String, Arc<dyn DiscoveryProvider>>>,
}

impl Default for DiscoveryRegistry {
    fn default() -> Self {
        let mut providers: BTreeMap<String, Arc<dyn DiscoveryProvider>> = BTreeMap::new();
        for provider in [
            Arc::new(DnsProvider) as Arc<dyn DiscoveryProvider>,
            Arc::new(DnsSrvProvider),
            Arc::new(HttpProvider),
        ] {
            providers.insert(provider.descriptor().id.to_string(), provider);
        }
        Self {
            providers: Arc::new(providers),
        }
    }
}

impl DiscoveryRegistry {
    pub fn descriptors(&self) -> Vec<ProviderDescriptor> {
        self.providers.values().map(|p| p.descriptor()).collect()
    }

    pub fn validate(&self, provider: &str, config: &Value) -> Result<(), DiscoveryError> {
        self.providers
            .get(provider)
            .ok_or_else(|| {
                DiscoveryError::Invalid(format!("unknown discovery provider '{provider}'"))
            })?
            .validate(config)
    }

    async fn discover(
        &self,
        provider: &str,
        config: &Value,
    ) -> Result<ProviderResult, DiscoveryError> {
        self.providers
            .get(provider)
            .ok_or_else(|| {
                DiscoveryError::Invalid(format!("unknown discovery provider '{provider}'"))
            })?
            .discover(config)
            .await
    }
}

pub struct DiscoveryService {
    pub db_url: String,
    pub publisher: ConfigPublisher,
    pub registry: DiscoveryRegistry,
    pub handle: DiscoveryHandle,
}

impl DiscoveryService {
    pub fn new(
        db_url: String,
        store: Store,
        config: ConfigHandle,
    ) -> (Self, DiscoveryHandle, ConfigPublisher, DiscoveryRegistry) {
        let publisher = ConfigPublisher::new(store, config);
        let handle = DiscoveryHandle::new();
        let registry = DiscoveryRegistry::default();
        (
            Self {
                db_url,
                publisher: publisher.clone(),
                registry: registry.clone(),
                handle: handle.clone(),
            },
            handle,
            publisher,
            registry,
        )
    }

    async fn refresh_source(&self, store: &Store, source: DiscoverySource) {
        let interval =
            config_u64(&source.config, "interval_ms", DEFAULT_INTERVAL_MS).max(MIN_INTERVAL_MS);
        let timeout_ms = config_u64(&source.config, "timeout_ms", DEFAULT_TIMEOUT_MS).max(100);
        let next = next_refresh_at(source.id, interval);
        if let Err(error) = store.mark_discovery_attempt(source.id, next).await {
            tracing::error!(
                source_id = source.id,
                "discovery status update failed: {error}"
            );
            return;
        }
        let discovered = tokio::time::timeout(
            Duration::from_millis(timeout_ms),
            self.registry.discover(&source.provider, &source.config),
        )
        .await;
        match discovered {
            Ok(Ok(mut result)) => {
                if result.endpoints.len() > MAX_ENDPOINTS {
                    let message = format!("provider returned more than {MAX_ENDPOINTS} endpoints");
                    let changed = store
                        .record_discovery_failure(&source, &message, next)
                        .await
                        .unwrap_or(false);
                    if changed {
                        let _ = self.publisher.publish().await;
                    }
                    return;
                }
                result.endpoints.sort_by(|a, b| a.key.cmp(&b.key));
                let revision = result
                    .revision
                    .unwrap_or_else(|| endpoint_revision(&result.endpoints));
                let interval = result
                    .valid_for
                    .map(|ttl| ttl.as_millis().try_into().unwrap_or(u64::MAX))
                    .unwrap_or(interval)
                    .clamp(MIN_INTERVAL_MS, interval.max(MIN_INTERVAL_MS));
                let next = next_refresh_at(source.id, interval);
                match store
                    .reconcile_discovery(&source, &result.endpoints, &revision, next)
                    .await
                {
                    Ok(change) => {
                        if change.changed
                            && let Err(error) = self.publisher.publish().await
                        {
                            tracing::error!(
                                source_id = source.id,
                                "discovery publish failed: {error}"
                            );
                        }
                        tracing::info!(
                            source_id = source.id,
                            service_id = source.service_id,
                            added = change.added,
                            updated = change.updated,
                            draining = change.draining,
                            removed = change.removed,
                            "discovery refresh succeeded"
                        );
                    }
                    Err(error) => tracing::error!(
                        source_id = source.id,
                        "discovery reconcile failed: {error}"
                    ),
                }
            }
            Ok(Err(error)) => {
                let changed = store
                    .record_discovery_failure(&source, &error.to_string(), next)
                    .await
                    .unwrap_or(false);
                if changed {
                    let _ = self.publisher.publish().await;
                }
                tracing::warn!(source_id = source.id, "discovery refresh failed: {error}");
            }
            Err(_) => {
                let message = format!("refresh timed out after {timeout_ms}ms");
                let changed = store
                    .record_discovery_failure(&source, &message, next)
                    .await
                    .unwrap_or(false);
                if changed {
                    let _ = self.publisher.publish().await;
                }
                tracing::warn!(source_id = source.id, "{message}");
            }
        }
    }

    async fn refresh_due(&self, store: &Store, requested: Option<i64>) {
        if store.expire_draining_targets().await.unwrap_or(false) {
            let _ = self.publisher.publish().await;
        }
        let Ok(sources) = store.list_discovery_sources().await else {
            return;
        };
        let now = Utc::now();
        for source in sources.into_iter().filter(|source| source.enabled) {
            let forced = requested.is_none_or(|id| id == source.id);
            let due = store
                .get_discovery_status(source.id)
                .await
                .ok()
                .flatten()
                .is_none_or(|status| status.next_refresh_at.is_none_or(|next| next <= now));
            if forced || due {
                self.refresh_source(store, source).await;
            }
        }
    }
}

#[async_trait]
impl BackgroundService for DiscoveryService {
    async fn start(&self, mut shutdown: ShutdownWatch) {
        let store = match Store::connect(&self.db_url).await {
            Ok(store) => store,
            Err(error) => {
                tracing::error!("discovery could not open store: {error}");
                return;
            }
        };
        self.refresh_due(&store, None).await;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                _ = shutdown.changed() => break,
                _ = tick.tick() => self.refresh_due(&store, Some(i64::MIN)).await,
                _ = self.handle.notify.notified() => {
                    let requested = self.handle.requested.write().await.take();
                    self.refresh_due(&store, requested).await;
                }
            }
        }
    }
}

fn next_refresh_at(source_id: i64, interval_ms: u64) -> chrono::DateTime<Utc> {
    Utc::now() + TimeDelta::milliseconds(jittered_interval_ms(source_id, interval_ms) as i64)
}

/// Refresh between 90% and 100% of the interval. The source id and current
/// interval bucket spread sources out without introducing another RNG dependency.
fn jittered_interval_ms(source_id: i64, interval_ms: u64) -> u64 {
    let interval_ms = interval_ms.max(MIN_INTERVAL_MS);
    let window = (interval_ms / 10).max(1);
    let bucket = Utc::now().timestamp_millis().unsigned_abs() / interval_ms;
    interval_ms.saturating_sub(fxhash(&format!("{source_id}:{bucket}")) % (window + 1))
}

fn config_u64(config: &Value, name: &str, default: u64) -> u64 {
    config.get(name).and_then(Value::as_u64).unwrap_or(default)
}

fn config_u16(config: &Value, name: &str, default: u16) -> Result<u16, DiscoveryError> {
    let value = config
        .get(name)
        .and_then(Value::as_u64)
        .unwrap_or(u64::from(default));
    u16::try_from(value).map_err(|_| DiscoveryError::Invalid(format!("{name} is out of range")))
}

fn config_u32(config: &Value, name: &str, default: u32) -> Result<u32, DiscoveryError> {
    let value = config
        .get(name)
        .and_then(Value::as_u64)
        .unwrap_or(u64::from(default));
    u32::try_from(value).map_err(|_| DiscoveryError::Invalid(format!("{name} is out of range")))
}

fn endpoint_revision(endpoints: &[DiscoveredEndpoint]) -> String {
    let digest = serde_json::to_string(
        &endpoints
            .iter()
            .map(|e| (&e.key, &e.host, e.port, e.weight, e.priority, &e.metadata))
            .collect::<Vec<_>>(),
    )
    .unwrap_or_default();
    fxhash(&digest).to_string()
}

fn fxhash(value: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn system_resolver() -> TokioResolver {
    TokioResolver::builder_tokio()
        .expect("system resolver")
        .build()
}

fn resolver_for(config: &Value) -> Result<TokioResolver, DiscoveryError> {
    let Some(raw) = config
        .get("resolver")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
    else {
        return Ok(system_resolver());
    };
    let address: SocketAddr = if raw.contains(':') {
        raw.parse()
    } else {
        format!("{raw}:53").parse()
    }
    .map_err(|_| {
        DiscoveryError::Invalid("resolver must be an IP address with optional port".into())
    })?;
    let mut cfg = ResolverConfig::new();
    cfg.add_name_server(NameServerConfig::new(address, Protocol::Udp));
    cfg.add_name_server(NameServerConfig::new(address, Protocol::Tcp));
    Ok(TokioResolver::builder_with_config(cfg, TokioConnectionProvider::default()).build())
}

struct DnsProvider;

#[async_trait]
impl DiscoveryProvider for DnsProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: "dns",
            name: "DNS A and AAAA",
            description: "Resolve a hostname and turn every returned address into a target.",
            supplies_port: false,
            supplies_weight: false,
            supplies_priority: false,
            config_schema: json!({"required":["hostname","port"]}),
            defaults: json!({"interval_ms":30000,"timeout_ms":5000,"record_types":["a","aaaa"],"weight":100,"priority":0}),
        }
    }

    fn validate(&self, config: &Value) -> Result<(), DiscoveryError> {
        let hostname = config
            .get("hostname")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if hostname.is_empty() {
            return Err(DiscoveryError::Invalid("hostname is required".into()));
        }
        if config_u16(config, "port", 0)? == 0 {
            return Err(DiscoveryError::Invalid(
                "port must be between 1 and 65535".into(),
            ));
        }
        resolver_for(config)?;
        Ok(())
    }

    async fn discover(&self, config: &Value) -> Result<ProviderResult, DiscoveryError> {
        self.validate(config)?;
        let hostname = config["hostname"].as_str().unwrap();
        let port = config_u16(config, "port", 0)?;
        let weight = config_u32(config, "weight", 100)?;
        let priority = config_u16(config, "priority", 0)?;
        let want = config
            .get("record_types")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>())
            .unwrap_or_else(|| vec!["a", "aaaa"]);
        let lookup = resolver_for(config)?
            .lookup_ip(hostname)
            .await
            .map_err(|error| DiscoveryError::Provider(error.to_string()))?;
        let mut endpoints = Vec::new();
        for address in lookup.iter() {
            if (address.is_ipv4() && !want.contains(&"a"))
                || (address.is_ipv6() && !want.contains(&"aaaa"))
            {
                continue;
            }
            endpoints.push(DiscoveredEndpoint {
                key: format!("dns:{address}:{port}"),
                host: address.to_string(),
                port,
                weight,
                priority,
                metadata: BTreeMap::from([("hostname".into(), hostname.into())]),
            });
        }
        Ok(ProviderResult {
            endpoints,
            revision: None,
            valid_for: Some(
                lookup
                    .valid_until()
                    .saturating_duration_since(std::time::Instant::now()),
            ),
        })
    }
}

struct DnsSrvProvider;

#[async_trait]
impl DiscoveryProvider for DnsSrvProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: "dns-srv",
            name: "DNS SRV",
            description: "Discover hosts, ports, weights, and priority from SRV records.",
            supplies_port: true,
            supplies_weight: true,
            supplies_priority: true,
            config_schema: json!({"required":["service_name"]}),
            defaults: json!({"interval_ms":30000,"timeout_ms":5000,"use_record_weight":true,"use_record_priority":true,"weight":100,"priority":0}),
        }
    }

    fn validate(&self, config: &Value) -> Result<(), DiscoveryError> {
        let name = config
            .get("service_name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if name.is_empty() {
            return Err(DiscoveryError::Invalid("service_name is required".into()));
        }
        resolver_for(config)?;
        Ok(())
    }

    async fn discover(&self, config: &Value) -> Result<ProviderResult, DiscoveryError> {
        self.validate(config)?;
        let name = config["service_name"].as_str().unwrap();
        let use_weight = config
            .get("use_record_weight")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let use_priority = config
            .get("use_record_priority")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let fallback_weight = config_u32(config, "weight", 100)?;
        let fallback_priority = config_u16(config, "priority", 0)?;
        let lookup = resolver_for(config)?
            .srv_lookup(name)
            .await
            .map_err(|error| DiscoveryError::Provider(error.to_string()))?;
        let mut endpoints = Vec::new();
        for record in lookup.iter() {
            let host = record.target().to_utf8().trim_end_matches('.').to_string();
            if host.is_empty() || host == "." || record.port() == 0 {
                continue;
            }
            let mut metadata = BTreeMap::new();
            metadata.insert("service_name".into(), name.into());
            metadata.insert("srv_weight".into(), record.weight().to_string());
            metadata.insert("srv_priority".into(), record.priority().to_string());
            endpoints.push(DiscoveredEndpoint {
                key: format!("srv:{host}:{}", record.port()),
                host,
                port: record.port(),
                weight: if use_weight {
                    u32::from(record.weight()).max(1)
                } else {
                    fallback_weight
                },
                priority: if use_priority {
                    record.priority()
                } else {
                    fallback_priority
                },
                metadata,
            });
        }
        Ok(ProviderResult {
            endpoints,
            revision: None,
            valid_for: Some(
                lookup
                    .as_lookup()
                    .valid_until()
                    .saturating_duration_since(std::time::Instant::now()),
            ),
        })
    }
}

struct HttpProvider;

#[async_trait]
impl DiscoveryProvider for HttpProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: "http",
            name: "HTTP registry",
            description: "Poll an HTTP JSON endpoint for an authoritative endpoint list.",
            supplies_port: true,
            supplies_weight: true,
            supplies_priority: true,
            config_schema: json!({"required":["url"]}),
            defaults: json!({"interval_ms":30000,"timeout_ms":5000,"tls_verify":true,"headers":{},"weight":100,"priority":0}),
        }
    }

    fn validate(&self, config: &Value) -> Result<(), DiscoveryError> {
        let url = config.get("url").and_then(Value::as_str).unwrap_or("");
        let parsed = reqwest::Url::parse(url)
            .map_err(|error| DiscoveryError::Invalid(format!("invalid url: {error}")))?;
        if !matches!(parsed.scheme(), "http" | "https")
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err(DiscoveryError::Invalid(
                "url must be http(s) without embedded credentials".into(),
            ));
        }
        Ok(())
    }

    async fn discover(&self, config: &Value) -> Result<ProviderResult, DiscoveryError> {
        self.validate(config)?;
        let tls_verify = config
            .get("tls_verify")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .danger_accept_invalid_certs(!tls_verify)
            .build()
            .map_err(|error| DiscoveryError::Provider(error.to_string()))?;
        let mut request = client.get(config["url"].as_str().unwrap());
        if let Some(headers) = config.get("headers").and_then(Value::as_object) {
            for (name, value) in headers {
                if let Some(value) = value.as_str() {
                    request = request.header(name, value);
                }
            }
        }
        let response = request
            .send()
            .await
            .map_err(|error| DiscoveryError::Provider(error.to_string()))?;
        if !response.status().is_success() {
            return Err(DiscoveryError::Provider(format!(
                "HTTP {}",
                response.status()
            )));
        }
        let revision = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let document: Value = response
            .json()
            .await
            .map_err(|error| DiscoveryError::Provider(format!("invalid JSON: {error}")))?;
        let mut value = &document;
        if let Some(path) = config
            .get("endpoints_path")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        {
            for component in path.split('.') {
                value = value.get(component).ok_or_else(|| {
                    DiscoveryError::Provider(format!("endpoints_path '{path}' was not found"))
                })?;
            }
        }
        let values = value
            .as_array()
            .or_else(|| value.get("endpoints").and_then(Value::as_array))
            .ok_or_else(|| {
                DiscoveryError::Provider("response must contain an endpoint array".into())
            })?;
        let fallback_port = config
            .get("port")
            .and_then(Value::as_u64)
            .and_then(|v| u16::try_from(v).ok());
        let fallback_weight = config_u32(config, "weight", 100)?;
        let fallback_priority = config_u16(config, "priority", 0)?;
        let mut endpoints = Vec::new();
        for item in values {
            if item.get("enabled").and_then(Value::as_bool) == Some(false) {
                continue;
            }
            let host = item
                .get("host")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            let port = item
                .get("port")
                .and_then(Value::as_u64)
                .and_then(|v| u16::try_from(v).ok())
                .or(fallback_port)
                .unwrap_or(0);
            if host.is_empty() || port == 0 {
                return Err(DiscoveryError::Provider(
                    "endpoint host and port are required".into(),
                ));
            }
            let weight = item
                .get("weight")
                .and_then(Value::as_u64)
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(fallback_weight);
            let priority = item
                .get("priority")
                .and_then(Value::as_u64)
                .and_then(|v| u16::try_from(v).ok())
                .unwrap_or(fallback_priority);
            let key = item
                .get("key")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("http:{host}:{port}"));
            let metadata = item
                .get("metadata")
                .and_then(Value::as_object)
                .map(|map| {
                    map.iter()
                        .filter_map(|(key, value)| {
                            value.as_str().map(|v| (key.clone(), v.to_string()))
                        })
                        .collect()
                })
                .unwrap_or_default();
            endpoints.push(DiscoveredEndpoint {
                key,
                host: host.into(),
                port,
                weight,
                priority,
                metadata,
            });
        }
        Ok(ProviderResult {
            endpoints,
            revision,
            valid_for: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_jitter_stays_inside_the_last_ten_percent() {
        for interval in [1_000, 30_000, 300_000] {
            let jittered = jittered_interval_ms(42, interval);
            assert!(jittered <= interval);
            assert!(jittered >= interval - interval / 10);
        }
    }

    #[test]
    fn provider_validation_rejects_missing_fields() {
        let registry = DiscoveryRegistry::default();
        assert!(registry.validate("dns", &json!({})).is_err());
        assert!(registry.validate("dns-srv", &json!({})).is_err());
        assert!(
            registry
                .validate("http", &json!({"url":"file:///tmp/a"}))
                .is_err()
        );
    }
}
