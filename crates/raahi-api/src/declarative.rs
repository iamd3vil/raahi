//! Declarative config endpoints: `POST /config/apply` reconciles the store
//! against a name-keyed config file (JSON, YAML or HUML); `GET /config/current`
//! renders the current state as one.

use axum::Json;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::http::header::CONTENT_TYPE;
use axum::response::{IntoResponse, Response};
use raahi_core::*;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{ApiError, ApiResult};
use crate::handlers::{
    LISTENER_NOTE, ensure_wasm_module_exists, stored_secret, validate_acme_certificate,
    validate_discovery_source, validate_plugin_config, validate_route_paths,
};
use crate::{AppState, reload, reload_certs};

#[derive(Deserialize)]
pub struct ApplyQuery {
    #[serde(default)]
    dry_run: bool,
}

pub async fn apply(
    State(s): State<AppState>,
    Query(q): Query<ApplyQuery>,
    headers: HeaderMap,
    body: String,
) -> ApiResult<Json<Value>> {
    let format = match headers.get(CONTENT_TYPE) {
        None => ConfigFormat::Json,
        Some(v) => v
            .to_str()
            .ok()
            .and_then(ConfigFormat::from_content_type)
            .ok_or_else(|| {
                ApiError::BadRequest(
                    "Content-Type must be application/json, application/yaml or application/huml"
                        .into(),
                )
            })?,
    };
    let mut doc = parse_config(&body, format)
        .map_err(|e| ApiError::BadRequest(format!("invalid config: {e}")))?;
    prepare(&s, &mut doc).await?;

    let report = s.store.apply_config(&doc, q.dry_run).await?;
    if !report.dry_run && !report.changes.is_empty() {
        reload(&s).await?;
        if report.touched("certificate") {
            reload_certs(&s).await?;
            s.acme_handle.trigger();
        }
        if report.touched("discovery_source") {
            s.discovery_handle.trigger(None).await;
        }
    }
    let mut out = json!(report);
    if report.touched("stream_route") {
        out["note"] = json!(LISTENER_NOTE);
    }
    Ok(Json(out))
}

