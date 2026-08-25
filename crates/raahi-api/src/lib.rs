//! Raahi control plane: the admin REST API + UI static serving, exposed as a Pingora
//! [`BackgroundService`] so it runs in-process alongside the data plane. Every mutation
//! writes to SQLite, then rebuilds and hot-swaps the proxy's config snapshot.

mod error;
mod handlers;
mod openapi;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::response::IntoResponse;
use axum::routing::get;
use pingora::server::ShutdownWatch;
use pingora::services::background::BackgroundService;
use raahi_acme::AcmeHandle;
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
    pub acme_handle: AcmeHandle,
    pub ui_dir: Option<PathBuf>,
    /// SHA-256 hex of the admin token; `None` = auth disabled. Swapped live on
    /// token generate/disable so the middleware never touches the DB.
    pub admin_hash: Arc<arc_swap::ArcSwap<Option<String>>>,
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
fn digest_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Bearer-token auth for the admin API. Accepts `Authorization: Bearer`,
/// `X-Admin-Token`, or `?access_token=` (the latter for EventSource/SSE, which
/// cannot set headers). No-op until a token is generated.
async fn require_admin(
    axum::extract::State(state): axum::extract::State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let Some(expected) = state.admin_hash.load().as_ref().clone() else {
        return next.run(req).await; // auth disabled
    };

    let presented: Option<String> = req
        .headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|v| {
            v.strip_prefix("Bearer ")
                .or_else(|| v.strip_prefix("bearer "))
        })
        .map(|s| s.trim().to_string())
        .or_else(|| {
            req.headers()
                .get("x-admin-token")
                .and_then(|h| h.to_str().ok())
                .map(|s| s.trim().to_string())
        })
        .or_else(|| {
            req.uri().query().and_then(|q| {
                q.split('&')
                    .find_map(|kv| kv.strip_prefix("access_token=").map(|v| v.to_string()))
            })
        });

    match presented {
        Some(token) if digest_eq(&handlers::sha256_hex(&token), &expected) => next.run(req).await,
        _ => (
            axum::http::StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({ "error": "missing or invalid admin token" })),
        )
            .into_response(),
    }
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
            axum::routing::post(create_admin_token).delete(delete_admin_token),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_admin,
        ))
        // Status stays open so the UI can tell whether to show the login screen.
        .route("/admin/status", get(admin_status));

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

        // Load the admin-token hash once; the middleware reads the live handle.
        let admin_hash = Arc::new(arc_swap::ArcSwap::from_pointee(
            store.get_admin_token_hash().await.ok().flatten(),
        ));
        if admin_hash.load().is_some() {
            info!("admin API auth: enabled (bearer token)");
        }

        let state = AppState {
            store,
            config: self.config.clone(),
            metrics: self.metrics.clone(),
            cert_handle: self.cert_handle.clone(),
            acme_handle: self.acme_handle.clone(),
            ui_dir: self.ui_dir.clone(),
            admin_hash,
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
