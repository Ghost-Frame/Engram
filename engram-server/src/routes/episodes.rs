use axum::{routing::{delete, get, post, put}, Json, Router};
use serde_json::{json, Value};

pub fn router() -> Router {
    Router::new()
        .route("/", post(create_episode))
        .route("/", get(list_episodes))
        .route("/:id", get(get_episode))
        .route("/:id", put(update_episode))
        .route("/:id", delete(delete_episode))
}

async fn create_episode() -> Json<Value> {
    Json(json!({ "status": "not implemented" }))
}

async fn list_episodes() -> Json<Value> {
    Json(json!({ "episodes": [] }))
}

async fn get_episode() -> Json<Value> {
    Json(json!({ "status": "not implemented" }))
}

async fn update_episode() -> Json<Value> {
    Json(json!({ "status": "not implemented" }))
}

async fn delete_episode() -> Json<Value> {
    Json(json!({ "status": "not implemented" }))
}
