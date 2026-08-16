//! Input specs for create/update operations. These are the user-settable subsets of
//! the entities (no `id`, no server-managed timestamps), deserialized directly from
//! API request bodies and consumed by the store. Updates are full replacements (PUT).

use serde::{Deserialize, Serialize};

use crate::model::*;
use crate::Id;

fn d_http() -> Protocol {
    Protocol::Http
}
fn d_connect() -> u64 {
    5_000
}
fn d_read() -> u64 {
    60_000
}
fn d_write() -> u64 {
    60_000
}
fn d_retries() -> u32 {
    1
}
fn d_weight() -> u32 {
    100
}
fn d_true() -> bool {
    true
}
fn d_obj() -> serde_json::Value {
    serde_json::json!({})
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceSpec {
    pub name: String,
    #[serde(default = "d_http")]
    pub protocol: Protocol,
    #[serde(default = "d_connect")]
    pub connect_timeout_ms: u64,
    #[serde(default = "d_read")]
    pub read_timeout_ms: u64,
    #[serde(default = "d_write")]
    pub write_timeout_ms: u64,
    #[serde(default = "d_retries")]
    pub retries: u32,
    #[serde(default)]
    pub lb_algorithm: LbAlgorithm,
    #[serde(default)]
    pub tls_sni: Option<String>,
    /// Optional HTTP health-check path (e.g. `/healthz`); unset = TCP check.
    #[serde(default)]
    pub health_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetSpec {
    pub host: String,
    pub port: u16,
    #[serde(default = "d_weight")]
    pub weight: u32,
    #[serde(default = "d_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteSpec {
    pub name: String,
    pub service_id: Id,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub methods: Vec<String>,
    #[serde(default)]
    pub strip_path: bool,
    #[serde(default)]
    pub preserve_host: bool,
    #[serde(default = "d_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginSpec {
    #[serde(rename = "type")]
    pub plugin_type: PluginType,
    pub scope: PluginScope,
    #[serde(default)]
    pub service_id: Option<Id>,
    #[serde(default)]
    pub route_id: Option<Id>,
    #[serde(default = "d_obj")]
    pub config: serde_json::Value,
    #[serde(default)]
    pub ordering: i32,
    #[serde(default = "d_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsumerSpec {
    pub username: String,
    /// Group names for the `acl` plugin.
    #[serde(default)]
    pub groups: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialSpec {
    #[serde(rename = "type")]
    pub credential_type: CredentialType,
    /// API key (key-auth), username (basic-auth), or key claim value (jwt).
    pub identifier: String,
    /// basic-auth: plaintext password (hashed before storage).
    /// jwt: HMAC secret (HS*) or RSA public key PEM (RS256).
    /// key-auth: unused.
    #[serde(default)]
    pub secret: Option<String>,
    /// jwt only: HS256 (default) / HS384 / HS512 / RS256.
    #[serde(default)]
    pub algorithm: Option<String>,
}

/// Upload spec for a WASM module: either raw wasm (base64) or WAT source text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmModuleSpec {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub wasm_base64: Option<String>,
    #[serde(default)]
    pub wat: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertificateSpec {
    pub name: String,
    #[serde(default)]
    pub sni: Vec<String>,
    pub cert_pem: String,
    pub key_pem: String,
}

/// The import document — the same shape `GET /export` produces. Entity ids inside
/// are the *exporting* instance's ids; the importer remaps them.
#[derive(Debug, Clone, Deserialize)]
pub struct ImportDoc {
    pub raahi_export_version: u32,
    #[serde(default)]
    pub settings: Option<SettingsSpec>,
    #[serde(default)]
    pub services: Vec<ImportService>,
    #[serde(default)]
    pub routes: Vec<Route>,
    #[serde(default)]
    pub plugins: Vec<Plugin>,
    #[serde(default)]
    pub consumers: Vec<ImportConsumer>,
    /// Raw values: certificates without `key_pem` (redacted exports) are skipped.
    #[serde(default)]
    pub certificates: Vec<serde_json::Value>,
    /// Raw values: `{name, description, wasm_base64}`.
    #[serde(default)]
    pub wasm_modules: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImportService {
    pub service: Service,
    #[serde(default)]
    pub targets: Vec<Target>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImportConsumer {
    pub consumer: Consumer,
    #[serde(default)]
    pub credentials: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsSpec {
    pub proxy_http_addr: String,
    #[serde(default)]
    pub proxy_https_addr: Option<String>,
    pub admin_addr: String,
    #[serde(default)]
    pub default_lb: LbAlgorithm,
    #[serde(default)]
    pub active_certificate_id: Option<Id>,
}
