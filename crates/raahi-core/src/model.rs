//! Domain entity types. These double as the wire (JSON) representation used by the
//! admin API and the in-memory representation used by the proxy. The store maps
//! SQLite rows to and from these.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::Id;

/// Upstream wire protocol for a service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Http,
    Https,
}

impl Protocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Protocol::Http => "http",
            Protocol::Https => "https",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "http" => Some(Protocol::Http),
            "https" => Some(Protocol::Https),
            _ => None,
        }
    }
    pub fn is_tls(self) -> bool {
        matches!(self, Protocol::Https)
    }
    pub fn default_port(self) -> u16 {
        match self {
            Protocol::Http => 80,
            Protocol::Https => 443,
        }
    }
}

/// Load-balancing algorithm across a service's targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LbAlgorithm {
    #[default]
    RoundRobin,
    Random,
    /// Consistent hashing (by client IP) — sticky-ish routing.
    Consistent,
    /// Smooth weighted round-robin honoring per-target weights.
    Weighted,
}

impl LbAlgorithm {
    pub fn as_str(self) -> &'static str {
        match self {
            LbAlgorithm::RoundRobin => "round_robin",
            LbAlgorithm::Random => "random",
            LbAlgorithm::Consistent => "consistent",
            LbAlgorithm::Weighted => "weighted",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "round_robin" => Some(LbAlgorithm::RoundRobin),
            "random" => Some(LbAlgorithm::Random),
            "consistent" => Some(LbAlgorithm::Consistent),
            "weighted" => Some(LbAlgorithm::Weighted),
            _ => None,
        }
    }
}

/// A logical upstream: a named group of [`Target`]s sharing connection settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Service {
    pub id: Id,
    pub name: String,
    pub protocol: Protocol,
    pub connect_timeout_ms: u64,
    pub read_timeout_ms: u64,
    pub write_timeout_ms: u64,
    pub retries: u32,
    pub lb_algorithm: LbAlgorithm,
    /// HTTP authority sent to upstreams when the route does not preserve the
    /// incoming Host header. Falls back to the target host when unset.
    #[serde(default)]
    pub upstream_authority: Option<String>,
    /// SNI to present to the upstream when `protocol = https`. Falls back to
    /// `upstream_authority`, then the target host.
    pub tls_sni: Option<String>,
    /// Active health check: GET this path on each target (healthy = 2xx/3xx).
    /// `None` = plain TCP-connect check. HTTPS services use verified HTTPS probes.
    #[serde(default)]
    pub health_path: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Lifecycle state for a discovery-managed target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TargetState {
    #[default]
    Active,
    Draining,
    Stale,
}

impl TargetState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Draining => "draining",
            Self::Stale => "stale",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "draining" => Some(Self::Draining),
            "stale" => Some(Self::Stale),
            _ => None,
        }
    }
}

/// A concrete backend (host:port) belonging to a [`Service`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Target {
    pub id: Id,
    pub service_id: Id,
    pub host: String,
    pub port: u16,
    pub weight: u32,
    /// Lower values are preferred. Load balancing happens within the lowest
    /// priority tier that contains a healthy target.
    #[serde(default)]
    pub priority: u16,
    pub enabled: bool,
    /// `None` for manual targets; set for targets owned by discovery.
    #[serde(default)]
    pub source_id: Option<Id>,
    /// Stable endpoint identity supplied by the discovery provider.
    #[serde(default)]
    pub provider_key: Option<String>,
    /// `active`, `draining`, or `stale` for discovered targets.
    #[serde(default)]
    pub state: TargetState,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    #[serde(default)]
    pub last_seen_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub missing_since: Option<DateTime<Utc>>,
}

/// A configured source that discovers targets for one service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverySource {
    pub id: Id,
    pub service_id: Id,
    pub name: String,
    pub provider: String,
    pub config: serde_json::Value,
    pub enabled: bool,
    pub stale_after_ms: u64,
    pub removal_grace_ms: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Operational state for a discovery source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DiscoveryState {
    #[default]
    Pending,
    Healthy,
    Failing,
    Stale,
    Disabled,
}

impl DiscoveryState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Healthy => "healthy",
            Self::Failing => "failing",
            Self::Stale => "stale",
            Self::Disabled => "disabled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "healthy" => Some(Self::Healthy),
            "failing" => Some(Self::Failing),
            "stale" => Some(Self::Stale),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }
}

/// Persisted operational state for a discovery source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverySourceStatus {
    pub source_id: Id,
    pub state: DiscoveryState,
    pub last_attempt_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub next_refresh_at: Option<DateTime<Utc>>,
    pub revision: Option<String>,
    pub endpoint_count: u64,
    pub active_count: u64,
    pub draining_count: u64,
    pub stale_count: u64,
    pub last_error: Option<String>,
}

