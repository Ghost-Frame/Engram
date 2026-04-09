use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/docs", get(docs_index))
        .route("/docs/openapi", get(openapi_stub))
        .route("/docs/routes", get(route_catalog))
}

async fn docs_index() -> Json<Value> {
    Json(json!({
        "service": "engram",
        "docs": {
            "openapi": "/docs/openapi",
            "routes": "/docs/routes"
        }
    }))
}

async fn openapi_stub() -> Json<Value> {
    Json(json!({
        "openapi": "3.0.0",
        "info": { "title": "Engram API", "version": "0.1.0" },
        "paths": {}
    }))
}

async fn route_catalog() -> Json<Value> {
    Json(json!({
        "families": [
            "health", "memory", "episodes", "conversations", "graph", "intelligence",
            "tasks", "axon", "broca", "soma", "thymus", "loom",
            "security", "webhooks", "skills", "personality", "projects", "prompts",
            "context", "brain", "inbox", "ingestion", "pack", "scratchpad",
            "agents", "artifacts", "auth-keys", "fsrs", "grounding", "search", "docs", "onboard"
        ]
    }))
}
