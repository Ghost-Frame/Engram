use axum::{routing::get, Json, Router};
use serde_json::{json, Value};

pub fn router() -> Router {
    Router::new()
        .route("/stats", get(stats))
        .route("/audit", get(audit_log))
        .route("/keys", get(list_keys))
}

async fn stats() -> Json<Value> {
    Json(json!({ "status": "not implemented" }))
}

async fn audit_log() -> Json<Value> {
    Json(json!({ "entries": [] }))
}

async fn list_keys() -> Json<Value> {
    Json(json!({ "keys": [] }))
}
