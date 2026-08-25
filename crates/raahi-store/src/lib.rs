//! Raahi config store: SQLite persistence (via sqlx) and compilation of the in-memory
//! [`ProxyConfig`] snapshot the data plane reads.

mod crud;
mod rows;

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use raahi_core::{CredentialType, ImportDoc, JwtCred, ProxyConfig, RouteSplit, Target};
use serde::Serialize;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("not found")]
    NotFound,
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("invalid: {0}")]
    Invalid(String),
}

/// SQLite-backed configuration store. Cheap to clone (`SqlitePool` is an `Arc`).
#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
    version: std::sync::Arc<AtomicU64>,
}

/// What an import did — counts of restored entities plus anything skipped and why.
#[derive(Debug, Default, Serialize)]
pub struct ImportReport {
    pub services: usize,
    pub targets: usize,
    pub routes: usize,
    pub stream_routes: usize,
    pub plugins: usize,
    pub consumers: usize,
    pub credentials: usize,
    pub certificates: usize,
    pub wasm_modules: usize,
    pub skipped: Vec<String>,
}

impl Store {
    /// Open (creating if needed) the SQLite database at `db_url` (e.g.
    /// `sqlite://raahi.db` or `sqlite::memory:`), run migrations, and ensure the
    /// singleton settings row exists.
    pub async fn connect(db_url: &str) -> Result<Self, StoreError> {
        let opts = SqliteConnectOptions::from_str(db_url)
            .map_err(StoreError::Db)?
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));

        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(opts)
            .await?;

        MIGRATOR.run(&pool).await?;

        let store = Store {
            pool,
            version: std::sync::Arc::new(AtomicU64::new(0)),
        };
        store.ensure_settings().await?;
        Ok(store)
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    async fn ensure_settings(&self) -> Result<(), StoreError> {
        let d = raahi_core::Settings::default();
        sqlx::query(
            "INSERT OR IGNORE INTO settings \
             (id, proxy_http_addr, proxy_https_addr, admin_addr, default_lb, active_certificate_id) \
             VALUES (1, ?, ?, ?, ?, NULL)",
        )
        .bind(&d.proxy_http_addr)
        .bind(&d.proxy_https_addr)
        .bind(&d.admin_addr)
        .bind(d.default_lb.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Declarative full-replace import of an export document, inside one
    /// transaction: existing routing config is wiped and rebuilt from `doc`.
    /// Entity ids are remapped; the admin token and listener addresses survive
    /// unless the document carries settings. Certificates without a `key_pem`
    /// (redacted exports) and dangling references are skipped, not fatal.
    pub async fn import(&self, doc: &ImportDoc) -> Result<ImportReport, StoreError> {
        use std::collections::HashMap as Map;

        if doc.raahi_export_version != 1 {
            return Err(StoreError::Invalid(format!(
                "unsupported export version {}",
                doc.raahi_export_version
            )));
        }

        let mut report = ImportReport::default();
        let mut tx = self.pool.begin().await?;

        for table in [
            "plugins",
            "routes",
            "stream_routes",
            "consumer_credentials",
            "consumers",
            "targets",
            "services",
            "certificates",
            "acme_accounts",
            "wasm_modules",
        ] {
            sqlx::query(&format!("DELETE FROM {table}"))
                .execute(&mut *tx)
                .await?;
        }

        // Services + targets (old id -> new id).
        let mut svc_map: Map<i64, i64> = Map::new();
        for is in &doc.services {
            let s = &is.service;
            let res = sqlx::query(
                "INSERT INTO services (name, protocol, connect_timeout_ms, read_timeout_ms, \
                 write_timeout_ms, retries, lb_algorithm, tls_sni, health_path, created_at, updated_at) \
                 VALUES (?,?,?,?,?,?,?,?,?,datetime('now'),datetime('now'))",
            )
            .bind(&s.name)
            .bind(s.protocol.as_str())
            .bind(s.connect_timeout_ms as i64)
            .bind(s.read_timeout_ms as i64)
            .bind(s.write_timeout_ms as i64)
            .bind(s.retries as i64)
            .bind(s.lb_algorithm.as_str())
            .bind(&s.tls_sni)
            .bind(&s.health_path)
            .execute(&mut *tx)
            .await?;
            let new_id = res.last_insert_rowid();
            svc_map.insert(s.id, new_id);
            report.services += 1;

            for t in &is.targets {
                sqlx::query(
                    "INSERT INTO targets (service_id, host, port, weight, enabled) VALUES (?,?,?,?,?)",
                )
                .bind(new_id)
                .bind(&t.host)
                .bind(t.port as i64)
                .bind(t.weight as i64)
                .bind(t.enabled as i64)
                .execute(&mut *tx)
                .await?;
                report.targets += 1;
            }
        }

        // Routes (old id -> new id).
        let mut route_map: Map<i64, i64> = Map::new();
        for r in &doc.routes {
            let Some(&sid) = svc_map.get(&r.service_id) else {
                report.skipped.push(format!(
                    "route '{}': unknown service {}",
                    r.name, r.service_id
                ));
                continue;
            };
            // Remap split service ids; entries pointing at services that didn't make
            // it into this import are dropped, not fatal.
            let mut splits: Vec<RouteSplit> = Vec::new();
            for sp in &r.splits {
                match svc_map.get(&sp.service_id) {
                    Some(&new) => splits.push(RouteSplit {
                        service_id: new,
                        weight: sp.weight,
                    }),
                    None => report.skipped.push(format!(
                        "route '{}': split references unknown service {}",
                        r.name, sp.service_id
                    )),
                }
            }
            let res = sqlx::query(
                "INSERT INTO routes (name, service_id, priority, hosts, paths, methods, \
                 headers, splits, strip_path, preserve_host, enabled) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            )
            .bind(&r.name)
            .bind(sid)
            .bind(r.priority as i64)
            .bind(serde_json::to_string(&r.hosts).unwrap_or_default())
            .bind(serde_json::to_string(&r.paths).unwrap_or_default())
            .bind(serde_json::to_string(&r.methods).unwrap_or_default())
            .bind(serde_json::to_string(&r.headers).unwrap_or_default())
            .bind(serde_json::to_string(&splits).unwrap_or_default())
            .bind(r.strip_path as i64)
            .bind(r.preserve_host as i64)
            .bind(r.enabled as i64)
            .execute(&mut *tx)
            .await?;
            route_map.insert(r.id, res.last_insert_rowid());
            report.routes += 1;
        }

        // Stream routes (service ids remapped; unknown services skipped, not fatal).
        for sr in &doc.stream_routes {
            let Some(&sid) = svc_map.get(&sr.service_id) else {
                report.skipped.push(format!(
                    "stream route '{}': unknown service {}",
                    sr.name, sr.service_id
                ));
                continue;
            };
            sqlx::query(
                "INSERT INTO stream_routes (name, listen_addr, service_id, enabled) VALUES (?,?,?,?)",
            )
            .bind(&sr.name)
            .bind(&sr.listen_addr)
            .bind(sid)
            .bind(sr.enabled as i64)
            .execute(&mut *tx)
            .await?;
            report.stream_routes += 1;
        }

        // Consumers + credentials.
        for ic in &doc.consumers {
            let c = &ic.consumer;
            let res = sqlx::query("INSERT INTO consumers (username, groups) VALUES (?,?)")
                .bind(&c.username)
                .bind(serde_json::to_string(&c.groups).unwrap_or_else(|_| "[]".into()))
                .execute(&mut *tx)
                .await?;
            let cid = res.last_insert_rowid();
            report.consumers += 1;

            for cr in &ic.credentials {
                let ctype = cr["type"].as_str().unwrap_or("");
                let identifier = cr["identifier"].as_str().unwrap_or("");
                let secret = cr["secret"].as_str();
                if identifier.is_empty() || CredentialType::from_str(ctype).is_none() {
                    report
                        .skipped
                        .push(format!("credential for '{}': malformed", c.username));
                    continue;
                }
                // basic-auth / jwt credentials are useless without their secret
                // (redacted exports).
                if secret.is_none() && ctype != "key-auth" {
                    report.skipped.push(format!(
                        "{ctype} credential '{identifier}' for '{}': secret not in export",
                        c.username
                    ));
                    continue;
                }
                sqlx::query(
                    "INSERT INTO consumer_credentials (consumer_id, type, identifier, secret) \
                     VALUES (?,?,?,?)",
                )
                .bind(cid)
                .bind(ctype)
                .bind(identifier)
                .bind(secret)
                .execute(&mut *tx)
                .await?;
                report.credentials += 1;
            }
        }

        // Certificates (old id -> new id, for settings.active_certificate_id).
        let mut cert_map: Map<i64, i64> = Map::new();
        for cv in &doc.certificates {
            let name = cv["name"].as_str().unwrap_or("");
            let key_pem = cv["key_pem"].as_str().unwrap_or("");
            let acme_config = cv.get("acme_config").filter(|value| !value.is_null());
            if key_pem.is_empty() && acme_config.is_none() {
                report.skipped.push(format!(
                    "certificate '{name}': key_pem not in export (re-export with include_secrets=true)"
                ));
                continue;
            }
            let res = sqlx::query(
                "INSERT INTO certificates \
                 (name, sni, cert_pem, key_pem, acme_config, acme_status) VALUES (?,?,?,?,?,?)",
            )
            .bind(name)
            .bind(cv["sni"].to_string())
            .bind(cv["cert_pem"].as_str().unwrap_or(""))
            .bind(key_pem)
            .bind(acme_config.map(|value| value.to_string()))
            .bind(
                cv.get("acme_status")
                    .filter(|value| !value.is_null())
                    .map(|value| value.to_string()),
            )
            .execute(&mut *tx)
            .await?;
            if let Some(old) = cv["id"].as_i64() {
                cert_map.insert(old, res.last_insert_rowid());
            }
            report.certificates += 1;
        }

        // WASM modules (bytes validated by the API layer before import).
        for mv in &doc.wasm_modules {
            let name = mv["name"].as_str().unwrap_or("");
            let Some(b64) = mv["wasm_base64"].as_str() else {
                report
                    .skipped
                    .push(format!("wasm module '{name}': no wasm_base64 in export"));
                continue;
            };
            use base64::Engine;
            let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) else {
                report
                    .skipped
                    .push(format!("wasm module '{name}': invalid base64"));
                continue;
            };
            sqlx::query(
                "INSERT INTO wasm_modules (name, description, wasm, created_at) \
                 VALUES (?,?,?,datetime('now'))",
            )
            .bind(name)
            .bind(mv["description"].as_str().unwrap_or(""))
            .bind(&bytes)
            .execute(&mut *tx)
            .await?;
            report.wasm_modules += 1;
        }

        // Plugins (with remapped scope references).
        for p in &doc.plugins {
            let sid = match p.service_id {
                Some(old) => match svc_map.get(&old) {
                    Some(&new) => Some(new),
                    None => {
                        report.skipped.push(format!(
                            "plugin {}: unknown service {old}",
                            p.plugin_type.as_str()
                        ));
                        continue;
                    }
                },
                None => None,
            };
            let rid = match p.route_id {
                Some(old) => match route_map.get(&old) {
                    Some(&new) => Some(new),
                    None => {
                        report.skipped.push(format!(
                            "plugin {}: unknown route {old}",
                            p.plugin_type.as_str()
                        ));
                        continue;
                    }
                },
                None => None,
            };
            sqlx::query(
                "INSERT INTO plugins (type, scope, service_id, route_id, config, ordering, enabled) \
                 VALUES (?,?,?,?,?,?,?)",
            )
            .bind(p.plugin_type.as_str())
            .bind(p.scope.as_str())
            .bind(sid)
            .bind(rid)
            .bind(p.config.to_string())
            .bind(p.ordering as i64)
            .bind(p.enabled as i64)
            .execute(&mut *tx)
            .await?;
            report.plugins += 1;
        }

        // Settings (admin token hash is never touched by imports).
        if let Some(s) = &doc.settings {
            let active_cert = s
                .active_certificate_id
                .and_then(|old| cert_map.get(&old).copied());
            sqlx::query(
                "UPDATE settings SET proxy_http_addr=?, proxy_https_addr=?, admin_addr=?, \
                 default_lb=?, active_certificate_id=? WHERE id=1",
            )
            .bind(&s.proxy_http_addr)
            .bind(&s.proxy_https_addr)
            .bind(&s.admin_addr)
            .bind(s.default_lb.as_str())
            .bind(active_cert)
            .execute(&mut *tx)
            .await?;
        }

        if let Some(acme) = &doc.acme {
            sqlx::query("UPDATE settings SET cloudflare_api_token=? WHERE id=1")
                .bind(&acme.cloudflare_api_token)
                .execute(&mut *tx)
                .await?;
            for account in &acme.accounts {
                sqlx::query(
                    "INSERT INTO acme_accounts \
                     (directory_url, email, credentials) VALUES (?,?,?)",
                )
                .bind(&account.directory_url)
                .bind(&account.email)
                .bind(&account.credentials)
                .execute(&mut *tx)
                .await?;
            }
        }

        tx.commit().await?;
        Ok(report)
    }

    /// Build a fresh immutable [`ProxyConfig`] snapshot from the current DB state.
    /// Each call stamps a new monotonic version.
    pub async fn build_snapshot(&self) -> Result<ProxyConfig, StoreError> {
        let services = self.list_services().await?;
        let targets = self.list_all_targets().await?;
        let routes = self.list_routes().await?;
        let stream_routes = self.list_stream_routes().await?;
        let plugins = self.list_plugins().await?;
        let consumers = self.list_consumers().await?;
        let creds = self.list_all_credentials().await?;
        let settings = self.get_settings().await?;
        let wasm_modules = self
            .list_wasm_modules()
            .await?
            .into_iter()
            .map(|m| (m.name, std::sync::Arc::new(m.wasm)))
            .collect();

        let services = services.into_iter().map(|s| (s.id, s)).collect();

        let mut targets_map: HashMap<i64, Vec<Target>> = HashMap::new();
        for t in targets.into_iter().filter(|t| t.enabled) {
            targets_map.entry(t.service_id).or_default().push(t);
        }

        let routes = routes.into_iter().filter(|r| r.enabled).collect();
        let stream_routes = stream_routes.into_iter().filter(|r| r.enabled).collect();
        let plugins = plugins.into_iter().filter(|p| p.enabled).collect();
        let consumers = consumers.into_iter().map(|c| (c.id, c)).collect();

        let mut key_index = HashMap::new();
        let mut basic_index = HashMap::new();
        let mut jwt_index = HashMap::new();
        for cr in creds {
            match cr.credential_type {
                CredentialType::KeyAuth => {
                    key_index.insert(cr.identifier, cr.consumer_id);
                }
                CredentialType::BasicAuth => {
                    if let Some(secret) = cr.secret {
                        basic_index.insert(cr.identifier, (cr.consumer_id, secret));
                    }
                }
                CredentialType::Jwt => {
                    // secret column holds {"algorithm": "...", "secret": "..."}.
                    let Some(blob) = cr.secret else { continue };
                    let Ok(v) = serde_json::from_str::<serde_json::Value>(&blob) else {
                        continue;
                    };
                    let algorithm = v["algorithm"].as_str().unwrap_or("HS256").to_string();
                    let Some(secret) = v["secret"].as_str() else {
                        continue;
                    };
                    jwt_index.insert(
                        cr.identifier,
                        JwtCred {
                            consumer_id: cr.consumer_id,
                            algorithm,
                            secret: secret.to_string(),
                        },
                    );
                }
            }
        }

        let version = self.version.fetch_add(1, Ordering::Relaxed) + 1;

        let mut cfg = ProxyConfig {
            version,
            routes,
            stream_routes,
            services,
            targets: targets_map,
            plugins,
            consumers,
            key_index,
            basic_index,
            jwt_index,
            wasm_modules,
            path_regexes: Default::default(),
            settings,
        };
        cfg.compile_path_regexes();
        Ok(cfg)
    }
}
