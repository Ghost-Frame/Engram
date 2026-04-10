use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use engram_lib::agents;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;
use std::{fs, path::PathBuf, sync::OnceLock};

use crate::{error::AppError, extractors::Auth, state::AppState};

type HmacSha256 = Hmac<Sha256>;

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
    let existing = agents::get_agent_by_name(&state.db.conn, &body.name, auth.user_id).await?;
    if existing.is_some() {
        return Err(AppError(engram_lib::EngError::InvalidInput(format!(
            "Agent '{}' already registered",
            body.name
        ))));
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

    let issued_at = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let payload = json!({
        "agent_id": agent.id,
        "user_id": auth.user_id,
        "name": agent.name,
        "trust_score": agent.trust_score,
        "issued_at": issued_at,
        "expires_at": null,
    });
    let signature = sign_value(&payload)?;
    Ok(Json(json!({
        "agent_id": agent.id,
        "user_id": auth.user_id,
        "name": agent.name,
        "trust_score": agent.trust_score,
        "issued_at": issued_at,
        "expires_at": null,
        "signature": signature,
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
    Ok(Json(
        json!({ "linked": true, "agent_id": id, "key_id": body.key_id }),
    ))
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

async fn verify(Json(body): Json<VerifyBody>) -> Result<Json<Value>, AppError> {
    if let Some(passport) = body.passport {
        let result = verify_signed_value(&passport)?;
        return Ok(Json(json!({ "type": "passport", "valid": result })));
    }
    if body.execution.is_some() {
        return Ok(Json(
            json!({ "type": "execution", "valid": false, "error": "verification not implemented" }),
        ));
    }
    if body.message.is_some() {
        return Ok(Json(
            json!({ "type": "message", "valid": false, "error": "verification not implemented" }),
        ));
    }
    if body.tool_manifest.is_some() {
        return Ok(Json(
            json!({ "type": "tool_manifest", "valid": false, "error": "verification not implemented" }),
        ));
    }
    Err(AppError(engram_lib::EngError::InvalidInput(
        "Provide 'passport', 'execution', 'message', or 'tool_manifest' to verify".into(),
    )))
}

fn signing_secret() -> Result<&'static str, AppError> {
    static SECRET: OnceLock<String> = OnceLock::new();
    let secret = SECRET.get_or_init(load_or_create_signing_secret);
    if secret.trim().is_empty() {
        return Err(AppError(engram_lib::EngError::Internal(
            "signing secret is empty".into(),
        )));
    }
    Ok(secret.as_str())
}

fn load_or_create_signing_secret() -> String {
    let path = signing_secret_path();
    if let Ok(existing) = fs::read_to_string(&path) {
        let trimmed = existing.trim().to_string();
        if !trimmed.is_empty() {
            return trimmed;
        }
    }

    let generated = uuid::Uuid::new_v4().to_string();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&path, &generated);
    generated
}

fn signing_secret_path() -> PathBuf {
    if let Ok(path) = std::env::var("ENGRAM_SIGNING_SECRET_FILE") {
        return PathBuf::from(path);
    }
    PathBuf::from("engram-signing-secret.txt")
}

fn sign_value(payload: &Value) -> Result<String, AppError> {
    let secret = signing_secret()?;
    let bytes = serde_json::to_vec(payload)
        .map_err(|e| AppError(engram_lib::EngError::Internal(e.to_string())))?;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(
        |e: hmac::digest::InvalidLength| AppError(engram_lib::EngError::Internal(e.to_string())),
    )?;
    mac.update(&bytes);
    let digest = mac.finalize().into_bytes();
    Ok(digest.iter().map(|b| format!("{:02x}", b)).collect())
}

fn verify_signed_value(value: &Value) -> Result<bool, AppError> {
    let Some(signature) = value.get("signature").and_then(|v| v.as_str()) else {
        return Ok(false);
    };
    let mut unsigned = value.clone();
    if let Some(obj) = unsigned.as_object_mut() {
        obj.remove("signature");
    }
    Ok(sign_value(&unsigned)? == signature)
}
