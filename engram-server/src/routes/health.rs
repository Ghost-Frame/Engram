use axum::{routing::get, Json, Router};
use serde_json::{json, Value};

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/health", get(get_health))
        .route("/live", get(get_live))
        .route("/ready", get(get_ready))
}

async fn get_health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "engram",
        "version": "0.1.0"
    }))
}

async fn get_live() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "engram",
        "version": "0.1.0"
    }))
}

async fn get_ready() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "engram",
        "version": "0.1.0"
    }))
}
