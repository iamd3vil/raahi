//! Declarative config file: the hand-written shape `raahi apply` sends to
//! `POST /api/v1/config/apply`. Unlike the export document it carries no ids —
//! services, routes, consumers and certificates are keyed by name, and plugins
//! nest under the service or route they apply to.
//!
//! A top-level section that is absent (`None`) is left untouched by apply; a
//! present one (even empty) is fully managed: entries missing from it are deleted.
//! Within an entry, nested lists (targets, discovery, plugins, credentials) are
//! always fully managed.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::*;
use crate::spec::*;

/// The only `raahi_config` version this build understands.
pub const CONFIG_VERSION: u32 = 1;

/// Text formats a config file can be written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFormat {
    Json,
    Yaml,
    Huml,
}

impl ConfigFormat {
    /// From a `--format` / `?format=` value or a file extension.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "json" => Some(Self::Json),
            "yaml" | "yml" => Some(Self::Yaml),
            "huml" => Some(Self::Huml),
            _ => None,
        }
    }

    /// From a request `Content-Type` (parameters such as `charset` are ignored).
    pub fn from_content_type(content_type: &str) -> Option<Self> {
        let essence = content_type.split(';').next().unwrap_or("").trim();
        match essence.to_ascii_lowercase().as_str() {
            "application/json" => Some(Self::Json),
            "application/yaml" | "application/x-yaml" | "text/yaml" | "text/x-yaml" => {
                Some(Self::Yaml)
            }
            "application/huml" | "text/huml" => Some(Self::Huml),
            _ => None,
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            Self::Json => "application/json",
            Self::Yaml => "application/yaml",
            Self::Huml => "application/huml",
        }
    }
}

/// Parse a config file. Unknown fields are rejected so typos (or id-based export
/// fields such as `service_id`) fail loudly instead of being ignored.
pub fn parse_config(text: &str, format: ConfigFormat) -> Result<ConfigDoc, String> {
    match format {
        ConfigFormat::Json => serde_json::from_str(text).map_err(|e| e.to_string()),
        ConfigFormat::Yaml => serde_yaml::from_str(text).map_err(|e| e.to_string()),
        ConfigFormat::Huml => huml_rs::serde::from_str(text).map_err(|e| e.to_string()),
    }
}

pub fn render_config(doc: &ConfigDoc, format: ConfigFormat) -> Result<String, String> {
    let mut text = match format {
        ConfigFormat::Json => serde_json::to_string_pretty(doc).map_err(|e| e.to_string())?,
        ConfigFormat::Yaml => serde_yaml::to_string(doc).map_err(|e| e.to_string())?,
        ConfigFormat::Huml => huml_rs::serde::to_string(doc).map_err(|e| e.to_string())?,
    };
    // JSON and HUML end without one; files written from a dump should.
    if !text.ends_with('\n') {
        text.push('\n');
    }
    Ok(text)
}

