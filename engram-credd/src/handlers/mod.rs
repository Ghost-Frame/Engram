//! HTTP request handlers for credd.

pub mod agents;
pub mod resolve;
pub mod secrets;

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
            CredError::Encryption(msg) => {
                (StatusCode::INTERNAL_SERVER_ERROR, format!("encryption: {}", msg))
            }
            CredError::Decryption(msg) => {
                (StatusCode::INTERNAL_SERVER_ERROR, format!("decryption: {}", msg))
            }
            CredError::Database(msg) => {
                (StatusCode::INTERNAL_SERVER_ERROR, format!("database: {}", msg))
            }
            CredError::YubiKey(msg) => {
                (StatusCode::INTERNAL_SERVER_ERROR, format!("yubikey: {}", msg))
            }
        };

        let body = Json(json!({
            "error": message,
        }));

        (status, body).into_response()
    }
}
