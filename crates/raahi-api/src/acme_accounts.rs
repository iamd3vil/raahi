//! Registration settings. Secrets never appear in ordinary API responses.
use crate::{ApiError, ApiResult, AppState};
use axum::{
    Json,
    extract::{Query, State},
};
use raahi_core::AcmeEabCredentials;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
pub struct DirectoryQuery {
    pub directory_url: String,
}

pub async fn status(
    State(state): State<AppState>,
    Query(query): Query<DirectoryQuery>,
) -> ApiResult<Json<Value>> {
    raahi_acme::validate_directory_url(&query.directory_url)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let directory = raahi_acme::normalize_directory_url(&query.directory_url);
    Ok(Json(json!({
        "configured": state.store.get_acme_eab(directory).await?.is_some(),
        "account_registered": state.store.get_acme_account(directory).await?.is_some(),
    })))
}

pub async fn save(
    State(state): State<AppState>,
    Json(credentials): Json<AcmeEabCredentials>,
) -> ApiResult<Json<Value>> {
    let credentials = raahi_acme::normalize_eab_credentials(&credentials)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    state.store.set_acme_eab(&credentials).await?;
    state.acme_handle.trigger();
    Ok(Json(json!({"configured": true})))
}

pub async fn remove(
    State(state): State<AppState>,
    Query(query): Query<DirectoryQuery>,
) -> ApiResult<Json<Value>> {
    raahi_acme::validate_directory_url(&query.directory_url)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    state
        .store
        .delete_acme_eab(raahi_acme::normalize_directory_url(&query.directory_url))
        .await?;
    Ok(Json(json!({"configured": false})))
}
