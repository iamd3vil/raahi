//! CRUD operations for every config entity. Updates are full replacements (PUT
//! semantics). All queries are parameterized.

use chrono::Utc;
use raahi_core::*;
use sqlx::Row;

use crate::rows::*;
use crate::{Store, StoreError};

/// Map insert/update DB errors to friendlier store errors.
fn map_err(e: sqlx::Error) -> StoreError {
    if let Some(db) = e.as_database_error() {
        if db.is_unique_violation() {
            return StoreError::Conflict(db.message().to_string());
        }
        if db.is_foreign_key_violation() {
            return StoreError::Invalid(format!("foreign key violation: {}", db.message()));
        }
    }
    StoreError::Db(e)
}

impl Store {
    // ---- services -------------------------------------------------------------
    pub async fn list_services(&self) -> Result<Vec<Service>, StoreError> {
        let rows = sqlx::query("SELECT * FROM services ORDER BY id")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(map_service).collect())
    }

    pub async fn get_service(&self, id: Id) -> Result<Option<Service>, StoreError> {
        let row = sqlx::query("SELECT * FROM services WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().map(map_service))
    }

    pub async fn create_service(&self, s: &ServiceSpec) -> Result<Service, StoreError> {
        let now = Utc::now().to_rfc3339();
        let res = sqlx::query(
            "INSERT INTO services (name, protocol, connect_timeout_ms, read_timeout_ms, \
             write_timeout_ms, retries, lb_algorithm, tls_sni, health_path, created_at, updated_at) \
             VALUES (?,?,?,?,?,?,?,?,?,?,?)",
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
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(map_err)?;
        Ok(self.get_service(res.last_insert_rowid()).await?.unwrap())
    }

    pub async fn update_service(
        &self,
        id: Id,
        s: &ServiceSpec,
    ) -> Result<Option<Service>, StoreError> {
        let now = Utc::now().to_rfc3339();
        let res = sqlx::query(
            "UPDATE services SET name=?, protocol=?, connect_timeout_ms=?, read_timeout_ms=?, \
             write_timeout_ms=?, retries=?, lb_algorithm=?, tls_sni=?, health_path=?, updated_at=? WHERE id=?",
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
        .bind(&now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(map_err)?;
        if res.rows_affected() == 0 {
            return Ok(None);
        }
        self.get_service(id).await
    }

    pub async fn delete_service(&self, id: Id) -> Result<bool, StoreError> {
        let res = sqlx::query("DELETE FROM services WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    // ---- targets --------------------------------------------------------------
    pub async fn list_all_targets(&self) -> Result<Vec<Target>, StoreError> {
        let rows = sqlx::query("SELECT * FROM targets ORDER BY id")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(map_target).collect())
    }

    pub async fn list_targets_for(&self, service_id: Id) -> Result<Vec<Target>, StoreError> {
        let rows = sqlx::query("SELECT * FROM targets WHERE service_id = ? ORDER BY id")
            .bind(service_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(map_target).collect())
    }

    pub async fn get_target(&self, id: Id) -> Result<Option<Target>, StoreError> {
        let row = sqlx::query("SELECT * FROM targets WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().map(map_target))
    }

    pub async fn create_target(
        &self,
        service_id: Id,
        t: &TargetSpec,
    ) -> Result<Target, StoreError> {
        let res = sqlx::query(
            "INSERT INTO targets (service_id, host, port, weight, enabled) VALUES (?,?,?,?,?)",
        )
        .bind(service_id)
        .bind(&t.host)
        .bind(t.port as i64)
        .bind(t.weight as i64)
        .bind(t.enabled as i64)
        .execute(&self.pool)
        .await
        .map_err(map_err)?;
        Ok(self.get_target(res.last_insert_rowid()).await?.unwrap())
    }

    pub async fn update_target(
        &self,
        id: Id,
        t: &TargetSpec,
    ) -> Result<Option<Target>, StoreError> {
        let res = sqlx::query(
            "UPDATE targets SET host=?, port=?, weight=?, enabled=? WHERE id=?",
        )
        .bind(&t.host)
        .bind(t.port as i64)
        .bind(t.weight as i64)
        .bind(t.enabled as i64)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(map_err)?;
        if res.rows_affected() == 0 {
            return Ok(None);
        }
        self.get_target(id).await
    }

    pub async fn delete_target(&self, id: Id) -> Result<bool, StoreError> {
        let res = sqlx::query("DELETE FROM targets WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    // ---- routes ---------------------------------------------------------------
    pub async fn list_routes(&self) -> Result<Vec<Route>, StoreError> {
        let rows = sqlx::query("SELECT * FROM routes ORDER BY id")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(map_route).collect())
    }

    pub async fn get_route(&self, id: Id) -> Result<Option<Route>, StoreError> {
        let row = sqlx::query("SELECT * FROM routes WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().map(map_route))
    }

    pub async fn create_route(&self, r: &RouteSpec) -> Result<Route, StoreError> {
        let res = sqlx::query(
            "INSERT INTO routes (name, service_id, priority, hosts, paths, methods, \
             headers, splits, strip_path, preserve_host, enabled) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(&r.name)
        .bind(r.service_id)
        .bind(r.priority as i64)
        .bind(serde_json::to_string(&r.hosts).unwrap())
        .bind(serde_json::to_string(&r.paths).unwrap())
        .bind(serde_json::to_string(&r.methods).unwrap())
        .bind(serde_json::to_string(&r.headers).unwrap())
        .bind(serde_json::to_string(&r.splits).unwrap())
        .bind(r.strip_path as i64)
        .bind(r.preserve_host as i64)
        .bind(r.enabled as i64)
        .execute(&self.pool)
        .await
        .map_err(map_err)?;
        Ok(self.get_route(res.last_insert_rowid()).await?.unwrap())
    }

    pub async fn update_route(&self, id: Id, r: &RouteSpec) -> Result<Option<Route>, StoreError> {
        let res = sqlx::query(
            "UPDATE routes SET name=?, service_id=?, priority=?, hosts=?, paths=?, methods=?, \
             headers=?, splits=?, strip_path=?, preserve_host=?, enabled=? WHERE id=?",
        )
        .bind(&r.name)
        .bind(r.service_id)
        .bind(r.priority as i64)
        .bind(serde_json::to_string(&r.hosts).unwrap())
        .bind(serde_json::to_string(&r.paths).unwrap())
        .bind(serde_json::to_string(&r.methods).unwrap())
        .bind(serde_json::to_string(&r.headers).unwrap())
        .bind(serde_json::to_string(&r.splits).unwrap())
        .bind(r.strip_path as i64)
        .bind(r.preserve_host as i64)
        .bind(r.enabled as i64)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(map_err)?;
        if res.rows_affected() == 0 {
            return Ok(None);
        }
        self.get_route(id).await
    }

    pub async fn delete_route(&self, id: Id) -> Result<bool, StoreError> {
        let res = sqlx::query("DELETE FROM routes WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    // ---- plugins --------------------------------------------------------------
    pub async fn list_plugins(&self) -> Result<Vec<Plugin>, StoreError> {
        let rows = sqlx::query("SELECT * FROM plugins ORDER BY ordering, id")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(map_plugin).collect())
    }

    pub async fn get_plugin(&self, id: Id) -> Result<Option<Plugin>, StoreError> {
        let row = sqlx::query("SELECT * FROM plugins WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().map(map_plugin))
    }

    pub async fn create_plugin(&self, p: &PluginSpec) -> Result<Plugin, StoreError> {
        let res = sqlx::query(
            "INSERT INTO plugins (type, scope, service_id, route_id, config, ordering, enabled) \
             VALUES (?,?,?,?,?,?,?)",
        )
        .bind(p.plugin_type.as_str())
        .bind(p.scope.as_str())
        .bind(p.service_id)
        .bind(p.route_id)
        .bind(serde_json::to_string(&p.config).unwrap())
        .bind(p.ordering as i64)
        .bind(p.enabled as i64)
        .execute(&self.pool)
        .await
        .map_err(map_err)?;
        Ok(self.get_plugin(res.last_insert_rowid()).await?.unwrap())
    }

    pub async fn update_plugin(&self, id: Id, p: &PluginSpec) -> Result<Option<Plugin>, StoreError> {
        let res = sqlx::query(
            "UPDATE plugins SET type=?, scope=?, service_id=?, route_id=?, config=?, \
             ordering=?, enabled=? WHERE id=?",
        )
        .bind(p.plugin_type.as_str())
        .bind(p.scope.as_str())
        .bind(p.service_id)
        .bind(p.route_id)
        .bind(serde_json::to_string(&p.config).unwrap())
        .bind(p.ordering as i64)
        .bind(p.enabled as i64)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(map_err)?;
        if res.rows_affected() == 0 {
            return Ok(None);
        }
        self.get_plugin(id).await
    }

    pub async fn delete_plugin(&self, id: Id) -> Result<bool, StoreError> {
        let res = sqlx::query("DELETE FROM plugins WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    // ---- consumers ------------------------------------------------------------
    pub async fn list_consumers(&self) -> Result<Vec<Consumer>, StoreError> {
        let rows = sqlx::query("SELECT * FROM consumers ORDER BY id")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(map_consumer).collect())
    }

    pub async fn get_consumer(&self, id: Id) -> Result<Option<Consumer>, StoreError> {
        let row = sqlx::query("SELECT * FROM consumers WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().map(map_consumer))
    }

    pub async fn create_consumer(&self, c: &ConsumerSpec) -> Result<Consumer, StoreError> {
        let res = sqlx::query("INSERT INTO consumers (username, groups) VALUES (?, ?)")
            .bind(&c.username)
            .bind(serde_json::to_string(&c.groups).unwrap_or_else(|_| "[]".into()))
            .execute(&self.pool)
            .await
            .map_err(map_err)?;
        Ok(self.get_consumer(res.last_insert_rowid()).await?.unwrap())
    }

    pub async fn update_consumer(
        &self,
        id: Id,
        c: &ConsumerSpec,
    ) -> Result<Option<Consumer>, StoreError> {
        let res = sqlx::query("UPDATE consumers SET username=?, groups=? WHERE id=?")
            .bind(&c.username)
            .bind(serde_json::to_string(&c.groups).unwrap_or_else(|_| "[]".into()))
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(map_err)?;
        if res.rows_affected() == 0 {
            return Ok(None);
        }
        self.get_consumer(id).await
    }

    pub async fn delete_consumer(&self, id: Id) -> Result<bool, StoreError> {
        let res = sqlx::query("DELETE FROM consumers WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    // ---- credentials ----------------------------------------------------------
    pub async fn list_all_credentials(&self) -> Result<Vec<ConsumerCredential>, StoreError> {
        let rows = sqlx::query("SELECT * FROM consumer_credentials ORDER BY id")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(map_credential).collect())
    }

    pub async fn list_credentials_for(
        &self,
        consumer_id: Id,
    ) -> Result<Vec<ConsumerCredential>, StoreError> {
        let rows = sqlx::query("SELECT * FROM consumer_credentials WHERE consumer_id = ? ORDER BY id")
            .bind(consumer_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(map_credential).collect())
    }

    pub async fn get_credential(&self, id: Id) -> Result<Option<ConsumerCredential>, StoreError> {
        let row = sqlx::query("SELECT * FROM consumer_credentials WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().map(map_credential))
    }

    /// `secret` should already be hashed (basic-auth) or `None` (key-auth).
    pub async fn create_credential(
        &self,
        consumer_id: Id,
        credential_type: CredentialType,
        identifier: &str,
        secret: Option<&str>,
    ) -> Result<ConsumerCredential, StoreError> {
        let res = sqlx::query(
            "INSERT INTO consumer_credentials (consumer_id, type, identifier, secret) \
             VALUES (?,?,?,?)",
        )
        .bind(consumer_id)
        .bind(credential_type.as_str())
        .bind(identifier)
        .bind(secret)
        .execute(&self.pool)
        .await
        .map_err(map_err)?;
        Ok(self.get_credential(res.last_insert_rowid()).await?.unwrap())
    }

    pub async fn delete_credential(&self, id: Id) -> Result<bool, StoreError> {
        let res = sqlx::query("DELETE FROM consumer_credentials WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    // ---- wasm modules -----------------------------------------------------------
    pub async fn list_wasm_modules(&self) -> Result<Vec<WasmModule>, StoreError> {
        let rows = sqlx::query("SELECT * FROM wasm_modules ORDER BY id")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(map_wasm_module).collect())
    }

    pub async fn get_wasm_module(&self, id: Id) -> Result<Option<WasmModule>, StoreError> {
        let row = sqlx::query("SELECT * FROM wasm_modules WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().map(map_wasm_module))
    }

    /// `wasm` must already be validated (compiled) by the caller.
    pub async fn create_wasm_module(
        &self,
        name: &str,
        description: &str,
        wasm: &[u8],
    ) -> Result<WasmModule, StoreError> {
        let res = sqlx::query(
            "INSERT INTO wasm_modules (name, description, wasm, created_at) \
             VALUES (?,?,?,datetime('now'))",
        )
        .bind(name)
        .bind(description)
        .bind(wasm)
        .execute(&self.pool)
        .await
        .map_err(map_err)?;
        Ok(self.get_wasm_module(res.last_insert_rowid()).await?.unwrap())
    }

    pub async fn delete_wasm_module(&self, id: Id) -> Result<bool, StoreError> {
        let res = sqlx::query("DELETE FROM wasm_modules WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    // ---- admin token ------------------------------------------------------------
    /// SHA-256 hex of the admin API token; `None` = auth disabled.
    pub async fn get_admin_token_hash(&self) -> Result<Option<String>, StoreError> {
        let row = sqlx::query("SELECT admin_token_hash FROM settings WHERE id = 1")
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.and_then(|r| r.get("admin_token_hash")))
    }

    pub async fn set_admin_token_hash(&self, hash: Option<&str>) -> Result<(), StoreError> {
        sqlx::query("UPDATE settings SET admin_token_hash = ? WHERE id = 1")
            .bind(hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ---- certificates ---------------------------------------------------------
    pub async fn list_certificates(&self) -> Result<Vec<Certificate>, StoreError> {
        let rows = sqlx::query("SELECT * FROM certificates ORDER BY id")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(map_certificate).collect())
    }

    pub async fn get_certificate(&self, id: Id) -> Result<Option<Certificate>, StoreError> {
        let row = sqlx::query("SELECT * FROM certificates WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().map(map_certificate))
    }

    pub async fn create_certificate(&self, c: &CertificateSpec) -> Result<Certificate, StoreError> {
        let res = sqlx::query(
            "INSERT INTO certificates (name, sni, cert_pem, key_pem) VALUES (?,?,?,?)",
        )
        .bind(&c.name)
        .bind(serde_json::to_string(&c.sni).unwrap())
        .bind(&c.cert_pem)
        .bind(&c.key_pem)
        .execute(&self.pool)
        .await
        .map_err(map_err)?;
        Ok(self.get_certificate(res.last_insert_rowid()).await?.unwrap())
    }

    pub async fn delete_certificate(&self, id: Id) -> Result<bool, StoreError> {
        let res = sqlx::query("DELETE FROM certificates WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    // ---- settings -------------------------------------------------------------
    pub async fn get_settings(&self) -> Result<Settings, StoreError> {
        let row = sqlx::query("SELECT * FROM settings WHERE id = 1")
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().map(map_settings).unwrap_or_default())
    }

    pub async fn update_settings(&self, s: &SettingsSpec) -> Result<Settings, StoreError> {
        sqlx::query(
            "UPDATE settings SET proxy_http_addr=?, proxy_https_addr=?, admin_addr=?, \
             default_lb=?, active_certificate_id=? WHERE id=1",
        )
        .bind(&s.proxy_http_addr)
        .bind(&s.proxy_https_addr)
        .bind(&s.admin_addr)
        .bind(s.default_lb.as_str())
        .bind(s.active_certificate_id)
        .execute(&self.pool)
        .await
        .map_err(map_err)?;
        self.get_settings().await
    }
}
