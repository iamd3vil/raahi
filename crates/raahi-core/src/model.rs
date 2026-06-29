//! Domain entity types. These double as the wire (JSON) representation used by the
//! admin API and the in-memory representation used by the proxy. The store maps
//! SQLite rows to and from these.

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
    /// SNI to present to the upstream when `protocol = https`. Falls back to the
    /// target host when unset.
    pub tls_sni: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A concrete backend (host:port) belonging to a [`Service`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Target {
    pub id: Id,
    pub service_id: Id,
    pub host: String,
    pub port: u16,
    pub weight: u32,
    pub enabled: bool,
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
    /// Strip the matched path prefix before forwarding upstream.
    pub strip_path: bool,
    /// Forward the original `Host` header instead of the upstream's host.
    pub preserve_host: bool,
    pub enabled: bool,
}

/// The kind of a [`Plugin`]. Built-in (native Rust) for this phase; WASM later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginType {
    KeyAuth,
    BasicAuth,
    RateLimit,
    Cors,
    RequestTransform,
    ResponseTransform,
}

impl PluginType {
    pub fn as_str(self) -> &'static str {
        match self {
            PluginType::KeyAuth => "key-auth",
            PluginType::BasicAuth => "basic-auth",
            PluginType::RateLimit => "rate-limit",
            PluginType::Cors => "cors",
            PluginType::RequestTransform => "request-transform",
            PluginType::ResponseTransform => "response-transform",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "key-auth" => PluginType::KeyAuth,
            "basic-auth" => PluginType::BasicAuth,
            "rate-limit" => PluginType::RateLimit,
            "cors" => PluginType::Cors,
            "request-transform" => PluginType::RequestTransform,
            "response-transform" => PluginType::ResponseTransform,
            _ => return None,
        })
    }
    /// Whether this plugin authenticates the request (sets a consumer).
    pub fn is_auth(self) -> bool {
        matches!(self, PluginType::KeyAuth | PluginType::BasicAuth)
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
}

/// Credential kind. Mirrors the auth [`PluginType`]s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CredentialType {
    KeyAuth,
    BasicAuth,
}

impl CredentialType {
    pub fn as_str(self) -> &'static str {
        match self {
            CredentialType::KeyAuth => "key-auth",
            CredentialType::BasicAuth => "basic-auth",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "key-auth" => CredentialType::KeyAuth,
            "basic-auth" => CredentialType::BasicAuth,
            _ => return None,
        })
    }
}

/// A credential belonging to a [`Consumer`].
///
/// - key-auth: `identifier` = API key, `secret` unused.
/// - basic-auth: `identifier` = username, `secret` = bcrypt password hash.
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