/// One endpoint returned by a discovery provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredEndpoint {
    pub key: String,
    pub host: String,
    pub port: u16,
    pub weight: u32,
    pub priority: u16,
    pub metadata: BTreeMap<String, String>,
}

/// A request matcher that binds incoming traffic to a [`Service`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    pub id: Id,
    pub name: String,
    pub service_id: Id,
    /// Higher wins. Ties broken by longest matched path prefix.
    pub priority: i32,
    /// Host patterns (exact or `*.suffix`). Empty = match any host.
    pub hosts: Vec<String>,
    /// Path prefixes. Empty = match any path.
    pub paths: Vec<String>,
    /// Uppercase HTTP methods. Empty = match any method.
    pub methods: Vec<String>,
    /// Header conditions: every entry must match a request header. Names are
    /// case-insensitive, values exact; `"*"` means "present with any value".
    /// Empty = no constraint.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Weighted traffic splits across services (canary). Non-empty overrides
    /// `service_id` by smooth weighted round-robin; empty = single service.
    #[serde(default)]
    pub splits: Vec<RouteSplit>,
    /// Strip the matched path prefix before forwarding upstream.
    pub strip_path: bool,
    /// Forward the original `Host` header instead of the upstream's host.
    pub preserve_host: bool,
    pub enabled: bool,
}

/// A raw TCP (L4) listener proxied to a [`Service`]'s targets. Adding/removing a
/// stream route requires a restart to (un)bind the listener; retargeting an
/// existing one to another service applies live.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamRoute {
    pub id: Id,
    pub name: String,
    /// Local address the listener binds (e.g. `0.0.0.0:5432`).
    pub listen_addr: String,
    pub service_id: Id,
    pub enabled: bool,
}

/// One entry of a [`Route`]'s weighted traffic split.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteSplit {
    pub service_id: Id,
    pub weight: u32,
}

/// The kind of a [`Plugin`]. Built-in (native Rust) for this phase; WASM later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginType {
    KeyAuth,
    BasicAuth,
    Jwt,
    Acl,
    IpRestriction,
    RateLimit,
    ProxyCache,
    RequestSizeLimit,
    RequestTermination,
    Redirect,
    Cors,
    Wasm,
    RequestTransform,
    ResponseTransform,
    Hsts,
    ResponseBodyTransform,
    HttpLog,
    RequestId,
    ResponseCompression,
}

impl PluginType {
    pub fn as_str(self) -> &'static str {
        match self {
            PluginType::KeyAuth => "key-auth",
            PluginType::BasicAuth => "basic-auth",
            PluginType::Jwt => "jwt",
            PluginType::Acl => "acl",
            PluginType::IpRestriction => "ip-restriction",
            PluginType::RateLimit => "rate-limit",
            PluginType::ProxyCache => "proxy-cache",
            PluginType::RequestSizeLimit => "request-size-limit",
            PluginType::RequestTermination => "request-termination",
            PluginType::Redirect => "redirect",
            PluginType::Cors => "cors",
            PluginType::Wasm => "wasm",
            PluginType::RequestTransform => "request-transform",
            PluginType::ResponseTransform => "response-transform",
            PluginType::Hsts => "hsts",
            PluginType::ResponseBodyTransform => "response-body-transform",
            PluginType::HttpLog => "http-log",
            PluginType::RequestId => "request-id",
            PluginType::ResponseCompression => "response-compression",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "key-auth" => PluginType::KeyAuth,
            "basic-auth" => PluginType::BasicAuth,
            "jwt" => PluginType::Jwt,
            "acl" => PluginType::Acl,
            "ip-restriction" => PluginType::IpRestriction,
            "rate-limit" => PluginType::RateLimit,
            "proxy-cache" => PluginType::ProxyCache,
            "request-size-limit" => PluginType::RequestSizeLimit,
            "request-termination" => PluginType::RequestTermination,
            "redirect" => PluginType::Redirect,
            "cors" => PluginType::Cors,
            "wasm" => PluginType::Wasm,
            "request-transform" => PluginType::RequestTransform,
            "response-transform" => PluginType::ResponseTransform,
            "hsts" => PluginType::Hsts,
            "response-body-transform" => PluginType::ResponseBodyTransform,
            "http-log" => PluginType::HttpLog,
            "request-id" => PluginType::RequestId,
            "response-compression" => PluginType::ResponseCompression,
            _ => return None,
        })
    }
    /// Whether this plugin authenticates the request (sets a consumer).
    pub fn is_auth(self) -> bool {
        matches!(
            self,
            PluginType::KeyAuth | PluginType::BasicAuth | PluginType::Jwt
        )
    }
}

/// Where a plugin applies. Route scope overrides service scope overrides global.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginScope {
    Global,
    Service,
    Route,
}

impl PluginScope {
    pub fn as_str(self) -> &'static str {
        match self {
            PluginScope::Global => "global",
            PluginScope::Service => "service",
            PluginScope::Route => "route",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "global" => PluginScope::Global,
            "service" => PluginScope::Service,
            "route" => PluginScope::Route,
            _ => return None,
        })
    }
}