/// The same per-entity checks the CRUD endpoints run, plus converting credential
/// secrets to their stored form. Runs before the store transaction so bcrypt
/// doesn't hold SQLite's write lock.
async fn prepare(s: &AppState, doc: &mut ConfigDoc) -> ApiResult<()> {
    for svc in doc.services.iter().flatten() {
        for d in &svc.discovery {
            validate_discovery_source(s, &d.spec()).map_err(|e| context(e, &svc.name))?;
        }
    }

    let global = doc
        .plugins
        .iter()
        .flatten()
        .map(|p| (p, "(global)".to_string()));
    let service = doc.services.iter().flatten().flat_map(|s| {
        s.plugins
            .iter()
            .map(|p| (p, format!("on service {}", s.name)))
    });
    let route = doc.routes.iter().flatten().flat_map(|r| {
        r.plugins
            .iter()
            .map(|p| (p, format!("on route {}", r.name)))
    });
    // Collected so no closure-holding iterator lives across the awaits below.
    let plugins: Vec<_> = global.chain(service).chain(route).collect();
    for (p, owner) in plugins {
        let what = format!("plugin {} {owner}", p.plugin_type.as_str());
        validate_plugin_config(p.plugin_type, &p.config).map_err(|e| context(e, &what))?;
        ensure_wasm_module_exists(s, p.plugin_type, &p.config)
            .await
            .map_err(|e| context(e, &what))?;
    }

    for r in doc.routes.iter().flatten() {
        validate_route_paths(&r.paths).map_err(|e| context(e, &format!("route {}", r.name)))?;
        if r.splits.iter().any(|sp| sp.weight < 1) {
            return Err(ApiError::BadRequest(format!(
                "route {}: split weights must be >= 1",
                r.name
            )));
        }
    }
    for r in doc.stream_routes.iter().flatten() {
        if r.listen_addr.parse::<std::net::SocketAddr>().is_err() {
            return Err(ApiError::BadRequest(format!(
                "stream route {}: listen_addr '{}' is not a valid socket address (host:port)",
                r.name, r.listen_addr
            )));
        }
    }

    if let Some(certs) = &doc.certificates {
        let existing = s.store.list_certificates().await?;
        for c in certs {
            let what = format!("certificate {}", c.name);
            if let Some(config) = &c.acme_config {
                if c.cert_pem.is_some() || c.key_pem.is_some() {
                    return Err(ApiError::BadRequest(format!(
                        "{what}: ACME certificates must not include cert_pem or key_pem"
                    )));
                }
                validate_acme_certificate(s, &c.sni, config)
                    .await
                    .map_err(|e| context(e, &what))?;
            } else if c.cert_pem.is_some() || c.key_pem.is_some() {
                // Validate the pair that will be stored, filling an omitted half
                // from the existing manual certificate.
                let stored = existing
                    .iter()
                    .find(|e| e.name == c.name && e.acme_config.is_none());
                let cert = c
                    .cert_pem
                    .as_deref()
                    .or(stored.map(|e| e.cert_pem.as_str()));
                let key = c.key_pem.as_deref().or(stored.map(|e| e.key_pem.as_str()));
                raahi_proxy::validate_cert(cert.unwrap_or(""), key.unwrap_or(""))
                    .map_err(|e| ApiError::BadRequest(format!("{what}: {e}")))?;
            }
        }
    }

    if let Some(consumers) = &mut doc.consumers {
        let existing = s.store.list_all_credentials().await?;
        for c in consumers {
            for cr in &mut c.credentials {
                let what = format!(
                    "{} credential for {}",
                    cr.credential_type.as_str(),
                    c.username
                );
                if cr.credential_type == CredentialType::KeyAuth {
                    cr.secret = None;
                    continue;
                }
                let Some(secret) = cr.secret.as_deref() else {
                    if cr.algorithm.is_some() {
                        return Err(ApiError::BadRequest(format!(
                            "{what}: algorithm needs a secret"
                        )));
                    }
                    continue; // keep the stored secret
                };
                // Re-hashing would change the stored bcrypt hash on every apply;
                // keep it while the password still matches.
                let current = existing.iter().find(|e| {
                    e.credential_type == cr.credential_type && e.identifier == cr.identifier
                });
                let unchanged = cr.credential_type == CredentialType::BasicAuth
                    && current
                        .and_then(|e| e.secret.as_deref())
                        .is_some_and(|hash| bcrypt::verify(secret, hash).unwrap_or(false));
                cr.secret = if unchanged {
                    current.and_then(|e| e.secret.clone())
                } else {
                    stored_secret(cr.credential_type, Some(secret), cr.algorithm.as_deref())
                        .map_err(|e| context(e, &what))?
                };
                cr.algorithm = None;
            }
        }
    }
    Ok(())
}

/// Prefix a validation error with the entity it came from.
fn context(error: ApiError, what: &str) -> ApiError {
    match error {
        ApiError::BadRequest(m) => ApiError::BadRequest(format!("{what}: {m}")),
        other => other,
    }
}

#[derive(Deserialize)]
pub struct CurrentQuery {
    format: Option<String>,
}

pub async fn current(
    State(s): State<AppState>,
    Query(q): Query<CurrentQuery>,
) -> ApiResult<Response> {
    let format = match q.format.as_deref() {
        None => ConfigFormat::Yaml,
        Some(name) => ConfigFormat::from_name(name)
            .ok_or_else(|| ApiError::BadRequest("format must be one of json, yaml, huml".into()))?,
    };
    let doc = s.store.current_config().await?;
    let text = render_config(&doc, format).map_err(ApiError::Internal)?;
    Ok(([(CONTENT_TYPE, format.content_type())], text).into_response())
}
