use axum::{routing::get, Json, Router};
use serde_json::{json, Value};

pub fn router() -> Router {
    Router::new()
        .route("/", get(get_graph))
        .route("/search", get(graph_search))
        .route("/:id/neighborhood", get(neighborhood))
}

async fn get_graph() -> Json<Value> {
    Json(json!({ "nodes": [], "edges": [] }))
}

async fn graph_search() -> Json<Value> {
    Json(json!({ "nodes": [] }))
}

async fn neighborhood() -> Json<Value> {
    Json(json!({ "nodes": [], "edges": [] }))
}
