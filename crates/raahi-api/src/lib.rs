//! Raahi control plane: the admin REST API + UI static serving, exposed as a Pingora
//! [`BackgroundService`] so it runs in-process alongside the data plane. Every mutation
//! writes to SQLite, then rebuilds and hot-swaps the proxy's config snapshot.

mod error;
mod handlers;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use axum::routing::get;
use axum::Router;
use pingora::server::ShutdownWatch;
use pingora::services::background::BackgroundService;
use raahi_proxy::{CertHandle, CertStore, ConfigHandle, Metrics};
use raahi_store::Store;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
use tracing::{error, info};

pub use error::{ApiError, ApiResult};

/// Shared state for all handlers. Cheap to clone.
#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub config: ConfigHandle,
    pub metrics: Arc<Metrics>,
    pub cert_handle: CertHandle,
    pub ui_dir: Option<PathBuf>,
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

/// Assemble the full admin router (API under `/api/v1`, optional SPA static serving).
pub fn build_router(state: AppState) -> Router {
    use handlers::*;

    let api = Router::new()
        .route("/services", get(list_services).post(create_service))
        .route(
            "/services/{id}",
            get(get_service).put(update_service).delete(delete_service),
        )
        .route("/services/{id}/targets", get(list_targets).post(create_target))
        .route("/targets/{id}", axum::routing::put(update_target).delete(delete_target))
        .route("/routes", get(list_routes).post(create_route))
        .route("/routes/{id}", get(get_route).put(update_route).delete(delete_route))
        .route("/plugins", get(list_plugins).post(create_plugin))
        .route("/plugins/{id}", get(get_plugin).put(update_plugin).delete(delete_plugin))
        .route("/consumers", get(list_consumers).post(create_consumer))
        .route(
            "/consumers/{id}",
            get(list_consumers).put(update_consumer).delete(delete_consumer),
        )
        .route(
            "/consumers/{id}/credentials",
            get(list_credentials).post(create_credential),
        )
        .route("/credentials/{id}", axum::routing::delete(delete_credential))
        .route("/certificates", get(list_certificates).post(create_certificate))
        .route("/certificates/{id}", axum::routing::delete(delete_certificate))
        .route("/settings", get(get_settings).put(update_settings))
        .route("/metrics", get(metrics))
        .route("/requests", get(requests))
        .route("/events", get(events))
        .route("/config", get(config_summary));

    let mut app = Router::new()
        .route("/healthz", get(healthz))
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

        let state = AppState {
            store,
            config: self.config.clone(),
            metrics: self.metrics.clone(),
            cert_handle: self.cert_handle.clone(),
            ui_dir: self.ui_dir.clone(),
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
