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
    get_agent, get_stats as get_soma_stats, heartbeat, list_agents, register_agent,
    RegisterAgentRequest,
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
    /// The plan spec says `agent_type` but lib uses `category`
    agent_type: Option<String>,
    category: Option<String>,
    description: Option<String>,
    /// Ignored -- lib doesn't support capabilities field
    #[allow(dead_code)]
    capabilities: Option<serde_json::Value>,
    /// Ignored -- lib doesn't support metadata field on register
    #[allow(dead_code)]
    metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct UpdateAgentBody {
    /// Ignored for now -- lib doesn't have a direct update function, uses register (upsert by name)
    #[allow(dead_code)]
    status: Option<String>,
    #[allow(dead_code)]
    metadata: Option<serde_json::Value>,
    category: Option<String>,
    description: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListAgentsParams {
    agent_type: Option<String>,
    status: Option<String>,
    limit: Option<usize>,
}

async fn create_agent_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<CreateAgentBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let category = body.category.or(body.agent_type);

    let req = RegisterAgentRequest {
        user_id: Some(auth.user_id),
        name: body.name,
        category,
        description: body.description,
    };

    let agent = register_agent(&state.db, req).await?;
    Ok((StatusCode::CREATED, Json(json!(agent))))
}

async fn list_agents_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ListAgentsParams>,
) -> Result<Json<Value>, AppError> {
    let active_only = params.status.as_deref() == Some("active");
    let mut agents = list_agents(&state.db, Some(auth.user_id), active_only).await?;

    // Filter by agent_type/category in-memory
    if let Some(ref agent_type) = params.agent_type {
        agents.retain(|a| a.category.as_deref() == Some(agent_type.as_str()));
    }

    let limit = params.limit.unwrap_or(100);
    agents.truncate(limit);

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
    // Fetch existing agent to get current name
    let existing = get_agent(&state.db, id, auth.user_id).await?;

    // Re-register with updated fields (lib uses INSERT OR IGNORE, so we update directly via re-register)
    // Since lib only supports INSERT OR IGNORE (not UPDATE), we need to use the DB directly
    // The only mutable fields via register are category and description.
    // We'll do a direct update via the db connection.
    let new_name = body.name.unwrap_or(existing.name.clone());
    let new_category = body.category.or(existing.category.clone());
    let new_description = body.description.or(existing.description.clone());

    state.db.conn
        .execute(
            "UPDATE agents SET name = ?1, category = ?2, description = ?3 WHERE id = ?4 AND user_id = ?5",
            libsql::params![new_name, new_category, new_description, id, auth.user_id],
        )
        .await
        .map_err(engram_lib::EngError::Database)?;

    let agent = get_agent(&state.db, id, auth.user_id).await?;
    Ok(Json(json!(agent)))
}

async fn delete_agent_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    state
        .db
        .conn
        .execute(
            "DELETE FROM agents WHERE id = ?1 AND user_id = ?2",
            libsql::params![id, auth.user_id],
        )
        .await
        .map_err(engram_lib::EngError::Database)?;

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
