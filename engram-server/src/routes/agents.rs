use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use engram_lib::agents;
use engram_lib::EngError;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/agents", get(list_agents_handler).post(register_agent_handler))
        .route("/agents/by-name/{name}", get(get_agent_by_name_handler))
        .route("/agents/{id}", get(get_agent_handler).delete(revoke_agent_handler))
        .route("/agents/{id}/heartbeat", post(heartbeat_handler))
        .route("/agents/{id}/executions", get(get_agent_executions_handler))
        .route("/agents/{id}/link-key/{key_id}", post(link_key_handler))
}

#[derive(Debug, Deserialize)]
struct RegisterAgentBody {
    name: String,
    category: Option<String>,
    description: Option<String>,
    code_hash: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExecutionQuery {
    limit: Option<i64>,
}

async fn register_agent_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<RegisterAgentBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let inserted = agents::insert_agent(
        &state.db.conn,
        auth.user_id,
        &body.name,
        body.category.as_deref(),
        body.description.as_deref(),
        body.code_hash.as_deref(),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(json!(inserted))))
}

async fn list_agents_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let list = agents::list_agents(&state.db.conn, auth.user_id).await?;
    Ok(Json(json!({ "agents": list, "count": list.len() })))
}

async fn get_agent_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let agent = agents::get_agent_by_id(&state.db.conn, id, auth.user_id).await?;
    Ok(Json(json!({ "agent": agent })))
}

async fn get_agent_by_name_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(name): Path<String>,
) -> Result<Json<Value>, AppError> {
    let agent = agents::get_agent_by_name(&state.db.conn, &name, auth.user_id).await?;
    Ok(Json(json!({ "agent": agent })))
}

async fn revoke_agent_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let reason = "revoked by API request".to_string();
    agents::revoke_agent(&state.db.conn, id, auth.user_id, &reason).await?;
    Ok(Json(json!({ "revoked": true, "id": id })))
}

async fn heartbeat_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    state
        .db
        .conn
        .execute(
            "UPDATE agents SET last_seen_at = datetime('now') WHERE id = ?1 AND user_id = ?2",
            libsql::params![id, auth.user_id],
        )
        .await
        .map_err(EngError::Database)?;
    Ok(Json(json!({ "ok": true, "id": id })))
}

async fn get_agent_executions_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
    Query(query): Query<ExecutionQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = query.limit.unwrap_or(50);
    let rows = agents::get_agent_executions(&state.db.conn, id, limit).await?;
    Ok(Json(json!({ "executions": rows, "count": rows.len() })))
}

async fn link_key_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path((id, key_id)): Path<(i64, i64)>,
) -> Result<Json<Value>, AppError> {
    agents::link_key_to_agent(&state.db.conn, id, key_id, auth.user_id).await?;
    Ok(Json(json!({ "linked": true, "agent_id": id, "key_id": key_id })))
}
