use axum::{routing::{get, post}, Json, Router};
use serde_json::{json, Value};

pub fn router() -> Router {
    Router::new()
        .route("/evaluate", post(evaluate))
        .route("/rules", get(list_rules))
        .route("/rules", post(create_rule))
}

async fn evaluate() -> Json<Value> {
    Json(json!({ "allowed": true, "triggered_rules": [] }))
}

async fn list_rules() -> Json<Value> {
    Json(json!({ "rules": [] }))
}

async fn create_rule() -> Json<Value> {
    Json(json!({ "status": "not implemented" }))
}
