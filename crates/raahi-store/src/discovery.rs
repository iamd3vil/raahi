use std::collections::HashSet;

use chrono::{DateTime, Duration, Utc};
use raahi_core::{
    DiscoveredEndpoint, DiscoverySource, DiscoverySourceSpec, DiscoverySourceStatus,
    DiscoveryState, Id,
};
use sqlx::Row;

use crate::rows::{map_discovery_source, parse_dt};
use crate::{Store, StoreError};

#[derive(Debug, Clone, Default)]
pub struct ReconcileResult {
    pub changed: bool,
    pub added: u64,
    pub updated: u64,
    pub draining: u64,
    pub removed: u64,
}

impl Store {
    pub async fn list_discovery_sources(&self) -> Result<Vec<DiscoverySource>, StoreError> {
        let rows = sqlx::query("SELECT * FROM discovery_sources ORDER BY id")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(map_discovery_source).collect())
    }

    pub async fn list_discovery_sources_for(
        &self,
        service_id: Id,
    ) -> Result<Vec<DiscoverySource>, StoreError> {
        let rows = sqlx::query("SELECT * FROM discovery_sources WHERE service_id = ? ORDER BY id")
            .bind(service_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(map_discovery_source).collect())
    }

    pub async fn get_discovery_source(
        &self,
        id: Id,
    ) -> Result<Option<DiscoverySource>, StoreError> {
        let row = sqlx::query("SELECT * FROM discovery_sources WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().map(map_discovery_source))
    }

    pub async fn create_discovery_source(
        &self,
        service_id: Id,
        source: &DiscoverySourceSpec,
    ) -> Result<DiscoverySource, StoreError> {
        if self.get_service(service_id).await?.is_none() {
            return Err(StoreError::NotFound);
        }
        let now = Utc::now().to_rfc3339();
        let mut tx = self.pool.begin().await?;
        let id = sqlx::query(
            "INSERT INTO discovery_sources
             (service_id, name, provider, config, enabled, stale_after_ms, removal_grace_ms, created_at, updated_at)
             VALUES (?,?,?,?,?,?,?,?,?)",
        )
        .bind(service_id)
        .bind(source.name.trim())
        .bind(&source.provider)
        .bind(source.config.to_string())
        .bind(source.enabled as i64)
        .bind(source.stale_after_ms as i64)
        .bind(source.removal_grace_ms as i64)
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await
        .map_err(super::crud::map_err)?
        .last_insert_rowid();
        sqlx::query("INSERT INTO discovery_source_status (source_id, state) VALUES (?,?)")
            .bind(id)
            .bind(if source.enabled {
                "pending"
            } else {
                "disabled"
            })
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(self.get_discovery_source(id).await?.unwrap())
    }

    pub async fn update_discovery_source(
        &self,
        id: Id,
        source: &DiscoverySourceSpec,
    ) -> Result<Option<DiscoverySource>, StoreError> {
        let old = self.get_discovery_source(id).await?;
        let now = Utc::now().to_rfc3339();
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE discovery_sources SET name=?, provider=?, config=?, enabled=?,
             stale_after_ms=?, removal_grace_ms=?, updated_at=? WHERE id=?",
        )
        .bind(source.name.trim())
        .bind(&source.provider)
        .bind(source.config.to_string())
        .bind(source.enabled as i64)
        .bind(source.stale_after_ms as i64)
        .bind(source.removal_grace_ms as i64)
        .bind(&now)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(super::crud::map_err)?;
        if result.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(None);
        }
        sqlx::query(
            "UPDATE discovery_source_status SET state=?, next_refresh_at=NULL WHERE source_id=?",
        )
        .bind(if source.enabled {
            "pending"
        } else {
            "disabled"
        })
        .bind(id)
        .execute(&mut *tx)
        .await?;
        if let Some(old) = old
            && old.provider != source.provider
        {
            sqlx::query("DELETE FROM targets WHERE source_id=?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        if !source.enabled {
            sqlx::query(
                "UPDATE targets SET state='draining', missing_since=COALESCE(missing_since, ?)
                 WHERE source_id=? AND state!='draining'",
            )
            .bind(&now)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        self.get_discovery_source(id).await
    }

    pub async fn delete_discovery_source(&self, id: Id) -> Result<bool, StoreError> {
        let result = sqlx::query("DELETE FROM discovery_sources WHERE id=?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn expire_draining_targets(&self) -> Result<bool, StoreError> {
        let now = Utc::now();
        let rows = sqlx::query(
            "SELECT t.id, t.missing_since, s.removal_grace_ms
             FROM targets t JOIN discovery_sources s ON s.id=t.source_id
             WHERE t.state='draining' AND t.missing_since IS NOT NULL",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut ids = Vec::new();
        for row in rows {
            let missing = parse_dt(&row.get::<String, _>("missing_since"));
            let grace = row.get::<i64, _>("removal_grace_ms");
            if grace <= 0 || now - missing >= Duration::milliseconds(grace) {
                ids.push(row.get::<Id, _>("id"));
            }
        }
        if ids.is_empty() {
            return Ok(false);
        }
        let mut tx = self.pool.begin().await?;
        for id in ids {
            sqlx::query("DELETE FROM targets WHERE id=?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    pub async fn get_discovery_status(
        &self,
        source_id: Id,
    ) -> Result<Option<DiscoverySourceStatus>, StoreError> {
        let Some(row) = sqlx::query(
            "SELECT s.source_id, s.state, s.last_attempt_at, s.last_success_at,
                    s.next_refresh_at, s.revision, s.endpoint_count, s.last_error,
                    SUM(CASE WHEN t.state='active' THEN 1 ELSE 0 END) active_count,
                    SUM(CASE WHEN t.state='draining' THEN 1 ELSE 0 END) draining_count,
                    SUM(CASE WHEN t.state='stale' THEN 1 ELSE 0 END) stale_count
             FROM discovery_source_status s
             LEFT JOIN targets t ON t.source_id=s.source_id
             WHERE s.source_id=? GROUP BY s.source_id",
        )
        .bind(source_id)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };
        let dt = |name: &str| row.get::<Option<String>, _>(name).as_deref().map(parse_dt);
        Ok(Some(DiscoverySourceStatus {
            source_id: row.get("source_id"),
            state: DiscoveryState::parse(&row.get::<String, _>("state")).unwrap_or_default(),
            last_attempt_at: dt("last_attempt_at"),
            last_success_at: dt("last_success_at"),
            next_refresh_at: dt("next_refresh_at"),
            revision: row.get("revision"),
            endpoint_count: row.get::<i64, _>("endpoint_count") as u64,
            active_count: row.get::<i64, _>("active_count") as u64,
            draining_count: row.get::<i64, _>("draining_count") as u64,
            stale_count: row.get::<i64, _>("stale_count") as u64,
            last_error: row.get("last_error"),
        }))
    }

    pub async fn mark_discovery_attempt(
        &self,
        source_id: Id,
        next_refresh_at: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE discovery_source_status SET last_attempt_at=?, next_refresh_at=? WHERE source_id=?",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(next_refresh_at.to_rfc3339())
        .bind(source_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn record_discovery_failure(
        &self,
        source: &DiscoverySource,
        error: &str,
        next_refresh_at: DateTime<Utc>,
    ) -> Result<bool, StoreError> {
        let now = Utc::now();
        let previous: Option<String> = sqlx::query_scalar(
            "SELECT last_success_at FROM discovery_source_status WHERE source_id=?",
        )
        .bind(source.id)
        .fetch_optional(&self.pool)
        .await?
        .flatten();
        let stale = previous
            .as_deref()
            .map(parse_dt)
            .is_none_or(|last| now - last >= Duration::milliseconds(source.stale_after_ms as i64));
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE discovery_source_status SET state=?, last_attempt_at=?, next_refresh_at=?, last_error=? WHERE source_id=?",
        )
        .bind(if stale { "stale" } else { "failing" })
        .bind(now.to_rfc3339())
        .bind(next_refresh_at.to_rfc3339())
        .bind(error)
        .bind(source.id)
        .execute(&mut *tx)
        .await?;
        let changed = if stale {
            sqlx::query(
                "UPDATE targets SET state='stale', enabled=0 WHERE source_id=? AND state!='stale'",
            )
            .bind(source.id)
            .execute(&mut *tx)
            .await?
            .rows_affected()
                > 0
        } else {
            false
        };
        tx.commit().await?;
        Ok(changed)
    }

    pub async fn reconcile_discovery(
        &self,
        source: &DiscoverySource,
        endpoints: &[DiscoveredEndpoint],
        revision: &str,
        next_refresh_at: DateTime<Utc>,
    ) -> Result<ReconcileResult, StoreError> {
        let now = Utc::now();
        let now_text = now.to_rfc3339();
        let mut seen = HashSet::new();
        for endpoint in endpoints {
            if endpoint.key.trim().is_empty()
                || endpoint.host.trim().is_empty()
                || endpoint.port == 0
            {
                return Err(StoreError::Invalid(
                    "provider returned an invalid endpoint".into(),
                ));
            }
            if !seen.insert(endpoint.key.clone()) {
                return Err(StoreError::Invalid(format!(
                    "provider returned duplicate endpoint key '{}'",
                    endpoint.key
                )));
            }
        }

        let mut result = ReconcileResult::default();
        let mut tx = self.pool.begin().await?;
        for endpoint in endpoints {
            let existing = sqlx::query(
                "SELECT id, host, port, weight, priority, state, metadata FROM targets
                 WHERE source_id=? AND provider_key=?",
            )
            .bind(source.id)
            .bind(&endpoint.key)
            .fetch_optional(&mut *tx)
            .await?;
            let metadata =
                serde_json::to_string(&endpoint.metadata).unwrap_or_else(|_| "{}".into());
            if let Some(row) = existing {
                let differs = row.get::<String, _>("host") != endpoint.host
                    || row.get::<i64, _>("port") != i64::from(endpoint.port)
                    || row.get::<i64, _>("weight") != i64::from(endpoint.weight)
                    || row.get::<i64, _>("priority") != i64::from(endpoint.priority)
                    || row.get::<String, _>("state") != "active"
                    || row.get::<String, _>("metadata") != metadata;
                sqlx::query(
                    "UPDATE targets SET host=?, port=?, weight=?, priority=?, enabled=1,
                     state='active', metadata=?, last_seen_at=?, missing_since=NULL WHERE id=?",
                )
                .bind(&endpoint.host)
                .bind(i64::from(endpoint.port))
                .bind(i64::from(endpoint.weight))
                .bind(i64::from(endpoint.priority))
                .bind(&metadata)
                .bind(&now_text)
                .bind(row.get::<Id, _>("id"))
                .execute(&mut *tx)
                .await?;
                if differs {
                    result.updated += 1;
                }
            } else {
                sqlx::query(
                    "INSERT INTO targets
                     (service_id, host, port, weight, priority, enabled, source_id, provider_key,
                      state, metadata, last_seen_at)
                     VALUES (?,?,?,?,?,1,?,?, 'active',?,?)",
                )
                .bind(source.service_id)
                .bind(&endpoint.host)
                .bind(i64::from(endpoint.port))
                .bind(i64::from(endpoint.weight))
                .bind(i64::from(endpoint.priority))
                .bind(source.id)
                .bind(&endpoint.key)
                .bind(&metadata)
                .bind(&now_text)
                .execute(&mut *tx)
                .await?;
                result.added += 1;
            }
        }

        let keys: Vec<String> = seen.into_iter().collect();
        let rows = sqlx::query(
            "SELECT id, provider_key, state, missing_since FROM targets WHERE source_id=?",
        )
        .bind(source.id)
        .fetch_all(&mut *tx)
        .await?;
        for row in rows {
            let key: String = row.get("provider_key");
            if keys.contains(&key) {
                continue;
            }
            let id: Id = row.get("id");
            let missing = row
                .get::<Option<String>, _>("missing_since")
                .as_deref()
                .map(parse_dt);
            if source.removal_grace_ms == 0
                || missing.is_some_and(|at| {
                    now - at >= Duration::milliseconds(source.removal_grace_ms as i64)
                })
            {
                sqlx::query("DELETE FROM targets WHERE id=?")
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                result.removed += 1;
            } else if row.get::<String, _>("state") != "draining" {
                sqlx::query("UPDATE targets SET state='draining', missing_since=? WHERE id=?")
                    .bind(&now_text)
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                result.draining += 1;
            }
        }

        sqlx::query(
            "UPDATE discovery_source_status SET state='healthy', last_attempt_at=?,
             last_success_at=?, next_refresh_at=?, revision=?, endpoint_count=?, last_error=NULL
             WHERE source_id=?",
        )
        .bind(&now_text)
        .bind(&now_text)
        .bind(next_refresh_at.to_rfc3339())
        .bind(revision)
        .bind(endpoints.len() as i64)
        .bind(source.id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        result.changed = result.added + result.updated + result.draining + result.removed > 0;
        Ok(result)
    }
}
