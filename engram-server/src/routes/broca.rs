use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use engram_lib::services::broca::{
    get_action, get_stats as get_broca_stats, log_action, query_actions, LogActionRequest,
};

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

#[derive(Debug, Deserialize)]
struct LogActionBody {
    agent: String,
    /// The plan spec says `service` and `action` but the lib uses `action` and `summary`
    service: Option<String>,
    action: Option<String>,
    summary: Option<String>,
    detail: Option<String>,
    project: Option<String>,
    metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct QueryActionsParams {
    agent: Option<String>,
    service: Option<String>,
    action: Option<String>,
    project: Option<String>,
    since: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
}

async fn log_action_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<LogActionBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    // Map service+action -> action, detail/summary -> summary
    let action = body.action.unwrap_or_else(|| {
        body.service
            .as_deref()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    });

    let summary = body
        .summary
        .or(body.detail)
        .unwrap_or_else(|| action.clone());

    let req = LogActionRequest {
        agent: body.agent,
        action,
        summary,
        project: body.project,
        metadata: body.metadata,
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
    let limit = params.limit.unwrap_or(100);
    let offset = params.offset.unwrap_or(0);

    // Support `service` as a query filter mapped to `project` field or agent filter
    let agent = params.agent.as_deref();
    let project = params.project.as_deref().or(params.service.as_deref());
    let action = params.action.as_deref();

    let mut entries = query_actions(
        &state.db,
        agent,
        project,
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
    let limit = params.limit.unwrap_or(100);
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
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let stats = get_broca_stats(&state.db, Some(auth.user_id)).await?;
    Ok(Json(json!(stats)))
}
