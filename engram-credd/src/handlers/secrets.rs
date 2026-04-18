//! Secret CRUD handlers.

pub use super::types::{ListQuery, SecretListItem, StoreRequest};

use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde_json::{json, Value};

use engram_cred::audit::{log_audit, AccessTier, AuditAction};
use engram_cred::storage::{delete_secret, get_secret, list_secrets, store_secret, update_secret};
use engram_cred::CredError;

use crate::auth::Auth;
use crate::handlers::AppError;
use crate::state::AppState;

/// List secrets.
#[tracing::instrument(skip_all, fields(handler = "credd.secrets.list"))]
pub async fn list_handler(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>, AppError> {
    let secrets = list_secrets(&state.db, auth.user_id(), query.category.as_deref()).await?;

    let items: Vec<SecretListItem> = secrets
        .into_iter()
        .filter(|s| auth.can_access_category(&s.category))
        .map(|s| SecretListItem {
            service: s.category,
            key: s.name,
            secret_type: s.secret_type.to_string(),
            created_at: s.created_at,
            updated_at: s.updated_at,
        })
        .collect();

    Ok(Json(json!({ "secrets": items })))
}

/// Store a new secret.
#[tracing::instrument(skip_all, fields(handler = "credd.secrets.store"))]
pub async fn store_handler(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Path((category, name)): Path<(String, String)>,
    Json(body): Json<StoreRequest>,
) -> Result<Json<Value>, AppError> {
    if !auth.is_master() {
        return Err(CredError::PermissionDenied("only master key can store secrets".into()).into());
    }

    let id = store_secret(
        &state.db,
        auth.user_id(),
        &category,
        &name,
        &body.data,
        state.master_key.as_ref(),
    )
    .await?;

    log_audit(
        &state.db,
        auth.user_id(),
        auth.agent_name(),
        AuditAction::Set,
        &category,
        &name,
        None,
        true,
    )
    .await?;

    Ok(Json(json!({
        "id": id,
        "category": category,
        "name": name,
    })))
}

/// Get a secret.
#[tracing::instrument(skip_all, fields(handler = "credd.secrets.get"))]
pub async fn get_handler(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Path((category, name)): Path<(String, String)>,
) -> Result<Json<Value>, AppError> {
    if !auth.can_access_category(&category) {
        log_audit(
            &state.db,
            auth.user_id(),
            auth.agent_name(),
            AuditAction::Get,
            &category,
            &name,
            None,
            false,
        )
        .await?;
        return Err(
            CredError::PermissionDenied(format!("no access to category: {}", category)).into(),
        );
    }

    let (row, data) = get_secret(
        &state.db,
        auth.user_id(),
        &category,
        &name,
        state.master_key.as_ref(),
    )
    .await?;

    log_audit(
        &state.db,
        auth.user_id(),
        auth.agent_name(),
        AuditAction::Get,
        &category,
        &name,
        Some(AccessTier::Raw),
        true,
    )
    .await?;

    Ok(Json(json!({
        "service": row.category,
        "key": row.name,
        "type": row.secret_type.as_str(),
        "value": data,
    })))
}

/// Update a secret.
#[tracing::instrument(skip_all, fields(handler = "credd.secrets.update"))]
pub async fn update_handler(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Path((category, name)): Path<(String, String)>,
    Json(body): Json<StoreRequest>,
) -> Result<Json<Value>, AppError> {
    if !auth.is_master() {
        return Err(
            CredError::PermissionDenied("only master key can update secrets".into()).into(),
        );
    }

    update_secret(
        &state.db,
        auth.user_id(),
        &category,
        &name,
        &body.data,
        state.master_key.as_ref(),
    )
    .await?;

    log_audit(
        &state.db,
        auth.user_id(),
        auth.agent_name(),
        AuditAction::Update,
        &category,
        &name,
        None,
        true,
    )
    .await?;

    Ok(Json(json!({
        "category": category,
        "name": name,
        "updated": true,
    })))
}

/// Delete a secret.
#[tracing::instrument(skip_all, fields(handler = "credd.secrets.delete"))]
pub async fn delete_handler(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Path((category, name)): Path<(String, String)>,
) -> Result<Json<Value>, AppError> {
    if !auth.is_master() {
        return Err(
            CredError::PermissionDenied("only master key can delete secrets".into()).into(),
        );
    }

    delete_secret(&state.db, auth.user_id(), &category, &name).await?;

    log_audit(
        &state.db,
        auth.user_id(),
        auth.agent_name(),
        AuditAction::Delete,
        &category,
        &name,
        None,
        true,
    )
    .await?;

    Ok(Json(json!({
        "category": category,
        "name": name,
        "deleted": true,
    })))
}
