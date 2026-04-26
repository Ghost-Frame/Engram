mod types;

use axum::{
    extract::{Path, Query},
    http::StatusCode,
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};

use crate::{
    error::AppError,
    extractors::{Auth, ResolvedDb},
    state::AppState,
};
use types::{CreateWebhookBody, DeadLetterQuery, TestWebhookBody};

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
        .route(
            "/webhooks/{id}/dead-letters",
            axum::routing::get(list_dead_letters_handler),
        )
}

async fn create_webhook_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Json(body): Json<CreateWebhookBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let events = body.events.unwrap_or_else(|| vec!["*".to_string()]);
    let (id, created_at) = kleos_lib::webhooks::create_webhook(
        &db,
        &body.url,
        &events,
        body.secret.as_deref(),
        auth.user_id,
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "created": true,
            "id": id,
            "url": body.url,
            "events": events,
            "created_at": created_at
        })),
    ))
}

async fn list_webhooks_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let items = kleos_lib::webhooks::list_webhooks(&db, auth.user_id).await?;
    Ok(Json(json!({ "webhooks": items, "count": items.len() })))
}

async fn delete_webhook_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    kleos_lib::webhooks::delete_webhook(&db, id).await?;
    Ok(Json(json!({ "deleted": true, "id": id })))
}

async fn test_webhook_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<TestWebhookBody>,
) -> Result<Json<Value>, AppError> {
    let exists = kleos_lib::webhooks::list_webhooks(&db, auth.user_id)
        .await?
        .into_iter()
        .any(|hook| hook.id == id);
    if !exists {
        return Err(AppError(kleos_lib::EngError::NotFound(format!(
            "webhook {} not found",
            id
        ))));
    }
    let event = body.event.as_deref().unwrap_or("test");
    let payload = json!({ "webhook_id": id, "test": true });
    kleos_lib::webhooks::emit_webhook_event(&db, event, &payload, auth.user_id).await;
    Ok(Json(json!({ "dispatched": true, "event": event })))
}

async fn list_dead_letters_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
    Query(query): Query<DeadLetterQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = query.limit.unwrap_or(50).min(200);
    let items = kleos_lib::webhooks::list_dead_letters(&db, id, limit).await?;
    Ok(Json(
        json!({ "dead_letters": items, "count": items.len(), "webhook_id": id }),
    ))
}
