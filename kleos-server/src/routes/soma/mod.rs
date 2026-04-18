use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use kleos_lib::services::soma::{
    add_agent_to_group, create_group, delete_agent, get_agent, get_stats as get_soma_stats,
    heartbeat, list_agent_logs, list_agents, list_groups, log_event, register_agent,
    remove_agent_from_group, set_status, RegisterAgentRequest,
};

mod types;
use types::{
    AddMemberBody, CreateAgentBody, CreateGroupBody, ListAgentsParams, ListLogsParams,
    LogEventBody, UpdateAgentBody,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/soma/agents",
            post(create_agent_handler).get(list_agents_handler),
        )
        .route(
            "/soma/agents/{id}",
            get(get_agent_handler)
                .patch(update_agent_handler)
                .delete(delete_agent_handler),
        )
        .route("/soma/agents/{id}/heartbeat", post(heartbeat_handler))
        .route(
            "/soma/agents/{id}/log",
            post(log_event_handler).get(list_logs_handler),
        )
        .route("/soma/agents/{id}/logs", get(list_logs_handler))
        .route(
            "/soma/groups",
            post(create_group_handler).get(list_groups_handler),
        )
        .route("/soma/groups/{id}/members", post(add_member_handler))
        .route(
            "/soma/groups/{id}/members/{agent_id}",
            axum::routing::delete(remove_member_handler),
        )
        .route("/soma/stats", get(get_stats))
}

async fn create_agent_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<CreateAgentBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let type_ = body
        .r#type
        .ok_or_else(|| kleos_lib::EngError::InvalidInput("type is required".into()))?;

    let req = RegisterAgentRequest {
        user_id: Some(auth.user_id),
        name: body.name,
        type_,
        description: body.description,
        capabilities: body.capabilities,
        config: body.config,
    };

    let agent = register_agent(&state.db, req).await?;
    Ok((StatusCode::CREATED, Json(json!(agent))))
}

async fn list_agents_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ListAgentsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(100).min(1000);
    let agents = list_agents(
        &state.db,
        auth.user_id,
        params.agent_type.as_deref(),
        params.status.as_deref(),
        limit,
    )
    .await?;

    Ok(Json(json!({ "agents": agents, "count": agents.len() })))
}

async fn get_agent_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let agent = get_agent(&state.db, id, auth.user_id).await?;
    Ok(Json(json!(agent)))
}

async fn update_agent_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<UpdateAgentBody>,
) -> Result<Json<Value>, AppError> {
    let existing = get_agent(&state.db, id, auth.user_id).await?;

    if body.r#type.is_some()
        || body.description.is_some()
        || body.capabilities.is_some()
        || body.config.is_some()
    {
        let type_ = body.r#type.unwrap_or(existing.type_.clone());
        let description = body.description.or(existing.description.clone());
        let capabilities = body.capabilities.or(Some(existing.capabilities.clone()));
        let config = body.config.or(Some(existing.config.clone()));
        register_agent(
            &state.db,
            RegisterAgentRequest {
                user_id: Some(auth.user_id),
                name: existing.name.clone(),
                type_,
                description,
                capabilities,
                config,
            },
        )
        .await?;
    }

    if let Some(status) = body.status.as_deref() {
        set_status(&state.db, id, auth.user_id, status).await?;
    }

    let agent = get_agent(&state.db, id, auth.user_id).await?;
    Ok(Json(json!(agent)))
}

async fn delete_agent_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    delete_agent(&state.db, id, auth.user_id).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn heartbeat_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    heartbeat(&state.db, id, auth.user_id).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn get_stats(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let stats = get_soma_stats(&state.db, Some(auth.user_id)).await?;
    Ok(Json(json!(stats)))
}

// --- New handlers for P0-0 Phase 27c: groups and logs ---

async fn create_group_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<CreateGroupBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let group = create_group(&state.db, body.name, body.description, auth.user_id).await?;
    Ok((StatusCode::CREATED, Json(json!(group))))
}

async fn list_groups_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let groups = list_groups(&state.db, auth.user_id).await?;
    Ok(Json(json!({ "groups": groups, "count": groups.len() })))
}

async fn add_member_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(group_id): Path<i64>,
    Json(body): Json<AddMemberBody>,
) -> Result<Json<Value>, AppError> {
    add_agent_to_group(&state.db, body.agent_id, group_id, auth.user_id).await?;
    Ok(Json(
        json!({ "ok": true, "group_id": group_id, "agent_id": body.agent_id }),
    ))
}

async fn remove_member_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path((group_id, agent_id)): Path<(i64, i64)>,
) -> Result<Json<Value>, AppError> {
    let removed = remove_agent_from_group(&state.db, agent_id, group_id, auth.user_id).await?;
    Ok(Json(json!({ "removed": removed })))
}

async fn log_event_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(agent_id): Path<i64>,
    Json(body): Json<LogEventBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let id = log_event(&state.db, agent_id, &body.level, &body.message, body.data).await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

async fn list_logs_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(agent_id): Path<i64>,
    Query(params): Query<ListLogsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(100).min(1000);
    let logs = list_agent_logs(&state.db, agent_id, auth.user_id, limit).await?;
    Ok(Json(json!({ "logs": logs, "count": logs.len() })))
}
