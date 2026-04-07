use axum::{routing::get, Json, Router};
use serde_json::{json, Value};

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/health", get(get_health))
        .route("/live", get(get_health))
        .route("/ready", get(get_health))
}

async fn get_health() -> Json<Value> {
    Json(json!({ "status": "ok", "service": "engram" }))
}
