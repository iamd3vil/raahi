//! Raahi control plane: the admin REST API + UI static serving, exposed as a Pingora
//! [`BackgroundService`] so it runs in-process alongside the data plane. Every mutation
//! writes to SQLite, then rebuilds and hot-swaps the proxy's config snapshot.

mod acme_accounts;
mod applications;
mod auth;
mod error;
mod handlers;
mod openapi;
mod sso;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::routing::get;
use pingora::server::ShutdownWatch;
use pingora::services::background::BackgroundService;
use raahi_acme::AcmeHandle;
use raahi_proxy::{CertHandle, CertStore, ConfigHandle, Metrics};
use raahi_store::Store;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
use tracing::{error, info};

pub use auth::{AuthMethod, AuthState, Principal, required_role};
pub use error::{ApiError, ApiResult};

/// Shared state for all handlers. Cheap to clone.
#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub config: ConfigHandle,
    pub metrics: Arc<Metrics>,
    pub cert_handle: CertHandle,
    pub acme_handle: AcmeHandle,
    pub ui_dir: Option<PathBuf>,
    /// Live auth configuration (admin token hash, user presence, SSO settings).
    pub auth: Arc<AuthState>,
}

/// Rebuild the config snapshot from the store and atomically swap it into the proxy.
pub async fn reload(state: &AppState) -> ApiResult<()> {
    let snap = state.store.build_snapshot().await?;
    state.config.store(snap);
    Ok(())
}

/// Rebuild the in-memory certificate store from the DB and swap it into the live TLS
/// handle — applies certificate add/remove/replace and default changes with no restart
/// (provided the HTTPS listener is already bound).
pub async fn reload_certs(state: &AppState) -> ApiResult<()> {
    let certs = state.store.list_certificates().await?;
    let settings = state.store.get_settings().await?;
    state
        .cert_handle
        .store(CertStore::from_certs(certs, settings.active_certificate_id));
    Ok(())
}

/// Constant-time-ish equality for two hex digests of equal length.
pub(crate) fn digest_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// SHA-256 hex digest (admin and session tokens are high-entropy, so a fast hash
/// is fine).
pub(crate) fn sha256_hex(data: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(data.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Assemble the full admin router (API under `/api/v1`, optional SPA static serving).
pub fn build_router(state: AppState) -> Router {
    use handlers::*;

    let api = Router::new()
        .route(
            "/acme/eab",
            get(acme_accounts::status)
                .put(acme_accounts::save)
                .delete(acme_accounts::remove),
        )
        .route("/applications", axum::routing::post(applications::create))
        .route(
            "/applications/test-upstream",
            axum::routing::post(applications::test_upstream),
        )
        .route("/services", get(list_services).post(create_service))
        .route(
            "/services/{id}",
            get(get_service).put(update_service).delete(delete_service),
        )
        .route(
            "/services/{id}/targets",
            get(list_targets).post(create_target),
        )
        .route(
            "/targets/{id}",
            axum::routing::put(update_target).delete(delete_target),
        )
        .route("/routes", get(list_routes).post(create_route))
        .route(
            "/routes/{id}",
            get(get_route).put(update_route).delete(delete_route),
        )
        .route(
            "/stream-routes",
            get(list_stream_routes).post(create_stream_route),
        )
        .route(
            "/stream-routes/{id}",
            axum::routing::put(update_stream_route).delete(delete_stream_route),
        )
        .route("/plugins", get(list_plugins).post(create_plugin))
        .route(
            "/plugins/{id}",
            get(get_plugin).put(update_plugin).delete(delete_plugin),
        )
        .route("/cache/purge", axum::routing::post(purge_cache))
        .route("/consumers", get(list_consumers).post(create_consumer))
        .route(
            "/consumers/{id}",
            get(get_consumer)
                .put(update_consumer)
                .delete(delete_consumer),
        )
        .route(
            "/consumers/{id}/credentials",
            get(list_credentials).post(create_credential),
        )
        .route(
            "/credentials/{id}",
            axum::routing::delete(delete_credential),
        )
        .route(
            "/certificates",
            get(list_certificates).post(create_certificate),
        )
        .route(
            "/certificates/{id}",
            axum::routing::delete(delete_certificate),
        )
        .route(
            "/certificates/{id}/renew",
            axum::routing::post(renew_certificate),
        )
        .route(
            "/acme/cloudflare-token",
            get(get_cloudflare_token_status)
                .put(set_cloudflare_token)
                .delete(delete_cloudflare_token),
        )
        .route("/settings", get(get_settings).put(update_settings))
        .route(
            "/wasm-modules",
            get(list_wasm_modules).post(create_wasm_module),
        )
        .route(
            "/wasm-modules/{id}",
            axum::routing::delete(delete_wasm_module),
        )
        .route("/metrics", get(metrics))
        .route("/requests", get(requests))
        .route("/events", get(events))
        .route("/config", get(config_summary))
        .route("/health", get(target_health))
        .route("/router/test", get(router_test))
        .route("/export", get(export_config))
        .route("/import", axum::routing::post(import_config))
        .route(
            "/admin/token",
            axum::routing::post(auth::create_admin_token).delete(auth::delete_admin_token),
        )
        // Users, SSO settings (admin role), and the signed-in user's own endpoints.
        .route("/users", get(auth::list_users).post(auth::create_user))
        .route(
            "/users/{id}",
            axum::routing::put(auth::update_user).delete(auth::delete_user),
        )
        .route(
            "/sso/config",
            get(auth::get_sso_config)
                .put(auth::set_sso_config)
                .delete(auth::delete_sso_config),
        )
        .route("/auth/me", get(auth::me))
        .route(
            "/auth/me/password",
            axum::routing::put(auth::change_password),
        )
        .route("/auth/logout", axum::routing::post(auth::logout))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::guard,
        ))
        // Open endpoints: status (so the UI can show the right login screen),
        // password login, and the OIDC redirect pair.
        .route("/admin/status", get(auth::status))
        .route("/auth/login", axum::routing::post(auth::login))
        .route("/auth/sso/start", get(sso::start))
        .route("/auth/sso/callback", get(sso::callback));

    let mut app = Router::new()
        .route("/healthz", get(healthz))
        .route("/metrics", get(prometheus_metrics))
        .route("/openapi.yaml", get(openapi::spec))
        .route("/docs", get(openapi::docs))
        .nest("/api/v1", api);

    // Serve the built SPA (if present) with a fallback to index.html for client routing.
    if let Some(dir) = &state.ui_dir {
        if dir.is_dir() {
            let index = dir.join("index.html");
            app = app.fallback_service(ServeDir::new(dir).fallback(ServeFile::new(index)));
            info!("serving UI from {}", dir.display());
        } else {
            info!("UI dir {} not found; UI not served", dir.display());
        }
    }

    app.layer(TraceLayer::new_for_http()).with_state(state)
}

