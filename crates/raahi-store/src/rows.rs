//! Row → domain-model mapping helpers. We use runtime queries (not the compile-time
//! `query!` macros) so the build needs no live database.

use chrono::{DateTime, NaiveDateTime, Utc};
use raahi_core::*;
use sqlx::sqlite::SqliteRow;
use sqlx::Row;

/// Tolerant timestamp parse: accepts RFC3339 or SQLite's `datetime()` text format.
pub fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .or_else(|_| {
            NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").map(|n| n.and_utc())
        })
        .unwrap_or_else(|_| Utc::now())
}

fn json_strings(s: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(s).unwrap_or_default()
}

pub fn map_service(r: &SqliteRow) -> Service {
    Service {
        id: r.get("id"),
        name: r.get("name"),
        protocol: Protocol::from_str(&r.get::<String, _>("protocol")).unwrap_or(Protocol::Http),
        connect_timeout_ms: r.get::<i64, _>("connect_timeout_ms") as u64,
        read_timeout_ms: r.get::<i64, _>("read_timeout_ms") as u64,
        write_timeout_ms: r.get::<i64, _>("write_timeout_ms") as u64,
        retries: r.get::<i64, _>("retries") as u32,
        lb_algorithm: LbAlgorithm::from_str(&r.get::<String, _>("lb_algorithm"))
            .unwrap_or_default(),
        tls_sni: r.get("tls_sni"),
        created_at: parse_dt(&r.get::<String, _>("created_at")),
        updated_at: parse_dt(&r.get::<String, _>("updated_at")),
    }
}

pub fn map_target(r: &SqliteRow) -> Target {
    Target {
        id: r.get("id"),
        service_id: r.get("service_id"),
        host: r.get("host"),
        port: r.get::<i64, _>("port") as u16,
        weight: r.get::<i64, _>("weight") as u32,
        enabled: r.get::<i64, _>("enabled") != 0,
    }
}

pub fn map_route(r: &SqliteRow) -> Route {
    Route {
        id: r.get("id"),
        name: r.get("name"),
        service_id: r.get("service_id"),
        priority: r.get::<i64, _>("priority") as i32,
        hosts: json_strings(&r.get::<String, _>("hosts")),
        paths: json_strings(&r.get::<String, _>("paths")),
        methods: json_strings(&r.get::<String, _>("methods")),
        strip_path: r.get::<i64, _>("strip_path") != 0,
        preserve_host: r.get::<i64, _>("preserve_host") != 0,
        enabled: r.get::<i64, _>("enabled") != 0,
    }
}

pub fn map_plugin(r: &SqliteRow) -> Plugin {
    Plugin {
        id: r.get("id"),
        plugin_type: PluginType::from_str(&r.get::<String, _>("type"))
            .unwrap_or(PluginType::Cors),
        scope: PluginScope::from_str(&r.get::<String, _>("scope"))
            .unwrap_or(PluginScope::Global),
        service_id: r.get("service_id"),
        route_id: r.get("route_id"),
        config: serde_json::from_str(&r.get::<String, _>("config"))
            .unwrap_or_else(|_| serde_json::json!({})),
        ordering: r.get::<i64, _>("ordering") as i32,
        enabled: r.get::<i64, _>("enabled") != 0,
    }
}

pub fn map_consumer(r: &SqliteRow) -> Consumer {
    Consumer {
        id: r.get("id"),
        username: r.get("username"),
    }
}

pub fn map_credential(r: &SqliteRow) -> ConsumerCredential {
    ConsumerCredential {
        id: r.get("id"),
        consumer_id: r.get("consumer_id"),
        credential_type: CredentialType::from_str(&r.get::<String, _>("type"))
            .unwrap_or(CredentialType::KeyAuth),
        identifier: r.get("identifier"),
        secret: r.get("secret"),
    }
}

pub fn map_certificate(r: &SqliteRow) -> Certificate {
    Certificate {
        id: r.get("id"),
        name: r.get("name"),
        sni: json_strings(&r.get::<String, _>("sni")),
        cert_pem: r.get("cert_pem"),
        key_pem: r.get("key_pem"),
    }
}

pub fn map_settings(r: &SqliteRow) -> Settings {
    Settings {
        proxy_http_addr: r.get("proxy_http_addr"),
        proxy_https_addr: r.get("proxy_https_addr"),
        admin_addr: r.get("admin_addr"),
        default_lb: LbAlgorithm::from_str(&r.get::<String, _>("default_lb")).unwrap_or_default(),
        active_certificate_id: r.get("active_certificate_id"),
    }
}