fn is_false(v: &bool) -> bool {
    !*v
}
fn is_true(v: &bool) -> bool {
    *v
}
fn is_zero_i32(v: &i32) -> bool {
    *v == 0
}
fn is_zero_u16(v: &u16) -> bool {
    *v == 0
}
fn is_empty_obj(v: &serde_json::Value) -> bool {
    v.as_object().is_some_and(|o| o.is_empty())
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigDoc {
    pub raahi_config: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub services: Option<Vec<ConfigService>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routes: Option<Vec<ConfigRoute>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_routes: Option<Vec<ConfigStreamRoute>>,
    /// Global plugins. Service and route plugins nest under their owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins: Option<Vec<ConfigPlugin>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumers: Option<Vec<ConfigConsumer>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificates: Option<Vec<ConfigCertificate>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigService {
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_authority: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_sni: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_path: Option<String>,
    /// Manual targets. Discovered targets are owned by `discovery` sources.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<ConfigTarget>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub discovery: Vec<ConfigDiscoverySource>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plugins: Vec<ConfigPlugin>,
}

impl ConfigService {
    pub fn spec(&self) -> ServiceSpec {
        ServiceSpec {
            name: self.name.clone(),
            protocol: self.protocol,
            connect_timeout_ms: self.connect_timeout_ms,
            read_timeout_ms: self.read_timeout_ms,
            write_timeout_ms: self.write_timeout_ms,
            retries: self.retries,
            lb_algorithm: self.lb_algorithm,
            upstream_authority: self.upstream_authority.clone(),
            tls_sni: self.tls_sni.clone(),
            health_path: self.health_path.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigTarget {
    pub host: String,
    pub port: u16,
    #[serde(default = "d_weight")]
    pub weight: u32,
    #[serde(default, skip_serializing_if = "is_zero_u16")]
    pub priority: u16,
    #[serde(default = "d_true", skip_serializing_if = "is_true")]
    pub enabled: bool,
}

impl ConfigTarget {
    pub fn spec(&self) -> TargetSpec {
        TargetSpec {
            host: self.host.clone(),
            port: self.port,
            weight: self.weight,
            priority: self.priority,
            enabled: self.enabled,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigDiscoverySource {
    pub name: String,
    pub provider: String,
    #[serde(default = "d_obj")]
    pub config: serde_json::Value,
    #[serde(default = "d_true", skip_serializing_if = "is_true")]
    pub enabled: bool,
    #[serde(default = "d_stale_after")]
    pub stale_after_ms: u64,
    #[serde(default = "d_removal_grace")]
    pub removal_grace_ms: u64,
}

impl ConfigDiscoverySource {
    pub fn spec(&self) -> DiscoverySourceSpec {
        DiscoverySourceSpec {
            name: self.name.clone(),
            provider: self.provider.clone(),
            config: self.config.clone(),
            enabled: self.enabled,
            stale_after_ms: self.stale_after_ms,
            removal_grace_ms: self.removal_grace_ms,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigRoute {
    pub name: String,
    /// Service name.
    pub service: String,
    #[serde(default, skip_serializing_if = "is_zero_i32")]
    pub priority: i32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub methods: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub splits: Vec<ConfigSplit>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub strip_path: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub preserve_host: bool,
    #[serde(default = "d_true", skip_serializing_if = "is_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plugins: Vec<ConfigPlugin>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigSplit {
    /// Service name.
    pub service: String,
    pub weight: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigStreamRoute {
    pub name: String,
    pub listen_addr: String,
    /// Service name.
    pub service: String,
    #[serde(default = "d_true", skip_serializing_if = "is_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigPlugin {
    #[serde(rename = "type")]
    pub plugin_type: PluginType,
    #[serde(default = "d_obj", skip_serializing_if = "is_empty_obj")]
    pub config: serde_json::Value,
    #[serde(default, skip_serializing_if = "is_zero_i32")]
    pub ordering: i32,
    #[serde(default = "d_true", skip_serializing_if = "is_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigConsumer {
    pub username: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub credentials: Vec<ConfigCredential>,
}

/// Same fields as [`CredentialSpec`]. An omitted `secret` on a basic-auth or jwt
/// credential that already exists keeps the stored one (dumps never carry
/// secrets); a new one must provide it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigCredential {
    #[serde(rename = "type")]
    pub credential_type: CredentialType,
    pub identifier: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub algorithm: Option<String>,
}

/// Either ACME-managed (`acme_config`) or manual (`cert_pem` + `key_pem`). A
/// manual certificate that already exists keeps its stored PEMs when they are
/// omitted (dumps never carry them).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigCertificate {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sni: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert_pem: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_pem: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acme_config: Option<AcmeConfig>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const YAML: &str = r#"
raahi_config: 1
services:
  - name: billing-api
    health_path: /healthz
    targets:
      - { host: 10.0.0.5, port: 8080 }
    plugins:
      - type: rate-limit
        config: { limit: 10, window_secs: 1 }
routes:
  - name: billing
    service: billing-api
    hosts: [billing.example.com]
    paths: [/]
    headers: { X-Env: prod }
    splits:
      - { service: billing-api, weight: 1 }
consumers:
  - username: ci-bot
    credentials:
      - { type: key-auth, identifier: "${CI_BOT_KEY}" }
"#;

    #[test]
    fn yaml_parses_with_defaults_and_absent_sections() {
        let doc = parse_config(YAML, ConfigFormat::Yaml).unwrap();
        let services = doc.services.as_ref().unwrap();
        assert_eq!(services[0].connect_timeout_ms, 5_000);
        assert_eq!(services[0].targets[0].weight, 100);
        assert!(services[0].targets[0].enabled);
        assert_eq!(services[0].plugins[0].plugin_type, PluginType::RateLimit);
        assert_eq!(doc.routes.as_ref().unwrap()[0].service, "billing-api");
        assert!(doc.stream_routes.is_none());
        assert!(doc.plugins.is_none());
        assert!(doc.certificates.is_none());
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let text = "raahi_config: 1\nroutes:\n  - { name: r, service_id: 3 }\n";
        let err = parse_config(text, ConfigFormat::Yaml).unwrap_err();
        assert!(err.contains("service_id"), "{err}");
    }

    #[test]
    fn every_format_round_trips() {
        let mut doc = parse_config(YAML, ConfigFormat::Yaml).unwrap();
        doc.stream_routes = Some(vec![]);
        doc.certificates = Some(vec![ConfigCertificate {
            name: "example".into(),
            sni: vec!["example.com".into()],
            cert_pem: None,
            key_pem: None,
            acme_config: Some(AcmeConfig {
                directory_url: LETS_ENCRYPT_STAGING.into(),
                challenge: AcmeChallenge::Dns01,
                email: Some("ops@example.com".into()),
            }),
        }]);
        for format in [ConfigFormat::Json, ConfigFormat::Yaml, ConfigFormat::Huml] {
            let text = render_config(&doc, format).unwrap();
            let back =
                parse_config(&text, format).unwrap_or_else(|e| panic!("{format:?}: {e}\n{text}"));
            assert_eq!(back, doc, "{format:?}:\n{text}");
            assert!(text.ends_with('\n'), "{format:?}");
        }
    }

    #[test]
    fn huml_parses_hand_written_file() {
        let text = r#"
raahi_config: 1
services::
  - ::
    name: "web"
    targets::
      - ::
        host: "127.0.0.1"
        port: 3000
routes::
  - ::
    name: "web"
    service: "web"
    hosts:: "web.example.com"
    paths:: "/"
"#;
        let doc = parse_config(text, ConfigFormat::Huml).unwrap();
        assert_eq!(doc.services.unwrap()[0].targets[0].port, 3000);
        assert_eq!(doc.routes.unwrap()[0].hosts, vec!["web.example.com"]);
    }

    #[test]
    fn content_types() {
        assert_eq!(
            ConfigFormat::from_content_type("application/yaml; charset=utf-8"),
            Some(ConfigFormat::Yaml)
        );
        assert_eq!(
            ConfigFormat::from_content_type("application/huml"),
            Some(ConfigFormat::Huml)
        );
        assert_eq!(ConfigFormat::from_content_type("text/plain"), None);
    }
}
