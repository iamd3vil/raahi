//! Admin API error type and its mapping to HTTP responses.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use raahi_store::StoreError;
use serde_json::json;

#[derive(Debug)]
pub enum ApiError {
    Store(StoreError),
    NotFound,
    BadRequest(String),
    Internal(String),
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::NotFound => ApiError::NotFound,
            StoreError::Conflict(m) => ApiError::BadRequest(format!("conflict: {m}")),
            StoreError::Invalid(m) => ApiError::BadRequest(m),
            other => ApiError::Store(other),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::Store(e) => {
                tracing::error!("store error: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error".to_string(),
                )
            }
            ApiError::Internal(m) => {
                tracing::error!("internal error: {m}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error".to_string(),
                )
            }
        };
        (status, Json(json!({ "error": msg }))).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