/// The admin API as a Pingora background service. Owns its own SQLite connection
/// (created inside the service's runtime).
pub struct ApiService {
    pub addr: String,
    pub db_url: String,
    pub config: ConfigHandle,
    pub metrics: Arc<Metrics>,
    pub cert_handle: CertHandle,
    pub acme_handle: AcmeHandle,
    pub ui_dir: Option<PathBuf>,
}

#[async_trait]
impl BackgroundService for ApiService {
    async fn start(&self, mut shutdown: ShutdownWatch) {
        let store = match Store::connect(&self.db_url).await {
            Ok(s) => s,
            Err(e) => {
                error!("admin API: failed to open db: {e}");
                return;
            }
        };
        // Refresh the snapshot once on startup so the proxy reflects current state.
        if let Ok(snap) = store.build_snapshot().await {
            self.config.store(snap);
        }

        // Load auth state once; the guard middleware reads the live handles.
        let auth = Arc::new(AuthState::new(
            store.get_admin_token_hash().await.ok().flatten(),
            store.count_users().await.unwrap_or(0) > 0,
            store.get_sso_config().await.ok().flatten(),
        ));
        if auth.enabled() {
            info!(
                "admin API auth: enabled (token: {}, users: {}, sso: {})",
                auth.admin_hash.load().is_some(),
                auth.has_users.load(std::sync::atomic::Ordering::Relaxed),
                auth.sso.load().is_some()
            );
        } else {
            info!("admin API auth: open (no admin token and no users)");
        }

        let state = AppState {
            store,
            config: self.config.clone(),
            metrics: self.metrics.clone(),
            cert_handle: self.cert_handle.clone(),
            acme_handle: self.acme_handle.clone(),
            ui_dir: self.ui_dir.clone(),
            auth,
        };
        let app = build_router(state);

        let listener = match tokio::net::TcpListener::bind(&self.addr).await {
            Ok(l) => l,
            Err(e) => {
                error!("admin API: cannot bind {}: {e}", self.addr);
                return;
            }
        };
        info!("admin API listening on {}", self.addr);

        let shutdown_fut = async move {
            let _ = shutdown.changed().await;
        };
        if let Err(e) = axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_fut)
            .await
        {
            error!("admin API server error: {e}");
        }
    }
}
