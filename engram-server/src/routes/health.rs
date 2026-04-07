use axum::{routing::get, Json, Router};
use serde_json::{json, Value};

pub fn router() -> Router {
    Router::new().route("/", get(get_health))
}

async fn get_health() -> Json<Value> {
    Json(json!({ "status": "ok", "service": "engram" }))
}
