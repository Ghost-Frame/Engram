use axum::{routing::get, Json, Router};
use serde_json::{json, Value};

pub fn router() -> Router {
    Router::new()
        .route("/chiasm", get(list_tasks))
        .route("/axon", get(list_events))
        .route("/soma", get(list_agents))
        .route("/broca", get(list_actions))
}

async fn list_tasks() -> Json<Value> {
    Json(json!({ "tasks": [] }))
}

async fn list_events() -> Json<Value> {
    Json(json!({ "events": [] }))
}

async fn list_agents() -> Json<Value> {
    Json(json!({ "agents": [] }))
}

async fn list_actions() -> Json<Value> {
    Json(json!({ "actions": [] }))
}
