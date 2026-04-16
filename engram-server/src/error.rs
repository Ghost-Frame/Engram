use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use engram_lib::EngError;
use serde_json::json;

pub struct AppError(pub EngError);

impl From<EngError> for AppError {
    fn from(e: EngError) -> Self {
        AppError(e)
    }
}

/// Classify a rusqlite error for HTTP status code selection.
fn classify_db_error_message(msg: &str) -> Option<(StatusCode, String)> {
    if msg.contains("UNIQUE constraint failed") {
        Some((StatusCode::CONFLICT, "Resource already exists".to_string()))
    } else if msg.contains("NOT NULL constraint failed") {
        Some((
            StatusCode::BAD_REQUEST,
            "Required field is missing".to_string(),
        ))
    } else if msg.contains("FOREIGN KEY constraint failed") {
        Some((
            StatusCode::BAD_REQUEST,
            "Referenced resource does not exist".to_string(),
        ))
    } else if msg.contains("CHECK constraint failed") {
        Some((
            StatusCode::BAD_REQUEST,
            "Value violates constraint".to_string(),
        ))
    } else {
        None
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self.0 {
            EngError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            EngError::InvalidInput(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            EngError::Auth(msg) => (StatusCode::UNAUTHORIZED, msg.clone()),
            EngError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg.clone()),
            EngError::NotImplemented(msg) => (StatusCode::NOT_IMPLEMENTED, msg.clone()),
            // Classify DB errors: constraint violations get 4xx, others get 500.
            EngError::Database(e) => {
                let msg = e.to_string();
                if let Some(classified) = classify_db_error_message(&msg) {
                    tracing::warn!("Database constraint error: {}", msg);
                    classified
                } else {
                    tracing::error!("Database error: {}", msg);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Database error".to_string(),
                    )
                }
            }
            EngError::DatabaseMessage(msg) => {
                if let Some(classified) = classify_db_error_message(msg) {
                    tracing::warn!("Database constraint error: {}", msg);
                    classified
                } else {
                    tracing::error!("Database error: {}", msg);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Database error".to_string(),
                    )
                }
            }
            EngError::Serialization(e) => {
                tracing::error!("Serialization error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal error".to_string(),
                )
            }
            EngError::Internal(msg) => {
                // SECURITY: internal errors may carry filesystem paths, library
                // version strings, or database row details. Log server-side at
                // error level and return a generic message to the client.
                tracing::error!("Internal error: {}", msg);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal server error".to_string(),
                )
            }
            EngError::Encryption(msg) => {
                tracing::error!("Encryption error: {}", msg);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Encryption error".to_string(),
                )
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}
