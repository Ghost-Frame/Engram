mod sse;
mod types;

use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::{Auth, ResolvedDb};
use crate::state::AppState;
use kleos_lib::services::axon::fanout::{deliver_webhooks, get_webhook_targets};
use kleos_lib::services::axon::{
    consume, delete_subscription, ensure_channel, get_cursor, get_event,
    get_stats as get_axon_stats, list_channels, list_subscriptions_for_agent, publish_event,
    query_events, upsert_subscription, PublishEventRequest, SubscribeRequest,
};
use kleos_lib::EngError;
use types::{
    CreateChannelBody, GetCursorParams, ListSubscriptionsParams, PollBody, PublishBody,
    QueryEventsParams, SubscribeBody, UnsubscribeBody,
};

/// Default body-size cap on `POST /axon/publish`. Mirrors the standalone
/// `BODY_MAX_BYTES` (64 KB). Override with `AXON_BODY_MAX_BYTES` at startup.
const DEFAULT_PUBLISH_BODY_BYTES: usize = 64 * 1024;

/// Read the publish body size cap from `AXON_BODY_MAX_BYTES` or fall back to the compiled default.
fn publish_body_limit() -> usize {
    std::env::var("AXON_BODY_MAX_BYTES")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_PUBLISH_BODY_BYTES)
}

/// Builds the Axon event bus router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/axon/publish",
            post(publish_event_handler).layer(DefaultBodyLimit::max(publish_body_limit())),
        )
        .route("/axon/events", get(list_events_handler))
        .route("/axon/events/{id}", get(get_event_handler))
        .route(
            "/axon/channels",
            get(list_channels_handler).post(create_channel_handler),
        )
        .route(
            "/axon/subscribe",
            post(subscribe_handler).delete(unsubscribe_handler),
        )
        .route("/axon/subscriptions", get(list_subscriptions_handler))
        .route("/axon/poll", post(poll_handler))
        .route("/axon/cursor", get(get_cursor_handler))
        .route("/axon/stats", get(get_stats))
        .route("/axon/health", get(health_handler))
        .route("/axon/stream", get(sse::stream_handler))
}

/// Publishes a new event, broadcasts to SSE subscribers, and fans out to webhooks.
async fn publish_event_handler(
    State(state): State<AppState>,
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Json(body): Json<PublishBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    // Support both `action` and `event_type` field names
    let action = body
        .action
        .or(body.event_type)
        .unwrap_or_else(|| "event".to_string());

    let req = PublishEventRequest {
        channel: body.channel,
        action,
        payload: body.payload,
        source: body.source,
        agent: body.agent,
        user_id: Some(auth.user_id),
    };

    let event = publish_event(&db, req).await?;
    let event_json = serde_json::to_value(&event).map_err(EngError::Serialization)?;

    // Broadcast to SSE subscribers (ignore error = no receivers)
    let _ = state.axon_broadcast.send(event_json.clone());

    // Fan out to webhook subscribers via tracked background task
    let db_clone = db.clone();
    let channel = event.channel.clone();
    let action = event.action.clone();
    let shutdown = state.shutdown_token.clone();
    let fanout_json = event_json.clone();
    state.background_tasks.lock().await.spawn(async move {
        tokio::select! {
            _ = shutdown.cancelled() => {}
            _ = async {
                if let Ok(targets) = get_webhook_targets(&db_clone, &channel, &action).await {
                    deliver_webhooks(&targets, &fanout_json);
                }
            } => {}
        }
    });

    Ok((StatusCode::CREATED, Json(event_json)))
}

/// Lists events with optional filters.
async fn list_events_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Query(params): Query<QueryEventsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(100).min(1000);
    // Support both `action` and `event_type` field names
    let action = params.action.or(params.event_type);

    let events = query_events(
        &db,
        params.channel.as_deref(),
        action.as_deref(),
        params.source.as_deref(),
        limit,
        0,
        auth.user_id,
    )
    .await?;

    // Filter by since_id if provided
    let events = if let Some(since_id) = params.since_id {
        events
            .into_iter()
            .filter(|e| e.id > since_id)
            .collect::<Vec<_>>()
    } else {
        events
    };

    Ok(Json(json!({ "events": events, "count": events.len() })))
}

/// Retrieves a single event by ID.
async fn get_event_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let event = get_event(&db, id, auth.user_id).await?;
    Ok(Json(json!(event)))
}

/// Lists all channels.
async fn list_channels_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(_auth): Auth,
) -> Result<Json<Value>, AppError> {
    let channels = list_channels(&db).await?;
    Ok(Json(json!({ "channels": channels })))
}

/// Returns Axon statistics.
async fn get_stats(ResolvedDb(db): ResolvedDb, Auth(_auth): Auth) -> Result<Json<Value>, AppError> {
    let stats = get_axon_stats(&db).await?;
    Ok(Json(json!(stats)))
}

/// Public unauthenticated health probe. Mirrors the standalone `/health`
/// surface: returns `{ status: "ok", version }` without auth or a tenant
/// context. Detailed per-tenant stats remain on the authenticated `/stats`.
async fn health_handler() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "axon",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

// --- New handlers for P0-0 Phase 27c ---

// SECURITY: relies on ResolvedDb shard isolation (Phase 5+) to scope to the caller's tenant. Do not add state.db calls here without re-binding auth.
async fn create_channel_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(_auth): Auth,
    Json(body): Json<CreateChannelBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    ensure_channel(&db, body.name.clone(), body.description).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "ok": true, "channel": body.name })),
    ))
}

/// Subscribes an agent to a channel.
async fn subscribe_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Json(body): Json<SubscribeBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    // SECURITY: validate webhook URL before storing to prevent SSRF on delivery.
    if let Some(ref url) = body.webhook_url {
        kleos_lib::webhooks::validate_webhook_url(url)?;
    }
    let req = SubscribeRequest {
        agent: body.agent,
        channel: body.channel,
        filter_type: body.filter_type,
        webhook_url: body.webhook_url,
    };
    let sub = upsert_subscription(&db, req, auth.user_id).await?;
    Ok((StatusCode::CREATED, Json(json!(sub))))
}

/// Unsubscribes from a channel.
async fn unsubscribe_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(_auth): Auth,
    Json(body): Json<UnsubscribeBody>,
) -> Result<Json<Value>, AppError> {
    let deleted = delete_subscription(&db, &body.agent, &body.channel).await?;
    Ok(Json(json!({ "deleted": deleted })))
}

/// Lists subscriptions for an agent.
async fn list_subscriptions_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Query(params): Query<ListSubscriptionsParams>,
) -> Result<Json<Value>, AppError> {
    let subs = list_subscriptions_for_agent(&db, &params.agent, auth.user_id).await?;
    Ok(Json(json!({ "subscriptions": subs, "count": subs.len() })))
}

/// Polls for new events.
async fn poll_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Json(body): Json<PollBody>,
) -> Result<Json<Value>, AppError> {
    let limit = body.limit.unwrap_or(100).min(1000);
    let events = consume(&db, &body.agent, &body.channel, limit, auth.user_id).await?;
    let cursor = get_cursor(&db, &body.agent, &body.channel, auth.user_id).await?;
    Ok(Json(json!({
        "events": events,
        "cursor": cursor.last_event_id,
        "count": events.len()
    })))
}

/// Retrieves cursor position.
async fn get_cursor_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Query(params): Query<GetCursorParams>,
) -> Result<Json<Value>, AppError> {
    let cursor = get_cursor(&db, &params.agent, &params.channel, auth.user_id).await?;
    Ok(Json(json!(cursor)))
}
