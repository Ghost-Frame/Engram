use axum::{
    routing::{delete, get, post, put},
    Json, Router,
};
use serde_json::{json, Value};

pub fn router() -> Router {
    Router::new()
        .route("/", post(store_memory))
        .route("/", get(list_memories))
        .route("/search", get(search_memories))
        .route("/:id", get(get_memory))
        .route("/:id", put(update_memory))
        .route("/:id", delete(delete_memory))
}

async fn store_memory() -> Json<Value> {
    Json(json!({ "status": "not implemented" }))
}

async fn list_memories() -> Json<Value> {
    Json(json!({ "memories": [] }))
}

async fn search_memories() -> Json<Value> {
    Json(json!({ "results": [] }))
}

async fn get_memory() -> Json<Value> {
    Json(json!({ "status": "not implemented" }))
}

async fn update_memory() -> Json<Value> {
    Json(json!({ "status": "not implemented" }))
}

async fn delete_memory() -> Json<Value> {
    Json(json!({ "status": "not implemented" }))
}
