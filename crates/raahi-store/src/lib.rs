//! Raahi config store: SQLite persistence (via sqlx) and compilation of the in-memory
//! [`ProxyConfig`] snapshot the data plane reads.

mod crud;
mod rows;

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use raahi_core::{CredentialType, ProxyConfig, Target};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;

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

    /// Build a fresh immutable [`ProxyConfig`] snapshot from the current DB state.
    /// Each call stamps a new monotonic version.
    pub async fn build_snapshot(&self) -> Result<ProxyConfig, StoreError> {
        let services = self.list_services().await?;
        let targets = self.list_all_targets().await?;
        let routes = self.list_routes().await?;
        let plugins = self.list_plugins().await?;
        let consumers = self.list_consumers().await?;
        let creds = self.list_all_credentials().await?;
        let settings = self.get_settings().await?;

        let services = services.into_iter().map(|s| (s.id, s)).collect();

        let mut targets_map: HashMap<i64, Vec<Target>> = HashMap::new();
        for t in targets.into_iter().filter(|t| t.enabled) {
            targets_map.entry(t.service_id).or_default().push(t);
        }

        let routes = routes.into_iter().filter(|r| r.enabled).collect();
        let plugins = plugins.into_iter().filter(|p| p.enabled).collect();
        let consumers = consumers.into_iter().map(|c| (c.id, c)).collect();

        let mut key_index = HashMap::new();
        let mut basic_index = HashMap::new();
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
            }
        }

        let version = self.version.fetch_add(1, Ordering::Relaxed) + 1;

        Ok(ProxyConfig {
            version,
            routes,
            services,
            targets: targets_map,
            plugins,
            consumers,
            key_index,
            basic_index,
            settings,
        })
    }
}
