//! HTTP request handlers for credd.

pub mod agents;
pub mod resolve;
pub mod secrets;
pub mod types;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use engram_cred::CredError;

/// Convert CredError to HTTP response.
pub struct AppError(CredError);

impl From<CredError> for AppError {
    fn from(e: CredError) -> Self {
        Self(e)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self.0 {
            CredError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            CredError::AuthFailed(msg) => (StatusCode::UNAUTHORIZED, msg.clone()),
            CredError::PermissionDenied(msg) => (StatusCode::FORBIDDEN, msg.clone()),
            CredError::KeyRevoked(msg) => (StatusCode::FORBIDDEN, msg.clone()),
            CredError::InvalidInput(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            // SECURITY (SEC-LOW-7): internal error variants must NOT leak
            // implementation details (DB engine, encryption backend, YubiKey
            // state) to API consumers. Log the real error server-side only.
            CredError::Encryption(msg) => {
                tracing::error!("encryption error: {}", msg);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "an internal error occurred".to_string(),
                )
            }
            CredError::Decryption(msg) => {
                tracing::error!("decryption error: {}", msg);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "an internal error occurred".to_string(),
                )
            }
            CredError::Database(msg) => {
                tracing::error!("database error: {}", msg);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "an internal error occurred".to_string(),
                )
            }
            CredError::YubiKey(msg) => {
                tracing::error!("yubikey error: {}", msg);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "an internal error occurred".to_string(),
                )
            }
        };

        let body = Json(json!({
            "error": message,
        }));

        (status, body).into_response()
    }
}