/// A configured plugin instance with opaque (per-type) JSON config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plugin {
    pub id: Id,
    #[serde(rename = "type")]
    pub plugin_type: PluginType,
    pub scope: PluginScope,
    pub service_id: Option<Id>,
    pub route_id: Option<Id>,
    pub config: serde_json::Value,
    pub ordering: i32,
    pub enabled: bool,
}

/// An identity that credentials authenticate to (for auth plugins / rate-limit keys).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Consumer {
    pub id: Id,
    pub username: String,
    /// Group names used by the `acl` plugin's allow/deny lists.
    #[serde(default)]
    pub groups: Vec<String>,
}

/// Credential kind. Mirrors the auth [`PluginType`]s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CredentialType {
    KeyAuth,
    BasicAuth,
    Jwt,
}

impl CredentialType {
    pub fn as_str(self) -> &'static str {
        match self {
            CredentialType::KeyAuth => "key-auth",
            CredentialType::BasicAuth => "basic-auth",
            CredentialType::Jwt => "jwt",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "key-auth" => CredentialType::KeyAuth,
            "basic-auth" => CredentialType::BasicAuth,
            "jwt" => CredentialType::Jwt,
            _ => return None,
        })
    }
}

/// A credential belonging to a [`Consumer`].
///
/// - key-auth: `identifier` = API key, `secret` unused.
/// - basic-auth: `identifier` = username, `secret` = bcrypt password hash.
/// - jwt: `identifier` = key (matched against the token's key claim, e.g. `iss`),
///   `secret` = JSON `{"algorithm": "...", "secret": "..."}` (HMAC secret or RSA
///   public key PEM).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsumerCredential {
    pub id: Id,
    pub consumer_id: Id,
    #[serde(rename = "type")]
    pub credential_type: CredentialType,
    pub identifier: String,
    /// Secret hash. Never serialized back out to API clients (the API redacts it).
    #[serde(skip_serializing)]
    pub secret: Option<String>,
}

/// A user-supplied WASM plugin module. `wasm` bytes are held in memory by the proxy
/// snapshot and never serialized to API clients (listings expose size only).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmModule {
    pub id: Id,
    pub name: String,
    pub description: String,
    #[serde(skip_serializing)]
    pub wasm: Vec<u8>,
    pub created_at: DateTime<Utc>,
}

/// A TLS certificate + private key (PEM). The private key is a secret and is never
/// serialized back to API clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Certificate {
    pub id: Id,
    pub name: String,
    pub sni: Vec<String>,
    pub cert_pem: String,
    #[serde(skip_serializing)]
    pub key_pem: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acme_config: Option<AcmeConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acme_status: Option<AcmeStatus>,
}

pub const LETS_ENCRYPT_PRODUCTION: &str = "https://acme-v02.api.letsencrypt.org/directory";
pub const LETS_ENCRYPT_STAGING: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AcmeChallenge {
    #[serde(rename = "dns-01")]
    Dns01,
    #[serde(rename = "tls-alpn-01")]
    TlsAlpn01,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcmeConfig {
    #[serde(default = "default_acme_directory")]
    pub directory_url: String,
    pub challenge: AcmeChallenge,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

pub const ZEROSSL_DIRECTORY: &str = "https://acme.zerossl.com/v2/DV90";

/// Registration secrets. Serialized only in explicitly requested secret backups.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcmeEabCredentials {
    pub directory_url: String,
    pub key_id: String,
    pub hmac_key: String,
}

impl std::fmt::Debug for AcmeEabCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcmeEabCredentials")
            .field("directory_url", &self.directory_url)
            .field("credentials", &"[redacted]")
            .finish()
    }
}

fn default_acme_directory() -> String {
    LETS_ENCRYPT_PRODUCTION.to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcmeState {
    Pending,
    Issuing,
    Issued,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcmeStatus {
    pub state: AcmeState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issued_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_attempt: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

impl AcmeStatus {
    pub fn pending() -> Self {
        Self {
            state: AcmeState::Pending,
            issued_at: None,
            expires_at: None,
            last_attempt: None,
            last_error: None,
        }
    }
}

/// Global runtime settings (singleton).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub proxy_http_addr: String,
    pub proxy_https_addr: Option<String>,
    pub admin_addr: String,
    pub default_lb: LbAlgorithm,
    /// Which certificate the HTTPS listener serves (rustls: one cert per listener).
    pub active_certificate_id: Option<Id>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            proxy_http_addr: "0.0.0.0:8080".to_string(),
            proxy_https_addr: Some("0.0.0.0:8443".to_string()),
            // Admin API is unauthenticated; bind to loopback by default (least privilege).
            admin_addr: "127.0.0.1:9080".to_string(),
            default_lb: LbAlgorithm::RoundRobin,
            active_certificate_id: None,
        }
    }
}
