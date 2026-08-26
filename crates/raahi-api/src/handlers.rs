//! Admin REST handlers. Every mutation persists to SQLite then rebuilds and
//! hot-swaps the proxy's config snapshot via [`crate::reload`].

use std::convert::Infallible;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use chrono::Utc;
use raahi_core::*;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};

use crate::error::{ApiError, ApiResult};
use crate::{AppState, reload, reload_certs};

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
    s.store
        .get_service(id)
        .await?
        .map(Json)
        .ok_or(ApiError::NotFound)
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
    let svc = s
        .store
        .update_service(id, &spec)
        .await?
        .ok_or(ApiError::NotFound)?;
    reload(&s).await?;
    Ok(Json(svc))
}

pub async fn delete_service(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> ApiResult<Json<Value>> {
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
    let t = s
        .store
        .update_target(id, &spec)
        .await?
        .ok_or(ApiError::NotFound)?;
    reload(&s).await?;
    Ok(Json(t))
}

pub async fn delete_target(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> ApiResult<Json<Value>> {
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
    s.store
        .get_route(id)
        .await?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

/// Route paths must be absolute prefixes (`/api`) or compilable `~` regexes
/// (`~/users/\d+`); anything else would silently never match.
fn validate_route_paths(spec: &RouteSpec) -> ApiResult<()> {
    for p in &spec.paths {
        if raahi_core::is_regex_path(p) {
            raahi_core::compile_path_regex(p)
                .map_err(|e| ApiError::BadRequest(format!("invalid regex path {p:?}: {e}")))?;
        } else if !p.starts_with('/') {
            return Err(ApiError::BadRequest(format!(
                "path {p:?} must start with '/' (or '~' for a regex path)"
            )));
        }
    }
    Ok(())
}

pub async fn create_route(
    State(s): State<AppState>,
    Json(spec): Json<RouteSpec>,
) -> ApiResult<Json<Route>> {
    validate_route_paths(&spec)?;
    ensure_split_services_exist(&s, &spec).await?;
    let r = s.store.create_route(&spec).await?;
    reload(&s).await?;
    Ok(Json(r))
}

pub async fn update_route(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    Json(spec): Json<RouteSpec>,
) -> ApiResult<Json<Route>> {
    validate_route_paths(&spec)?;
    ensure_split_services_exist(&s, &spec).await?;
    let r = s
        .store
        .update_route(id, &spec)
        .await?
        .ok_or(ApiError::NotFound)?;
    reload(&s).await?;
    Ok(Json(r))
}

/// Every traffic-split entry must reference an existing service with weight >= 1.
async fn ensure_split_services_exist(s: &AppState, spec: &RouteSpec) -> ApiResult<()> {
    if spec.splits.is_empty() {
        return Ok(());
    }
    let services = s.store.list_services().await?;
    for sp in &spec.splits {
        if sp.weight < 1 {
            return Err(ApiError::BadRequest("split weights must be >= 1".into()));
        }
        if !services.iter().any(|svc| svc.id == sp.service_id) {
            return Err(ApiError::BadRequest(format!(
                "split service {} not found",
                sp.service_id
            )));
        }
    }
    Ok(())
}

pub async fn delete_route(State(s): State<AppState>, Path(id): Path<Id>) -> ApiResult<Json<Value>> {
    if !s.store.delete_route(id).await? {
        return Err(ApiError::NotFound);
    }
    reload(&s).await?;
    Ok(Json(json!({ "deleted": true })))
}

// ---- stream routes -------------------------------------------------------------

/// Reminder attached to stream-route responses whenever the set of listen
/// addresses changed: listeners bind at startup only (retargeting is live).
const LISTENER_NOTE: &str = "listener changes take effect on restart";

/// listen_addr must be a valid socket address and the target service must exist.
async fn validate_stream_route(s: &AppState, spec: &StreamRouteSpec) -> ApiResult<()> {
    if spec.listen_addr.parse::<std::net::SocketAddr>().is_err() {
        return Err(ApiError::BadRequest(format!(
            "listen_addr '{}' is not a valid socket address (host:port)",
            spec.listen_addr
        )));
    }
    if s.store.get_service(spec.service_id).await?.is_none() {
        return Err(ApiError::BadRequest(format!(
            "service {} not found",
            spec.service_id
        )));
    }
    Ok(())
}

pub async fn list_stream_routes(State(s): State<AppState>) -> ApiResult<Json<Vec<StreamRoute>>> {
    Ok(Json(s.store.list_stream_routes().await?))
}

pub async fn create_stream_route(
    State(s): State<AppState>,
    Json(spec): Json<StreamRouteSpec>,
) -> ApiResult<Json<Value>> {
    validate_stream_route(&s, &spec).await?;
    let r = s.store.create_stream_route(&spec).await?;
    reload(&s).await?;
    let mut out = json!(r);
    out["note"] = json!(LISTENER_NOTE); // a new listener needs binding
    Ok(Json(out))
}

pub async fn update_stream_route(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    Json(spec): Json<StreamRouteSpec>,
) -> ApiResult<Json<Value>> {
    validate_stream_route(&s, &spec).await?;
    let prev = s
        .store
        .get_stream_route(id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let r = s
        .store
        .update_stream_route(id, &spec)
        .await?
        .ok_or(ApiError::NotFound)?;
    reload(&s).await?;
    let mut out = json!(r);
    if prev.listen_addr != r.listen_addr {
        out["note"] = json!(LISTENER_NOTE); // rebinding; retargeting alone is live
    }
    Ok(Json(out))
}

pub async fn delete_stream_route(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> ApiResult<Json<Value>> {
    if !s.store.delete_stream_route(id).await? {
        return Err(ApiError::NotFound);
    }
    reload(&s).await?;
    Ok(Json(json!({ "deleted": true, "note": LISTENER_NOTE })))
}

// ---- plugins -----------------------------------------------------------------
pub async fn list_plugins(State(s): State<AppState>) -> ApiResult<Json<Vec<Plugin>>> {
    Ok(Json(s.store.list_plugins().await?))
}

pub async fn get_plugin(State(s): State<AppState>, Path(id): Path<Id>) -> ApiResult<Json<Plugin>> {
    s.store
        .get_plugin(id)
        .await?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

pub async fn create_plugin(
    State(s): State<AppState>,
    Json(spec): Json<PluginSpec>,
) -> ApiResult<Json<Plugin>> {
    validate_plugin(&spec)?;
    ensure_wasm_module_exists(&s, &spec).await?;
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
    ensure_wasm_module_exists(&s, &spec).await?;
    let p = s
        .store
        .update_plugin(id, &spec)
        .await?
        .ok_or(ApiError::NotFound)?;
    reload(&s).await?;
    Ok(Json(p))
}

/// A `wasm` plugin must reference an uploaded module by name.
async fn ensure_wasm_module_exists(s: &AppState, spec: &PluginSpec) -> ApiResult<()> {
    if spec.plugin_type != PluginType::Wasm {
        return Ok(());
    }
    let name = spec.config["module"].as_str().unwrap_or("");
    let exists = s
        .store
        .list_wasm_modules()
        .await?
        .iter()
        .any(|m| m.name == name);
    if !exists {
        return Err(ApiError::BadRequest(format!(
            "wasm module '{name}' not found"
        )));
    }
    Ok(())
}

pub async fn delete_plugin(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> ApiResult<Json<Value>> {
    if !s.store.delete_plugin(id).await? {
        return Err(ApiError::NotFound);
    }
    reload(&s).await?;
    Ok(Json(json!({ "deleted": true })))
}

fn validate_plugin(spec: &PluginSpec) -> ApiResult<()> {
    match spec.scope {
        PluginScope::Service if spec.service_id.is_none() => {
            return Err(ApiError::BadRequest(
                "service-scoped plugin needs service_id".into(),
            ));
        }
        PluginScope::Route if spec.route_id.is_none() => {
            return Err(ApiError::BadRequest(
                "route-scoped plugin needs route_id".into(),
            ));
        }
        _ => {}
    }
    // Per-type config sanity (misconfigurations that would silently no-op or black-hole).
    match spec.plugin_type {
        PluginType::Redirect => {
            let loc = spec.config["location"].as_str().unwrap_or("");
            if !loc.starts_with("http://") && !loc.starts_with("https://") {
                return Err(ApiError::BadRequest(
                    "redirect needs a location starting with http:// or https://".into(),
                ));
            }
        }
        PluginType::HttpLog => {
            let ep = spec.config["endpoint"].as_str().unwrap_or("");
            if !ep.starts_with("http://") && !ep.starts_with("https://") {
                return Err(ApiError::BadRequest(
                    "http-log needs an endpoint starting with http:// or https://".into(),
                ));
            }
        }
        PluginType::IpRestriction => {
            let both_empty = spec.config["allow"]
                .as_array()
                .map_or(true, |a| a.is_empty())
                && spec.config["deny"]
                    .as_array()
                    .map_or(true, |a| a.is_empty());
            if both_empty {
                return Err(ApiError::BadRequest(
                    "ip-restriction needs at least one allow or deny entry".into(),
                ));
            }
        }
        PluginType::Wasm => {
            if spec.config["module"].as_str().unwrap_or("").is_empty() {
                return Err(ApiError::BadRequest(
                    "wasm plugin needs a module name (upload one under WASM modules)".into(),
                ));
            }
        }
        PluginType::ProxyCache => {
            if spec.config["ttl_secs"].as_u64().unwrap_or(60) < 1 {
                return Err(ApiError::BadRequest(
                    "proxy-cache needs ttl_secs >= 1".into(),
                ));
            }
        }
        PluginType::Hsts => {
            if spec.config["max_age_secs"].as_u64().unwrap_or(63_072_000) < 1 {
                return Err(ApiError::BadRequest("hsts needs max_age_secs >= 1".into()));
            }
        }
        PluginType::RequestId => {
            let name = spec.config["header_name"]
                .as_str()
                .unwrap_or("X-Request-Id");
            if axum::http::header::HeaderName::from_bytes(name.as_bytes()).is_err() {
                return Err(ApiError::BadRequest(format!(
                    "request-id header_name {name:?} is not a valid header name"
                )));
            }
        }
        PluginType::ResponseCompression => {
            // The level is applied to every algorithm the module may pick, so it has
            // to stay within gzip's range; 0 would mean "disabled".
            let level = spec.config["level"].as_u64().unwrap_or(5);
            if level < 1 || level > raahi_proxy::MAX_COMPRESSION_LEVEL as u64 {
                return Err(ApiError::BadRequest(format!(
                    "response-compression level must be 1..={}",
                    raahi_proxy::MAX_COMPRESSION_LEVEL
                )));
            }
        }
        PluginType::ResponseBodyTransform => {
            let empty = spec.config["replace"]
                .as_array()
                .map_or(true, |a| a.is_empty());
            if empty {
                return Err(ApiError::BadRequest(
                    "response-body-transform needs at least one replace entry".into(),
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

/// Drop every proxy-cache entry (process-global cache).
pub async fn purge_cache() -> Json<Value> {
    Json(json!({ "purged": raahi_proxy::purge_cache() }))
}

// ---- consumers + credentials -------------------------------------------------
pub async fn list_consumers(State(s): State<AppState>) -> ApiResult<Json<Vec<Consumer>>> {
    Ok(Json(s.store.list_consumers().await?))
}

pub async fn get_consumer(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> ApiResult<Json<Consumer>> {
    s.store
        .get_consumer(id)
        .await?
        .map(Json)
        .ok_or(ApiError::NotFound)
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
    let c = s
        .store
        .update_consumer(id, &spec)
        .await?
        .ok_or(ApiError::NotFound)?;
    reload(&s).await?;
    Ok(Json(c))
}

pub async fn delete_consumer(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> ApiResult<Json<Value>> {
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
    // basic-auth secrets are hashed (bcrypt) before storage; key-auth has no secret;
    // jwt stores its verification material as a JSON blob (needed raw to verify).
    let stored = match spec.credential_type {
        CredentialType::BasicAuth => {
            let pw = spec.secret.as_deref().ok_or_else(|| {
                ApiError::BadRequest("basic-auth credential needs a secret".into())
            })?;
            Some(
                bcrypt::hash(pw, bcrypt::DEFAULT_COST)
                    .map_err(|e| ApiError::Internal(format!("hash: {e}")))?,
            )
        }
        CredentialType::KeyAuth => None,
        CredentialType::Jwt => {
            let secret = spec.secret.as_deref().ok_or_else(|| {
                ApiError::BadRequest(
                    "jwt credential needs a secret (HMAC secret or RSA public key PEM)".into(),
                )
            })?;
            let algorithm = spec.algorithm.as_deref().unwrap_or("HS256");
            if !matches!(algorithm, "HS256" | "HS384" | "HS512" | "RS256") {
                return Err(ApiError::BadRequest(
                    "jwt algorithm must be one of HS256, HS384, HS512, RS256".into(),
                ));
            }
            if algorithm == "RS256" && !secret.contains("BEGIN") {
                return Err(ApiError::BadRequest(
                    "RS256 requires an RSA public key in PEM format".into(),
                ));
            }
            Some(json!({ "algorithm": algorithm, "secret": secret }).to_string())
        }
    };
    let cred = s
        .store
        .create_credential(
            consumer_id,
            spec.credential_type,
            &spec.identifier,
            stored.as_deref(),
        )
        .await?;
    reload(&s).await?;
    Ok(Json(cred))
}

pub async fn delete_credential(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> ApiResult<Json<Value>> {
    if !s.store.delete_credential(id).await? {
        return Err(ApiError::NotFound);
    }
    reload(&s).await?;
    Ok(Json(json!({ "deleted": true })))
}

// ---- wasm modules --------------------------------------------------------------
pub async fn list_wasm_modules(State(s): State<AppState>) -> ApiResult<Json<Value>> {
    let mods = s.store.list_wasm_modules().await?;
    Ok(Json(json!(
        mods.iter()
            .map(|m| json!({
                "id": m.id,
                "name": m.name,
                "description": m.description,
                "size_bytes": m.wasm.len(),
                "created_at": m.created_at,
            }))
            .collect::<Vec<_>>()
    )))
}

pub async fn create_wasm_module(
    State(s): State<AppState>,
    Json(spec): Json<WasmModuleSpec>,
) -> ApiResult<Json<Value>> {
    if spec.name.trim().is_empty() {
        return Err(ApiError::BadRequest("module needs a name".into()));
    }
    // Accept raw wasm (base64) or WAT source text.
    let bytes = match (&spec.wasm_base64, &spec.wat) {
        (Some(b64), _) if !b64.trim().is_empty() => {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(b64.trim())
                .map_err(|e| ApiError::BadRequest(format!("invalid base64: {e}")))?
        }
        (_, Some(wat)) if !wat.trim().is_empty() => {
            raahi_proxy::wat_to_wasm(wat).map_err(ApiError::BadRequest)?
        }
        _ => {
            return Err(ApiError::BadRequest(
                "provide the module as wasm_base64 or wat".into(),
            ));
        }
    };
    // Compile-check + ABI check before storing.
    raahi_proxy::validate_wasm(&bytes).map_err(ApiError::BadRequest)?;
    let m = s
        .store
        .create_wasm_module(spec.name.trim(), &spec.description, &bytes)
        .await?;
    reload(&s).await?;
    Ok(Json(json!({
        "id": m.id, "name": m.name, "description": m.description,
        "size_bytes": m.wasm.len(), "created_at": m.created_at,
    })))
}

pub async fn delete_wasm_module(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> ApiResult<Json<Value>> {
    // Refuse while any wasm plugin references this module by name.
    if let Some(m) = s.store.get_wasm_module(id).await? {
        let in_use = s.store.list_plugins().await?.into_iter().any(|p| {
            p.plugin_type == PluginType::Wasm && p.config["module"].as_str() == Some(&m.name)
        });
        if in_use {
            return Err(ApiError::BadRequest(format!(
                "module '{}' is referenced by a wasm plugin; delete that plugin first",
                m.name
            )));
        }
    }
    if !s.store.delete_wasm_module(id).await? {
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
    if let Some(config) = &spec.acme_config {
        if !spec.cert_pem.is_empty() || !spec.key_pem.is_empty() {
            return Err(ApiError::BadRequest(
                "ACME certificates must not include cert_pem or key_pem".into(),
            ));
        }
        raahi_acme::validate_config(&spec.sni, config)
            .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    } else {
        // Validate the PEM actually parses (cert + private key) before storing.
        raahi_proxy::validate_cert(&spec.cert_pem, &spec.key_pem).map_err(ApiError::BadRequest)?;
    }
    let c = s.store.create_certificate(&spec).await?;
    // Live-swap the cert store (no restart) if the HTTPS listener is already bound.
    reload_certs(&s).await?;
    if c.acme_config.is_some() {
        s.acme_handle.trigger();
    }
    Ok(Json(c))
}

pub async fn delete_certificate(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> ApiResult<Json<Value>> {
    if !s.store.delete_certificate(id).await? {
        return Err(ApiError::NotFound);
    }
    reload_certs(&s).await?;
    Ok(Json(json!({ "deleted": true })))
}

pub async fn renew_certificate(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> ApiResult<Json<Value>> {
    let certificate = s
        .store
        .get_certificate(id)
        .await?
        .ok_or(ApiError::NotFound)?;
    if certificate.acme_config.is_none() {
        return Err(ApiError::BadRequest(
            "only ACME-managed certificates can be renewed".into(),
        ));
    }
    if certificate.acme_status.as_ref().is_some_and(|status| {
        status.state == AcmeState::Issuing
            && status
                .last_attempt
                .is_some_and(|attempt| attempt > Utc::now() - chrono::Duration::hours(1))
    }) {
        return Err(ApiError::BadRequest(
            "certificate issuance is already in progress".into(),
        ));
    }
    s.store
        .update_certificate_acme_status(id, &AcmeStatus::pending())
        .await?;
    s.acme_handle.trigger();
    Ok(Json(json!({ "queued": true })))
}

#[derive(serde::Deserialize)]
pub struct CloudflareTokenSpec {
    token: String,
}

pub async fn get_cloudflare_token_status(State(s): State<AppState>) -> ApiResult<Json<Value>> {
    let configured = s
        .store
        .get_cloudflare_api_token()
        .await?
        .is_some_and(|token| !token.trim().is_empty());
    Ok(Json(json!({ "configured": configured })))
}

pub async fn set_cloudflare_token(
    State(s): State<AppState>,
    Json(spec): Json<CloudflareTokenSpec>,
) -> ApiResult<Json<Value>> {
    let token = spec.token.trim();
    if token.is_empty() {
        return Err(ApiError::BadRequest("Cloudflare API token is empty".into()));
    }
    s.store.set_cloudflare_api_token(Some(token)).await?;
    s.acme_handle.trigger();
    Ok(Json(json!({ "configured": true })))
}

pub async fn delete_cloudflare_token(State(s): State<AppState>) -> ApiResult<Json<Value>> {
    s.store.set_cloudflare_api_token(None).await?;
    Ok(Json(json!({ "configured": false })))
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
    // The default certificate (active_certificate_id) may have changed — swap it live.
    reload_certs(&s).await?;
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

pub async fn requests(State(s): State<AppState>, Query(q): Query<RequestsQuery>) -> Json<Value> {
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

/// Prometheus text-format exporter. Served at `/metrics` (outside `/api/v1` and
/// outside admin auth so scrapers work without config; bind the admin listener
/// accordingly).
pub async fn prometheus_metrics(State(s): State<AppState>) -> impl axum::response::IntoResponse {
    let snap = s.metrics.snapshot();
    let rc = s.config.load();
    let route_name = |id: Id| {
        rc.data
            .routes
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| format!("#{id}"))
    };
    let esc = |v: &str| v.replace('\\', "\\\\").replace('"', "\\\"");

    let mut out = String::with_capacity(2048);
    let mut m = |line: String| {
        out.push_str(&line);
        out.push('\n');
    };

    m("# HELP raahi_requests_total Total requests handled by the proxy.".into());
    m("# TYPE raahi_requests_total counter".into());
    m(format!("raahi_requests_total {}", snap.total));

    m("# HELP raahi_responses_total Responses by status class.".into());
    m("# TYPE raahi_responses_total counter".into());
    for (class, v) in [
        ("2xx", snap.class_2xx),
        ("3xx", snap.class_3xx),
        ("4xx", snap.class_4xx),
        ("5xx", snap.class_5xx),
    ] {
        m(format!("raahi_responses_total{{class=\"{class}\"}} {v}"));
    }

    m("# HELP raahi_no_route_total Requests that matched no route (404).".into());
    m("# TYPE raahi_no_route_total counter".into());
    m(format!("raahi_no_route_total {}", snap.no_route));

    m("# HELP raahi_request_latency_ms Request latency in milliseconds (recent-window percentiles).".into());
    m("# TYPE raahi_request_latency_ms gauge".into());
    for (q, v) in [
        ("0.5", snap.p50_latency_ms),
        ("0.95", snap.p95_latency_ms),
        ("0.99", snap.p99_latency_ms),
    ] {
        m(format!("raahi_request_latency_ms{{quantile=\"{q}\"}} {v}"));
    }
    m("# HELP raahi_request_latency_avg_ms Mean request latency (all-time).".into());
    m("# TYPE raahi_request_latency_avg_ms gauge".into());
    m(format!(
        "raahi_request_latency_avg_ms {}",
        snap.avg_latency_ms
    ));

    // Samples of one metric family must stay contiguous in the exposition format.
    m("# HELP raahi_route_requests_total Requests per route.".into());
    m("# TYPE raahi_route_requests_total counter".into());
    for r in &snap.top_routes {
        m(format!(
            "raahi_route_requests_total{{route=\"{}\"}} {}",
            esc(&route_name(r.route_id)),
            r.count
        ));
    }
    m("# HELP raahi_route_errors_total 4xx+5xx responses per route.".into());
    m("# TYPE raahi_route_errors_total counter".into());
    for r in &snap.top_routes {
        m(format!(
            "raahi_route_errors_total{{route=\"{}\"}} {}",
            esc(&route_name(r.route_id)),
            r.errors
        ));
    }

    m("# HELP raahi_consumer_requests_total Requests per authenticated consumer.".into());
    m("# TYPE raahi_consumer_requests_total counter".into());
    for c in &snap.top_consumers {
        m(format!(
            "raahi_consumer_requests_total{{consumer=\"{}\"}} {}",
            esc(&c.consumer),
            c.count
        ));
    }

    m("# HELP raahi_target_healthy Upstream target health (1 = healthy).".into());
    m("# TYPE raahi_target_healthy gauge".into());
    let mut targets: Vec<(Id, String, bool)> = Vec::new();
    for (&sid, sr) in &rc.services {
        for b in &sr.backends {
            targets.push((
                sid,
                format!("{}:{}", b.host, b.port),
                b.healthy.load(std::sync::atomic::Ordering::Relaxed),
            ));
        }
    }
    targets.sort();
    for (sid, addr, healthy) in targets {
        let svc = rc
            .services
            .get(&sid)
            .map(|sr| sr.service.name.clone())
            .unwrap_or_else(|| format!("#{sid}"));
        m(format!(
            "raahi_target_healthy{{service=\"{}\",target=\"{}\"}} {}",
            esc(&svc),
            esc(&addr),
            healthy as u8
        ));
    }

    // Stream (L4) listeners. Each family's samples stay contiguous.
    let stream = raahi_proxy::stream_stats();
    m(
        "# HELP raahi_stream_connections_total TCP connections accepted per stream listener."
            .into(),
    );
    m("# TYPE raahi_stream_connections_total counter".into());
    for (listener, conns, _, _) in &stream {
        m(format!(
            "raahi_stream_connections_total{{listener=\"{}\"}} {conns}",
            esc(listener)
        ));
    }
    m("# HELP raahi_stream_bytes_total Bytes proxied per stream listener by direction (up = client to upstream).".into());
    m("# TYPE raahi_stream_bytes_total counter".into());
    for (listener, _, up, down) in &stream {
        m(format!(
            "raahi_stream_bytes_total{{listener=\"{}\",direction=\"up\"}} {up}",
            esc(listener)
        ));
        m(format!(
            "raahi_stream_bytes_total{{listener=\"{}\",direction=\"down\"}} {down}",
            esc(listener)
        ));
    }

    m("# HELP raahi_config_version Monotonic config snapshot version.".into());
    m("# TYPE raahi_config_version gauge".into());
    m(format!("raahi_config_version {}", rc.data.version));

    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        out,
    )
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

/// Live health of every upstream target (active TCP checks + passive marking).
pub async fn target_health(State(s): State<AppState>) -> Json<Value> {
    let rc = s.config.load();
    let mut out = Vec::new();
    for (&sid, sr) in &rc.services {
        for b in &sr.backends {
            out.push(json!({
                "service_id": sid,
                "target_id": b.target_id,
                "host": b.host,
                "port": b.port,
                // The concrete address probes and proxying currently use — shows
                // which of a multi-address host (e.g. localhost) was elected.
                "addr": b.addr_string(),
                "healthy": b.healthy.load(std::sync::atomic::Ordering::Relaxed),
            }));
        }
    }
    Json(json!(out))
}

#[derive(Deserialize)]
pub struct RouterTestQuery {
    #[serde(default)]
    host: String,
    path: String,
    #[serde(default = "default_method")]
    method: String,
    /// Request headers as `Name:value,Name2:value2`.
    #[serde(default)]
    headers: String,
}
fn default_method() -> String {
    "GET".into()
}

/// Dry-run the router: which route/service would this request hit?
pub async fn router_test(
    State(s): State<AppState>,
    Query(q): Query<RouterTestQuery>,
) -> Json<Value> {
    let hdrs: Vec<(String, String)> = q
        .headers
        .split(',')
        .filter_map(|kv| {
            let (k, v) = kv.split_once(':')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .filter(|(k, _)| !k.is_empty())
        .collect();
    let header = |name: &str| {
        hdrs.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    let rc = s.config.load();
    match rc
        .data
        .match_route(&q.host, &q.path, &q.method.to_uppercase(), &header)
    {
        Some(m) => {
            let plugins: Vec<&str> = {
                let mut ordered = rc.data.plugins_for(m.route.id, m.service.id);
                ordered.sort_by_key(|p| p.ordering);
                ordered.iter().map(|p| p.plugin_type.as_str()).collect()
            };
            let mut out = json!({
                "matched": true,
                "route_id": m.route.id,
                "route_name": m.route.name,
                "service_id": m.service.id,
                "service_name": m.service.name,
                "matched_prefix": m.matched_prefix,
                "strip_path": m.route.strip_path,
                "preserve_host": m.route.preserve_host,
                "plugins": plugins,
            });
            if !m.route.splits.is_empty() {
                let splits: Vec<Value> = m
                    .route
                    .splits
                    .iter()
                    .map(|sp| {
                        let name = rc
                            .data
                            .services
                            .get(&sp.service_id)
                            .map(|svc| svc.name.clone())
                            .unwrap_or_else(|| format!("#{}", sp.service_id));
                        json!({ "service_id": sp.service_id, "service_name": name, "weight": sp.weight })
                    })
                    .collect();
                out["splits"] = json!(splits);
            }
            Json(out)
        }
        None => Json(json!({ "matched": false })),
    }
}

#[derive(Deserialize)]
pub struct ExportQuery {
    /// Include cert private keys and credential secrets/hashes — needed for a
    /// restorable backup. Off by default.
    #[serde(default)]
    include_secrets: bool,
}

/// Export the full configuration as one JSON document. With
/// `?include_secrets=true` the export is a complete restorable backup.
pub async fn export_config(
    State(s): State<AppState>,
    Query(q): Query<ExportQuery>,
) -> ApiResult<Json<Value>> {
    use base64::Engine;

    let services = s.store.list_services().await?;
    let mut services_out = Vec::new();
    for svc in &services {
        let targets = s.store.list_targets_for(svc.id).await?;
        services_out.push(json!({ "service": svc, "targets": targets }));
    }

    let consumers = s.store.list_consumers().await?;
    let mut consumers_out = Vec::new();
    for c in &consumers {
        let creds = s.store.list_credentials_for(c.id).await?;
        let creds: Vec<Value> = creds
            .iter()
            .map(|cr| {
                let mut v = json!(cr);
                if q.include_secrets {
                    v["secret"] = json!(cr.secret);
                }
                v
            })
            .collect();
        consumers_out.push(json!({ "consumer": c, "credentials": creds }));
    }

    let certs: Vec<Value> = s
        .store
        .list_certificates()
        .await?
        .iter()
        .map(|c| {
            let mut v = json!(c);
            if q.include_secrets {
                v["key_pem"] = json!(c.key_pem);
            }
            v
        })
        .collect();

    let wasm: Vec<Value> = s
        .store
        .list_wasm_modules()
        .await?
        .iter()
        .map(|m| {
            json!({
                "name": m.name,
                "description": m.description,
                "wasm_base64": base64::engine::general_purpose::STANDARD.encode(&m.wasm),
            })
        })
        .collect();

    let acme = if q.include_secrets {
        Some(ImportAcmeState {
            cloudflare_api_token: s.store.get_cloudflare_api_token().await?,
            accounts: s.store.list_acme_accounts().await?,
        })
    } else {
        None
    };

    Ok(Json(json!({
        "raahi_export_version": 1,
        "settings": s.store.get_settings().await?,
        "services": services_out,
        "routes": s.store.list_routes().await?,
        "stream_routes": s.store.list_stream_routes().await?,
        "plugins": s.store.list_plugins().await?,
        "consumers": consumers_out,
        "certificates": certs,
        "wasm_modules": wasm,
        "acme": acme,
    })))
}

/// Declarative import: full replace of the routing config from an export document.
pub async fn import_config(
    State(s): State<AppState>,
    Json(doc): Json<ImportDoc>,
) -> ApiResult<Json<Value>> {
    // Validate wasm modules and certificates *before* the destructive import.
    use base64::Engine;
    for mv in &doc.wasm_modules {
        if let Some(b64) = mv["wasm_base64"].as_str() {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| ApiError::BadRequest(format!("wasm module base64: {e}")))?;
            raahi_proxy::validate_wasm(&bytes).map_err(ApiError::BadRequest)?;
        }
    }
    for cv in &doc.certificates {
        let (cert, key) = (
            cv["cert_pem"].as_str().unwrap_or(""),
            cv["key_pem"].as_str().unwrap_or(""),
        );
        if !key.is_empty() {
            raahi_proxy::validate_cert(cert, key).map_err(|e| {
                ApiError::BadRequest(format!(
                    "certificate '{}': {e}",
                    cv["name"].as_str().unwrap_or("?")
                ))
            })?;
        }
    }
    if let Some(acme) = &doc.acme {
        for account in &acme.accounts {
            raahi_acme::validate_account_credentials(&account.credentials).map_err(|error| {
                ApiError::BadRequest(format!(
                    "ACME account for '{}': {error}",
                    account.directory_url
                ))
            })?;
        }
    }

    let report = s.store.import(&doc).await?;
    reload(&s).await?;
    reload_certs(&s).await?;
    s.acme_handle.trigger();
    Ok(Json(json!(report)))
}
