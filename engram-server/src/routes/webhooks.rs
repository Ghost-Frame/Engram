use axum::{routing::{delete, get, post}, Json, Router};
use serde_json::{json, Value};

pub fn router() -> Router {
    Router::new()
        .route("/", post(create_webhook))
        .route("/", get(list_webhooks))
        .route("/:id", delete(delete_webhook))
}

async fn create_webhook() -> Json<Value> {
    Json(json!({ "status": "not implemented" }))
}

async fn list_webhooks() -> Json<Value> {
    Json(json!({ "webhooks": [] }))
}

async fn delete_webhook() -> Json<Value> {
    Json(json!({ "status": "not implemented" }))
}
