//! Admin REST handlers. Every mutation persists to SQLite then rebuilds and
//! hot-swaps the proxy's config snapshot via [`crate::reload`].

use std::convert::Infallible;

use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use raahi_core::*;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};

use crate::error::{ApiError, ApiResult};
use crate::{reload, AppState};

type Id = i64;

pub async fn healthz() -> &'static str {
    "ok"
}

// ---- services ----------------------------------------------------------------
pub async fn list_services(State(s): State<AppState>) -> ApiResult<Json<Vec<Service>>> {
    Ok(Json(s.store.list_services().await?))
}

pub async fn get_service(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> ApiResult<Json<Service>> {
    s.store.get_service(id).await?.map(Json).ok_or(ApiError::NotFound)
}

pub async fn create_service(
    State(s): State<AppState>,
    Json(spec): Json<ServiceSpec>,
) -> ApiResult<Json<Service>> {
    let svc = s.store.create_service(&spec).await?;
    reload(&s).await?;
    Ok(Json(svc))
}

pub async fn update_service(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    Json(spec): Json<ServiceSpec>,
) -> ApiResult<Json<Service>> {
    let svc = s.store.update_service(id, &spec).await?.ok_or(ApiError::NotFound)?;
    reload(&s).await?;
    Ok(Json(svc))
}

pub async fn delete_service(State(s): State<AppState>, Path(id): Path<Id>) -> ApiResult<Json<Value>> {
    if !s.store.delete_service(id).await? {
        return Err(ApiError::NotFound);
    }
    reload(&s).await?;
    Ok(Json(json!({ "deleted": true })))
}

// ---- targets -----------------------------------------------------------------
pub async fn list_targets(
    State(s): State<AppState>,
    Path(service_id): Path<Id>,
) -> ApiResult<Json<Vec<Target>>> {
    Ok(Json(s.store.list_targets_for(service_id).await?))
}

pub async fn create_target(
    State(s): State<AppState>,
    Path(service_id): Path<Id>,
    Json(spec): Json<TargetSpec>,
) -> ApiResult<Json<Target>> {
    let t = s.store.create_target(service_id, &spec).await?;
    reload(&s).await?;
    Ok(Json(t))
}

pub async fn update_target(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    Json(spec): Json<TargetSpec>,
) -> ApiResult<Json<Target>> {
    let t = s.store.update_target(id, &spec).await?.ok_or(ApiError::NotFound)?;
    reload(&s).await?;
    Ok(Json(t))
}

pub async fn delete_target(State(s): State<AppState>, Path(id): Path<Id>) -> ApiResult<Json<Value>> {
    if !s.store.delete_target(id).await? {
        return Err(ApiError::NotFound);
    }
    reload(&s).await?;
    Ok(Json(json!({ "deleted": true })))
}

// ---- routes ------------------------------------------------------------------
pub async fn list_routes(State(s): State<AppState>) -> ApiResult<Json<Vec<Route>>> {
    Ok(Json(s.store.list_routes().await?))
}

pub async fn get_route(State(s): State<AppState>, Path(id): Path<Id>) -> ApiResult<Json<Route>> {
    s.store.get_route(id).await?.map(Json).ok_or(ApiError::NotFound)
}

pub async fn create_route(
    State(s): State<AppState>,
    Json(spec): Json<RouteSpec>,
) -> ApiResult<Json<Route>> {
    let r = s.store.create_route(&spec).await?;
    reload(&s).await?;
    Ok(Json(r))
}

pub async fn update_route(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    Json(spec): Json<RouteSpec>,
) -> ApiResult<Json<Route>> {
    let r = s.store.update_route(id, &spec).await?.ok_or(ApiError::NotFound)?;
    reload(&s).await?;
    Ok(Json(r))
}

pub async fn delete_route(State(s): State<AppState>, Path(id): Path<Id>) -> ApiResult<Json<Value>> {
    if !s.store.delete_route(id).await? {
        return Err(ApiError::NotFound);
    }
    reload(&s).await?;
    Ok(Json(json!({ "deleted": true })))
}

// ---- plugins -----------------------------------------------------------------
pub async fn list_plugins(State(s): State<AppState>) -> ApiResult<Json<Vec<Plugin>>> {
    Ok(Json(s.store.list_plugins().await?))
}

pub async fn get_plugin(State(s): State<AppState>, Path(id): Path<Id>) -> ApiResult<Json<Plugin>> {
    s.store.get_plugin(id).await?.map(Json).ok_or(ApiError::NotFound)
}

pub async fn create_plugin(
    State(s): State<AppState>,
    Json(spec): Json<PluginSpec>,
) -> ApiResult<Json<Plugin>> {
    validate_plugin(&spec)?;
    let p = s.store.create_plugin(&spec).await?;
    reload(&s).await?;
    Ok(Json(p))
}

pub async fn update_plugin(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    Json(spec): Json<PluginSpec>,
) -> ApiResult<Json<Plugin>> {
    validate_plugin(&spec)?;
    let p = s.store.update_plugin(id, &spec).await?.ok_or(ApiError::NotFound)?;
    reload(&s).await?;
    Ok(Json(p))
}

pub async fn delete_plugin(State(s): State<AppState>, Path(id): Path<Id>) -> ApiResult<Json<Value>> {
    if !s.store.delete_plugin(id).await? {
        return Err(ApiError::NotFound);
    }
    reload(&s).await?;
    Ok(Json(json!({ "deleted": true })))
}

fn validate_plugin(spec: &PluginSpec) -> ApiResult<()> {
    match spec.scope {
        PluginScope::Service if spec.service_id.is_none() => {
            Err(ApiError::BadRequest("service-scoped plugin needs service_id".into()))
        }
        PluginScope::Route if spec.route_id.is_none() => {
            Err(ApiError::BadRequest("route-scoped plugin needs route_id".into()))
        }
        _ => Ok(()),
    }
}

// ---- consumers + credentials -------------------------------------------------
pub async fn list_consumers(State(s): State<AppState>) -> ApiResult<Json<Vec<Consumer>>> {
    Ok(Json(s.store.list_consumers().await?))
}

pub async fn create_consumer(
    State(s): State<AppState>,
    Json(spec): Json<ConsumerSpec>,
) -> ApiResult<Json<Consumer>> {
    let c = s.store.create_consumer(&spec).await?;
    reload(&s).await?;
    Ok(Json(c))
}

pub async fn update_consumer(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    Json(spec): Json<ConsumerSpec>,
) -> ApiResult<Json<Consumer>> {
    let c = s.store.update_consumer(id, &spec).await?.ok_or(ApiError::NotFound)?;
    reload(&s).await?;
    Ok(Json(c))
}

pub async fn delete_consumer(State(s): State<AppState>, Path(id): Path<Id>) -> ApiResult<Json<Value>> {
    if !s.store.delete_consumer(id).await? {
        return Err(ApiError::NotFound);
    }
    reload(&s).await?;
    Ok(Json(json!({ "deleted": true })))
}

pub async fn list_credentials(
    State(s): State<AppState>,
    Path(consumer_id): Path<Id>,
) -> ApiResult<Json<Vec<ConsumerCredential>>> {
    // ConsumerCredential serializes without the secret.
    Ok(Json(s.store.list_credentials_for(consumer_id).await?))
}

pub async fn create_credential(
    State(s): State<AppState>,
    Path(consumer_id): Path<Id>,
    Json(spec): Json<CredentialSpec>,
) -> ApiResult<Json<ConsumerCredential>> {
    // basic-auth secrets are hashed (bcrypt) before storage; key-auth has no secret.
    let hashed = match spec.credential_type {
        CredentialType::BasicAuth => {
            let pw = spec
                .secret
                .as_deref()
                .ok_or_else(|| ApiError::BadRequest("basic-auth credential needs a secret".into()))?;
            Some(
                bcrypt::hash(pw, bcrypt::DEFAULT_COST)
                    .map_err(|e| ApiError::Internal(format!("hash: {e}")))?,
            )
        }
        CredentialType::KeyAuth => None,
    };
    let cred = s
        .store
        .create_credential(consumer_id, spec.credential_type, &spec.identifier, hashed.as_deref())
        .await?;
    reload(&s).await?;
    Ok(Json(cred))
}

pub async fn delete_credential(State(s): State<AppState>, Path(id): Path<Id>) -> ApiResult<Json<Value>> {
    if !s.store.delete_credential(id).await? {
        return Err(ApiError::NotFound);
    }
    reload(&s).await?;
    Ok(Json(json!({ "deleted": true })))
}

// ---- certificates ------------------------------------------------------------
pub async fn list_certificates(State(s): State<AppState>) -> ApiResult<Json<Vec<Certificate>>> {
    // key_pem is skipped in serialization.
    Ok(Json(s.store.list_certificates().await?))
}

pub async fn create_certificate(
    State(s): State<AppState>,
    Json(spec): Json<CertificateSpec>,
) -> ApiResult<Json<Certificate>> {
    if !spec.cert_pem.contains("BEGIN CERTIFICATE") {
        return Err(ApiError::BadRequest("cert_pem is not a PEM certificate".into()));
    }
    if !spec.key_pem.contains("PRIVATE KEY") {
        return Err(ApiError::BadRequest("key_pem is not a PEM private key".into()));
    }
    let c = s.store.create_certificate(&spec).await?;
    // Certs only take effect on the TLS listener at (re)start; no live swap.
    Ok(Json(c))
}

pub async fn delete_certificate(State(s): State<AppState>, Path(id): Path<Id>) -> ApiResult<Json<Value>> {
    if !s.store.delete_certificate(id).await? {
        return Err(ApiError::NotFound);
    }
    Ok(Json(json!({ "deleted": true })))
}

// ---- settings ----------------------------------------------------------------
pub async fn get_settings(State(s): State<AppState>) -> ApiResult<Json<Settings>> {
    Ok(Json(s.store.get_settings().await?))
}

pub async fn update_settings(
    State(s): State<AppState>,
    Json(spec): Json<SettingsSpec>,
) -> ApiResult<Json<Settings>> {
    let settings = s.store.update_settings(&spec).await?;
    reload(&s).await?;
    Ok(Json(settings))
}

// ---- observability -----------------------------------------------------------
pub async fn metrics(State(s): State<AppState>) -> Json<Value> {
    let snap = s.metrics.snapshot();
    Json(json!(snap))
}

#[derive(Deserialize)]
pub struct RequestsQuery {
    #[serde(default = "default_limit")]
    limit: usize,
}
fn default_limit() -> usize {
    100
}

pub async fn requests(
    State(s): State<AppState>,
    Query(q): Query<RequestsQuery>,
) -> Json<Value> {
    Json(json!(s.metrics.recent(q.limit.min(500))))
}

/// SSE stream of completed requests for the live tail / dashboard.
pub async fn events(
    State(s): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = s.metrics.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|res| match res {
        Ok(rec) => Some(Ok(Event::default()
            .json_data(rec)
            .unwrap_or_else(|_| Event::default().data("{}")))),
        Err(_) => None, // dropped on lag; client keeps the connection
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Debug view of the live compiled snapshot (counts + version).
pub async fn config_summary(State(s): State<AppState>) -> Json<Value> {
    let rc = s.config.load();
    Json(json!({
        "version": rc.data.version,
        "routes": rc.data.routes.len(),
        "services": rc.data.services.len(),
        "plugins": rc.data.plugins.len(),
        "consumers": rc.data.consumers.len(),
        "key_credentials": rc.data.key_index.len(),
    }))
}
