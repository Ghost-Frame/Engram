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

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self.0 {
            EngError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            EngError::InvalidInput(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            EngError::Auth(msg) => (StatusCode::UNAUTHORIZED, msg.clone()),
            EngError::Database(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            EngError::Serialization(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            EngError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}
