use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::post,
    Json, Router,
};
use kleos_lib::webhooks::{self, CreateWebhookRequest};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{error::AppError, extractors::Auth, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/webhooks",
            post(create_webhook_handler).get(list_webhooks_handler),
        )
        .route(
            "/webhooks/{id}",
            axum::routing::delete(delete_webhook_handler),
        )
        .route("/webhooks/test/{id}", post(test_webhook_handler))
}

async fn create_webhook_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(mut req): Json<CreateWebhookRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    req.user_id = Some(auth.user_id);
    let webhook = webhooks::create_webhook(&state.db, req).await?;
    Ok((StatusCode::CREATED, Json(json!(webhook))))
}

async fn list_webhooks_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let items = webhooks::list_webhooks(&state.db, auth.user_id).await?;
    Ok(Json(json!({ "webhooks": items, "count": items.len() })))
}

async fn delete_webhook_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    webhooks::delete_webhook(&state.db, id).await?;
    Ok(Json(json!({ "deleted": true, "id": id })))
}

#[derive(Debug, Deserialize)]
struct TestWebhookBody {
    event: Option<String>,
}

async fn test_webhook_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<TestWebhookBody>,
) -> Result<Json<Value>, AppError> {
    let _webhook = webhooks::get_webhook(&state.db, id).await?;
    let event = body.event.as_deref().unwrap_or("test");
    let payload = json!({ "webhook_id": id, "test": true });
    let dispatched = webhooks::dispatch(&state.db, auth.user_id, event, &payload).await?;
    Ok(Json(json!({ "dispatched": dispatched, "event": event })))
}
