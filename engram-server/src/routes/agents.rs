use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use engram_lib::agents;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{error::AppError, extractors::Auth, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/agents", post(register_agent).get(list_agents))
        .route("/agents/{id}", get(get_agent))
        .route("/agents/{id}/revoke", post(revoke_agent))
        .route("/agents/{id}/passport", get(get_passport))
        .route("/agents/{id}/link-key", post(link_key))
        .route("/agents/{id}/executions", get(get_executions))
        .route("/verify", post(verify))
}

#[derive(Debug, Deserialize)]
struct RegisterBody {
    pub name: String,
    pub category: Option<String>,
    pub description: Option<String>,
    pub code_hash: Option<String>,
}

async fn register_agent(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<RegisterBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    if body.name.trim().is_empty() {
        return Err(AppError(engram_lib::EngError::InvalidInput(
            "name (string) required".into(),
        )));
    }

    // Check for duplicate
    let existing =
        agents::get_agent_by_name(&state.db.conn, &body.name, auth.user_id).await?;
    if existing.is_some() {
        return Err(AppError(engram_lib::EngError::InvalidInput(
            format!("Agent '{}' already registered", body.name),
        )));
    }

    let result = agents::insert_agent(
        &state.db.conn,
        auth.user_id,
        &body.name,
        body.category.as_deref(),
        body.description.as_deref(),
        body.code_hash.as_deref(),
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "agent_id": result.id,
            "name": body.name,
            "trust_score": result.trust_score,
            "created_at": result.created_at,
        })),
    ))
}

async fn list_agents(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let agents = agents::list_agents(&state.db.conn, auth.user_id).await?;
    Ok(Json(json!({ "agents": agents })))
}

async fn get_agent(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let agent = agents::get_agent_by_id(&state.db.conn, id, auth.user_id)
        .await?
        .ok_or_else(|| AppError(engram_lib::EngError::NotFound("Agent not found".into())))?;

    // Omit code_hash from response (matches TS behavior)
    Ok(Json(json!({
        "id": agent.id,
        "user_id": agent.user_id,
        "name": agent.name,
        "category": agent.category,
        "description": agent.description,
        "trust_score": agent.trust_score,
        "total_ops": agent.total_ops,
        "successful_ops": agent.successful_ops,
        "failed_ops": agent.failed_ops,
        "guard_allows": agent.guard_allows,
        "guard_warns": agent.guard_warns,
        "guard_blocks": agent.guard_blocks,
        "is_active": agent.is_active,
        "last_seen_at": agent.last_seen_at,
        "revoked_at": agent.revoked_at,
        "revoke_reason": agent.revoke_reason,
        "created_at": agent.created_at,
    })))
}

#[derive(Debug, Deserialize)]
struct RevokeBody {
    pub reason: Option<String>,
}

async fn revoke_agent(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<RevokeBody>,
) -> Result<Json<Value>, AppError> {
    let reason = body.reason.as_deref().unwrap_or("revoked");
    agents::revoke_agent(&state.db.conn, id, auth.user_id, reason).await?;
    Ok(Json(json!({ "revoked": true, "agent_id": id })))
}

async fn get_passport(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let agent = agents::get_agent_by_id(&state.db.conn, id, auth.user_id)
        .await?
        .ok_or_else(|| AppError(engram_lib::EngError::NotFound("Agent not found".into())))?;

    if !agent.is_active {
        return Err(AppError(engram_lib::EngError::InvalidInput(
            "Agent is revoked".into(),
        )));
    }

    // TODO: Implement HMAC signing for passports (requires signing secret infrastructure)
    Ok(Json(json!({
        "agent_id": agent.id,
        "user_id": auth.user_id,
        "name": agent.name,
        "trust_score": agent.trust_score,
        "issued_at": chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        "expires_at": null,
        "signature": "not_implemented",
    })))
}

#[derive(Debug, Deserialize)]
struct LinkKeyBody {
    pub key_id: i64,
}

async fn link_key(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<LinkKeyBody>,
) -> Result<Json<Value>, AppError> {
    let agent = agents::get_agent_by_id(&state.db.conn, id, auth.user_id)
        .await?
        .ok_or_else(|| AppError(engram_lib::EngError::NotFound("Agent not found".into())))?;

    agents::link_key_to_agent(&state.db.conn, agent.id, body.key_id, auth.user_id).await?;
    Ok(Json(json!({ "linked": true, "agent_id": id, "key_id": body.key_id })))
}

#[derive(Debug, Deserialize)]
struct ExecutionsQuery {
    pub limit: Option<i64>,
}

async fn get_executions(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Query(params): Query<ExecutionsQuery>,
) -> Result<Json<Value>, AppError> {
    // Verify agent belongs to user
    let agent = agents::get_agent_by_id(&state.db.conn, id, auth.user_id)
        .await?
        .ok_or_else(|| AppError(engram_lib::EngError::NotFound("Agent not found".into())))?;

    let limit = params.limit.unwrap_or(50);
    let executions = agents::get_agent_executions(&state.db.conn, agent.id, limit).await?;
    Ok(Json(json!({ "agent_id": id, "executions": executions })))
}

#[derive(Debug, Deserialize)]
struct VerifyBody {
    pub passport: Option<Value>,
    pub execution: Option<Value>,
    pub message: Option<Value>,
    pub tool_manifest: Option<Value>,
}

async fn verify(
    Json(body): Json<VerifyBody>,
) -> Result<Json<Value>, AppError> {
    // TODO: Implement HMAC verification (requires signing secret infrastructure)
    if body.passport.is_some() {
        return Ok(Json(json!({ "type": "passport", "valid": false, "error": "verification not implemented" })));
    }
    if body.execution.is_some() {
        return Ok(Json(json!({ "type": "execution", "valid": false, "error": "verification not implemented" })));
    }
    if body.message.is_some() {
        return Ok(Json(json!({ "type": "message", "valid": false, "error": "verification not implemented" })));
    }
    if body.tool_manifest.is_some() {
        return Ok(Json(json!({ "type": "tool_manifest", "valid": false, "error": "verification not implemented" })));
    }
    Err(AppError(engram_lib::EngError::InvalidInput(
        "Provide 'passport', 'execution', 'message', or 'tool_manifest' to verify".into(),
    )))
}
