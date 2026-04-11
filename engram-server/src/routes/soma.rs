use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use engram_lib::services::soma::{
    delete_agent, get_agent, get_stats as get_soma_stats, heartbeat, list_agents, register_agent,
    set_status, RegisterAgentRequest,
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
        .route("/soma/stats", get(get_stats))
}

#[derive(Debug, Deserialize)]
struct CreateAgentBody {
    name: String,
    #[serde(alias = "agent_type", alias = "category")]
    r#type: Option<String>,
    description: Option<String>,
    capabilities: Option<serde_json::Value>,
    config: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct UpdateAgentBody {
    status: Option<String>,
    #[serde(alias = "agent_type", alias = "category")]
    r#type: Option<String>,
    description: Option<String>,
    capabilities: Option<serde_json::Value>,
    config: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct ListAgentsParams {
    #[serde(alias = "type")]
    agent_type: Option<String>,
    status: Option<String>,
    limit: Option<usize>,
}

async fn create_agent_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<CreateAgentBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let type_ = body
        .r#type
        .ok_or_else(|| engram_lib::EngError::InvalidInput("type is required".into()))?;

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
