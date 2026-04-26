use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use kleos_lib::services::broca::{
    get_action, get_stats as get_broca_stats, log_action, query_actions, LogActionRequest,
};

mod types;
use types::{LogActionBody, QueryActionsParams};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/broca/actions",
            post(log_action_handler).get(list_actions_handler),
        )
        .route("/broca/actions/{id}", get(get_action_handler))
        .route("/broca/feed", get(get_feed_handler))
        .route("/broca/stats", get(get_stats))
}

async fn log_action_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<LogActionBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let action = body.action.clone().unwrap_or_else(|| "unknown".to_string());

    let narrative = body.narrative.or(body.summary).or(body.detail);

    let mut payload = body.payload.or(body.metadata);
    if let Some(project) = body.project {
        let obj = payload.get_or_insert_with(|| serde_json::Value::Object(Default::default()));
        if let Some(map) = obj.as_object_mut() {
            map.entry("project")
                .or_insert(serde_json::Value::String(project));
        }
    }

    let req = LogActionRequest {
        agent: body.agent,
        service: body.service,
        action,
        narrative,
        payload,
        axon_event_id: body.axon_event_id,
        user_id: Some(auth.user_id),
    };

    let entry = log_action(&state.db, req).await?;
    Ok((StatusCode::CREATED, Json(json!(entry))))
}

async fn list_actions_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<QueryActionsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(100).min(1000);
    let offset = params.offset.unwrap_or(0);

    let agent = params.agent.as_deref();
    let service = params.service.as_deref();
    let action = params.action.as_deref();

    let mut entries = query_actions(
        &state.db,
        agent,
        service,
        action,
        limit,
        offset,
        auth.user_id,
    )
    .await?;

    // Apply since filter in-memory if provided
    if let Some(ref since) = params.since {
        entries.retain(|e| e.created_at.as_str() >= since.as_str());
    }

    Ok(Json(json!({ "actions": entries, "count": entries.len() })))
}

async fn get_action_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let entry = get_action(&state.db, id, auth.user_id).await?;
    Ok(Json(json!(entry)))
}

async fn get_feed_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<QueryActionsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(100).min(1000);
    let offset = params.offset.unwrap_or(0);
    let agent = params.agent.as_deref();

    let mut entries =
        query_actions(&state.db, agent, None, None, limit, offset, auth.user_id).await?;

    if let Some(ref since) = params.since {
        entries.retain(|e| e.created_at.as_str() >= since.as_str());
    }

    Ok(Json(json!({ "items": entries, "count": entries.len() })))
}

async fn get_stats(
    State(state): State<AppState>,
    Auth(_auth): Auth,
) -> Result<Json<Value>, AppError> {
    let stats = get_broca_stats(&state.db).await?;
    Ok(Json(json!(stats)))
}
