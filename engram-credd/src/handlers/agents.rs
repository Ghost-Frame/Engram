//! Agent key management handlers.

use axum::{
    extract::{Path, State},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use engram_cred::agent_keys::{
    create_agent_key, delete_agent_key, list_agent_keys, revoke_agent_key, AgentKeyPermissions,
};
use engram_cred::CredError;

use crate::auth::Auth;
use crate::handlers::AppError;
use crate::state::AppState;

#[derive(Serialize)]
pub struct AgentKeyInfo {
    pub name: String,
    pub categories: Vec<String>,
    pub allow_raw: bool,
    pub created_at: String,
    pub revoked_at: Option<String>,
    pub is_valid: bool,
}

/// List agent keys.
#[tracing::instrument(skip_all, fields(handler = "credd.agents.list"))]
pub async fn list_handler(
    Auth(auth): Auth,
    State(state): State<AppState>,
) -> Result<Json<Value>, AppError> {
    if !auth.is_master() {
        return Err(
            CredError::PermissionDenied("only master key can list agent keys".into()).into(),
        );
    }

    let keys = list_agent_keys(&state.db, auth.user_id()).await?;
    let items: Vec<AgentKeyInfo> = keys
        .into_iter()
        .map(|k| {
            let is_valid = k.is_valid();
            AgentKeyInfo {
                name: k.name,
                categories: k.permissions.categories,
                allow_raw: k.permissions.allow_raw,
                created_at: k.created_at,
                revoked_at: k.revoked_at,
                is_valid,
            }
        })
        .collect();

    Ok(Json(json!({ "keys": items })))
}

#[derive(Deserialize)]
pub struct CreateKeyRequest {
    pub name: String,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub allow_raw: bool,
}

/// Create a new agent key.
#[tracing::instrument(skip_all, fields(handler = "credd.agents.create"))]
pub async fn create_handler(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Json(body): Json<CreateKeyRequest>,
) -> Result<Json<Value>, AppError> {
    if !auth.is_master() {
        return Err(
            CredError::PermissionDenied("only master key can create agent keys".into()).into(),
        );
    }

    let permissions = AgentKeyPermissions {
        categories: body.categories,
        allow_raw: body.allow_raw,
    };

    let (raw_key, key_info) =
        create_agent_key(&state.db, auth.user_id(), &body.name, &permissions).await?;

    Ok(Json(json!({
        "name": key_info.name,
        "key": raw_key,
        "created_at": key_info.created_at,
        "note": "Save this key securely - it cannot be retrieved later",
    })))
}

/// Revoke an agent key.
#[tracing::instrument(skip_all, fields(handler = "credd.agents.revoke"))]
pub async fn revoke_handler(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Value>, AppError> {
    if !auth.is_master() {
        return Err(
            CredError::PermissionDenied("only master key can revoke agent keys".into()).into(),
        );
    }

    revoke_agent_key(&state.db, auth.user_id(), &name).await?;

    Ok(Json(json!({
        "name": name,
        "revoked": true,
    })))
}

/// Delete an agent key entirely.
#[tracing::instrument(skip_all, fields(handler = "credd.agents.delete"))]
pub async fn delete_handler(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Value>, AppError> {
    if !auth.is_master() {
        return Err(
            CredError::PermissionDenied("only master key can delete agent keys".into()).into(),
        );
    }

    delete_agent_key(&state.db, auth.user_id(), &name).await?;

    Ok(Json(json!({
        "name": name,
        "deleted": true,
    })))
}
