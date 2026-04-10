use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use engram_lib::services::axon::{
    get_event, get_stats as get_axon_stats, list_channels, publish_event, query_events,
    PublishEventRequest,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/axon/publish", post(publish_event_handler))
        .route("/axon/events", get(list_events_handler))
        .route("/axon/events/{id}", get(get_event_handler))
        .route("/axon/channels", get(list_channels_handler))
        .route("/axon/stats", get(get_stats))
}

#[derive(Debug, Deserialize)]
struct PublishBody {
    channel: String,
    /// The plan spec says event_type but the lib uses `action`
    action: Option<String>,
    event_type: Option<String>,
    payload: Option<serde_json::Value>,
    source: Option<String>,
    agent: Option<String>,
}

#[derive(Debug, Deserialize)]
struct QueryEventsParams {
    channel: Option<String>,
    event_type: Option<String>,
    action: Option<String>,
    source: Option<String>,
    since_id: Option<i64>,
    limit: Option<usize>,
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
    let limit = params.limit.unwrap_or(100);
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
