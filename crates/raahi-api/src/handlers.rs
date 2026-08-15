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
use crate::{reload, reload_certs, AppState};

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
    let p = s.store.update_plugin(id, &spec).await?.ok_or(ApiError::NotFound)?;
    reload(&s).await?;
    Ok(Json(p))
}

/// A `wasm` plugin must reference an uploaded module by name.
async fn ensure_wasm_module_exists(s: &AppState, spec: &PluginSpec) -> ApiResult<()> {
    if spec.plugin_type != PluginType::Wasm {
        return Ok(());
    }
    let name = spec.config["module"].as_str().unwrap_or("");
    let exists = s.store.list_wasm_modules().await?.iter().any(|m| m.name == name);
    if !exists {
        return Err(ApiError::BadRequest(format!("wasm module '{name}' not found")));
    }
    Ok(())
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
            return Err(ApiError::BadRequest("service-scoped plugin needs service_id".into()));
        }
        PluginScope::Route if spec.route_id.is_none() => {
            return Err(ApiError::BadRequest("route-scoped plugin needs route_id".into()));
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
            let both_empty = spec.config["allow"].as_array().map_or(true, |a| a.is_empty())
                && spec.config["deny"].as_array().map_or(true, |a| a.is_empty());
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
        _ => {}
    }
    Ok(())
}

// ---- consumers + credentials -------------------------------------------------
pub async fn list_consumers(State(s): State<AppState>) -> ApiResult<Json<Vec<Consumer>>> {
    Ok(Json(s.store.list_consumers().await?))
}

pub async fn get_consumer(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> ApiResult<Json<Consumer>> {
    s.store.get_consumer(id).await?.map(Json).ok_or(ApiError::NotFound)
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
    // basic-auth secrets are hashed (bcrypt) before storage; key-auth has no secret;
    // jwt stores its verification material as a JSON blob (needed raw to verify).
    let stored = match spec.credential_type {
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
        .create_credential(consumer_id, spec.credential_type, &spec.identifier, stored.as_deref())
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

// ---- wasm modules --------------------------------------------------------------
pub async fn list_wasm_modules(State(s): State<AppState>) -> ApiResult<Json<Value>> {
    let mods = s.store.list_wasm_modules().await?;
    Ok(Json(json!(mods
        .iter()
        .map(|m| json!({
            "id": m.id,
            "name": m.name,
            "description": m.description,
            "size_bytes": m.wasm.len(),
            "created_at": m.created_at,
        }))
        .collect::<Vec<_>>())))
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
            ))
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
    // Validate the PEM actually parses (cert + private key) before storing.
    raahi_proxy::validate_cert(&spec.cert_pem, &spec.key_pem).map_err(ApiError::BadRequest)?;
    let c = s.store.create_certificate(&spec).await?;
    // Live-swap the cert store (no restart) if the HTTPS listener is already bound.
    reload_certs(&s).await?;
    Ok(Json(c))
}

pub async fn delete_certificate(State(s): State<AppState>, Path(id): Path<Id>) -> ApiResult<Json<Value>> {
    if !s.store.delete_certificate(id).await? {
        return Err(ApiError::NotFound);
    }
    reload_certs(&s).await?;
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
    // The default certificate (active_certificate_id) may have changed — swap it live.
    reload_certs(&s).await?;
    Ok(Json(settings))
}

// ---- admin auth ----------------------------------------------------------------

/// SHA-256 hex digest (admin tokens are high-entropy, so a fast hash is fine).
pub(crate) fn sha256_hex(data: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(data.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether admin auth is enabled (used by the UI before login; unauthenticated).
pub async fn admin_status(State(s): State<AppState>) -> Json<Value> {
    Json(json!({ "auth_enabled": s.admin_hash.load().is_some() }))
}

/// Generate (or rotate) the admin token. The plaintext is returned exactly once.
pub async fn create_admin_token(State(s): State<AppState>) -> ApiResult<Json<Value>> {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let hash = sha256_hex(&token);
    s.store.set_admin_token_hash(Some(&hash)).await?;
    s.admin_hash.store(std::sync::Arc::new(Some(hash)));
    Ok(Json(json!({
        "token": token,
        "note": "Store this token now — it is not retrievable later.",
    })))
}

/// Disable admin auth (open the API again; loopback binding still applies).
pub async fn delete_admin_token(State(s): State<AppState>) -> ApiResult<Json<Value>> {
    s.store.set_admin_token_hash(None).await?;
    s.admin_hash.store(std::sync::Arc::new(None));
    Ok(Json(json!({ "auth_enabled": false })))
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
}
fn default_method() -> String {
    "GET".into()
}

/// Dry-run the router: which route/service would this request hit?
pub async fn router_test(
    State(s): State<AppState>,
    Query(q): Query<RouterTestQuery>,
) -> Json<Value> {
    let rc = s.config.load();
    match rc.data.match_route(&q.host, &q.path, &q.method.to_uppercase()) {
        Some(m) => {
            let plugins: Vec<&str> = {
                let mut ordered = rc.data.plugins_for(m.route.id, m.service.id);
                ordered.sort_by_key(|p| p.ordering);
                ordered.iter().map(|p| p.plugin_type.as_str()).collect()
            };
            Json(json!({
                "matched": true,
                "route_id": m.route.id,
                "route_name": m.route.name,
                "service_id": m.service.id,
                "service_name": m.service.name,
                "matched_prefix": m.matched_prefix,
                "strip_path": m.route.strip_path,
                "preserve_host": m.route.preserve_host,
                "plugins": plugins,
            }))
        }
        None => Json(json!({ "matched": false })),
    }
}

/// Export the full configuration as one JSON document (secrets excluded).
pub async fn export_config(State(s): State<AppState>) -> ApiResult<Json<Value>> {
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
        consumers_out.push(json!({ "consumer": c, "credentials": creds }));
    }
    Ok(Json(json!({
        "raahi_export_version": 1,
        "settings": s.store.get_settings().await?,
        "services": services_out,
        "routes": s.store.list_routes().await?,
        "plugins": s.store.list_plugins().await?,
        "consumers": consumers_out,
        // key_pem is never serialized; cert_pem is public material.
        "certificates": s.store.list_certificates().await?,
    })))
}
