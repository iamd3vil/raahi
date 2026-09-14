use crate::{Store, StoreError, rows::map_certificate, rows::map_route};
use raahi_core::*;
use serde_json::json;

impl Store {
    /// Create all gateway resources together. The first INSERT obtains SQLite's
    /// write lock before conflict checks, serializing concurrent wizard submissions.
    pub async fn create_application(
        &self,
        input: &ApplicationSpec,
    ) -> Result<ApplicationCreated, StoreError> {
        let spec = input.normalized().map_err(StoreError::Invalid)?;
        let upstream =
            parse_application_upstream(&spec.upstream_url).map_err(StoreError::Invalid)?;
        let mut tx = self.pool.begin().await?;
        let service_id = sqlx::query("INSERT INTO services (name, protocol, upstream_authority, tls_sni, created_at, updated_at) VALUES (?,?,?,?,datetime('now'),datetime('now'))")
            .bind(&spec.name).bind(upstream.protocol.as_str())
            .bind(Some(&upstream.host))
            .bind(upstream.protocol.is_tls().then_some(&upstream.host))
            .execute(&mut *tx).await.map_err(super::crud::map_err)?.last_insert_rowid();

        let routes = sqlx::query("SELECT * FROM routes")
            .fetch_all(&mut *tx)
            .await?;
        if routes.iter().map(map_route).any(|r| {
            r.hosts
                .iter()
                .any(|h| h.trim_end_matches('.').eq_ignore_ascii_case(&spec.domain))
        }) {
            return Err(StoreError::Conflict(format!(
                "A route already uses {}. Edit it in Routes instead.",
                spec.domain
            )));
        }
        if let Some(overlap) = routes.iter().map(map_route).find(|r| {
            r.enabled
                && r.priority >= 100
                && (r.hosts.is_empty() || r.hosts.iter().any(|h| host_matches(h, &spec.domain)))
        }) {
            return Err(StoreError::Conflict(format!(
                "Route '{}' could take precedence for {}. Adjust its priority or host match in Routes first.",
                overlap.name, spec.domain
            )));
        }
        if spec.https {
            let listener: Option<String> =
                sqlx::query_scalar("SELECT proxy_https_addr FROM settings WHERE id=1")
                    .fetch_one(&mut *tx)
                    .await?;
            if listener.as_deref().is_none_or(str::is_empty) {
                return Err(StoreError::Invalid(
                    "Enable the HTTPS listener in Settings before adding an HTTPS application"
                        .into(),
                ));
            }
            let certs = sqlx::query("SELECT * FROM certificates")
                .fetch_all(&mut *tx)
                .await?;
            if !certs.iter().map(map_certificate).any(|c| {
                !c.cert_pem.is_empty()
                    && !c.key_pem.is_empty()
                    && c.sni.iter().any(|h| host_matches(h, &spec.domain))
            }) {
                return Err(StoreError::Invalid(format!(
                    "Add or issue a certificate covering {} in Certificates first",
                    spec.domain
                )));
            }
        }
        let target_id = sqlx::query("INSERT INTO targets (service_id, host, port) VALUES (?,?,?)")
            .bind(service_id)
            .bind(&upstream.host)
            .bind(i64::from(upstream.port))
            .execute(&mut *tx)
            .await?
            .last_insert_rowid();
        let route_id = sqlx::query("INSERT INTO routes (name, service_id, priority, hosts, paths, strip_path, preserve_host) VALUES (?,?,100,?,'[\"/\"]',0,1)")
            .bind(&spec.domain).bind(service_id).bind(json!([spec.domain]).to_string())
            .execute(&mut *tx).await.map_err(super::crud::map_err)?.last_insert_rowid();
        let mut plugins = Vec::new();
        if spec.https {
            plugins.push(("redirect", json!({"status":308, "location":format!("https://{}",spec.domain), "preserve_path":true, "http_only":true})));
        }
        if spec.hsts {
            plugins.push((
                "hsts",
                json!({"max_age_secs":31536000, "include_subdomains":false, "preload":false}),
            ));
        }
        if !spec.allowed_cidrs.is_empty() {
            plugins.push((
                "ip-restriction",
                json!({"allow":spec.allowed_cidrs, "deny":[]}),
            ));
        }
        let mut plugin_ids = Vec::new();
        for (kind, config) in plugins {
            let id = sqlx::query(
                "INSERT INTO plugins (type, scope, route_id, config) VALUES (?,'route',?,?)",
            )
            .bind(kind)
            .bind(route_id)
            .bind(config.to_string())
            .execute(&mut *tx)
            .await?
            .last_insert_rowid();
            plugin_ids.push(id);
        }
        tx.commit().await?;
        Ok(ApplicationCreated {
            name: spec.name,
            url: format!(
                "{}://{}",
                if spec.https { "https" } else { "http" },
                spec.domain
            ),
            service_id,
            target_id,
            route_id,
            plugin_ids,
        })
    }
}
