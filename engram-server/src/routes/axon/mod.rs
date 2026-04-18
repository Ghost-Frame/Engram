mod types;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use engram_lib::services::axon::{
    consume, delete_subscription, ensure_channel, get_cursor, get_event,
    get_stats as get_axon_stats, list_channels, list_subscriptions_for_agent, publish_event,
    query_events, upsert_subscription, PublishEventRequest, SubscribeRequest,
};
use types::{
    CreateChannelBody, GetCursorParams, ListSubscriptionsParams, PollBody, PublishBody,
    QueryEventsParams, SubscribeBody, UnsubscribeBody,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/axon/publish", post(publish_event_handler))
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
}

async fn publish_event_handler(
    State(state): State<AppState>,
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

    let event = publish_event(&state.db, req).await?;
    Ok((StatusCode::CREATED, Json(json!(event))))
}

async fn list_events_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<QueryEventsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(100).min(1000);
    // Support both `action` and `event_type` field names
    let action = params.action.or(params.event_type);

    let events = query_events(
        &state.db,
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

async fn get_event_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let event = get_event(&state.db, id, auth.user_id).await?;
    Ok(Json(json!(event)))
}

async fn list_channels_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let channels = list_channels(&state.db, auth.user_id).await?;
    Ok(Json(json!({ "channels": channels })))
}

async fn get_stats(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let stats = get_axon_stats(&state.db, Some(auth.user_id)).await?;
    Ok(Json(json!(stats)))
}

// --- New handlers for P0-0 Phase 27c ---

async fn create_channel_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Json(body): Json<CreateChannelBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    ensure_channel(&state.db, body.name.clone(), body.description).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "ok": true, "channel": body.name })),
    ))
}

async fn subscribe_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<SubscribeBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    // SECURITY: validate webhook URL before storing to prevent SSRF on delivery.
    if let Some(ref url) = body.webhook_url {
        engram_lib::webhooks::validate_webhook_url(url)?;
    }
    let req = SubscribeRequest {
        agent: body.agent,
        channel: body.channel,
        filter_type: body.filter_type,
        webhook_url: body.webhook_url,
    };
    let sub = upsert_subscription(&state.db, req, auth.user_id).await?;
    Ok((StatusCode::CREATED, Json(json!(sub))))
}

async fn unsubscribe_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<UnsubscribeBody>,
) -> Result<Json<Value>, AppError> {
    let deleted = delete_subscription(&state.db, &body.agent, &body.channel, auth.user_id).await?;
    Ok(Json(json!({ "deleted": deleted })))
}

async fn list_subscriptions_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ListSubscriptionsParams>,
) -> Result<Json<Value>, AppError> {
    let subs = list_subscriptions_for_agent(&state.db, &params.agent, auth.user_id).await?;
    Ok(Json(json!({ "subscriptions": subs, "count": subs.len() })))
}

async fn poll_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<PollBody>,
) -> Result<Json<Value>, AppError> {
    let limit = body.limit.unwrap_or(100).min(1000);
    let events = consume(&state.db, &body.agent, &body.channel, limit, auth.user_id).await?;
    let cursor = get_cursor(&state.db, &body.agent, &body.channel, auth.user_id).await?;
    Ok(Json(json!({
        "events": events,
        "cursor": cursor.last_event_id,
        "count": events.len()
    })))
}

async fn get_cursor_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<GetCursorParams>,
) -> Result<Json<Value>, AppError> {
    let cursor = get_cursor(&state.db, &params.agent, &params.channel, auth.user_id).await?;
    Ok(Json(json!(cursor)))
}
