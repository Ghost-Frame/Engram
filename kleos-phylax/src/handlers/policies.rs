//! Access policy CRUD handlers. Master-only.

use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use kleos_cred::CredError;
use kleos_credd::auth::Auth;
use kleos_credd::handlers::AppError;

use crate::models::policy;
use crate::state::PhylaxState;

/// Every resolve mode a policy may name. text/raw remain meaningful for
/// master-targeted policies even though agents can never use them.
const VALID_MODES: &[&str] = &["text", "proxy", "raw", "exec", "verify", "sign", "derive"];

/// Reject unknown mode strings: a typo'd mode would otherwise create a
/// policy that silently never matches.
fn validate_modes(modes: &[String]) -> Result<(), AppError> {
    for m in modes {
        if !VALID_MODES.contains(&m.as_str()) {
            return Err(CredError::InvalidInput(format!("unknown resolve mode '{m}'")).into());
        }
    }
    Ok(())
}

/// Exec allowlist entries must be absolute paths: relative names would
/// resolve against the daemon's environment, not a policy decision.
fn validate_exec_allowlist(allowlist: Option<&[String]>) -> Result<(), AppError> {
    if let Some(paths) = allowlist {
        for p in paths {
            if !p.starts_with('/') {
                return Err(CredError::InvalidInput(format!(
                    "exec allowlist entry '{p}' must be an absolute path"
                ))
                .into());
            }
        }
    }
    Ok(())
}

/// Request body for creating a policy.
#[derive(Deserialize)]
pub struct CreatePolicyRequest {
    /// Namespace this policy applies to.
    pub namespace: String,
    /// Category filter (optional).
    pub category: Option<String>,
    /// Secret name filter (optional).
    pub secret_name: Option<String>,
    /// Whether approval is required.
    pub require_approval: bool,
    /// Allowed resolve modes.
    pub allowed_modes: Option<Vec<String>>,
    /// Absolute argv[0] paths exec mode may spawn (None = exec denied).
    pub exec_allowlist: Option<Vec<String>>,
}

/// Request body for updating a policy.
#[derive(Deserialize)]
pub struct UpdatePolicyRequest {
    /// Whether approval is required.
    pub require_approval: bool,
    /// Allowed resolve modes.
    pub allowed_modes: Vec<String>,
    /// Absolute argv[0] paths exec mode may spawn (None = exec denied).
    pub exec_allowlist: Option<Vec<String>>,
}

/// List all access policies. Master-only.
pub async fn list_policies(
    Auth(auth): Auth,
    State(state): State<PhylaxState>,
) -> Result<impl IntoResponse, AppError> {
    if !auth.is_master() {
        return Err(CredError::PermissionDenied("master key required".into()).into());
    }

    let policies = policy::list_policies(&state.inner.db, auth.user_id()).await?;
    let items: Vec<_> = policies.iter().map(|p| p.to_json()).collect();
    Ok(Json(json!({ "policies": items })))
}

/// Create a new access policy. Master-only.
pub async fn create_policy(
    Auth(auth): Auth,
    State(state): State<PhylaxState>,
    Json(body): Json<CreatePolicyRequest>,
) -> Result<impl IntoResponse, AppError> {
    if !auth.is_master() {
        return Err(CredError::PermissionDenied("master key required".into()).into());
    }

    let modes = body
        .allowed_modes
        .unwrap_or_else(|| vec!["text".into(), "proxy".into(), "raw".into()]);
    validate_modes(&modes)?;
    validate_exec_allowlist(body.exec_allowlist.as_deref())?;

    let p = policy::create_policy(
        &state.inner.db,
        auth.user_id(),
        &body.namespace,
        body.category.as_deref(),
        body.secret_name.as_deref(),
        body.require_approval,
        &modes,
        body.exec_allowlist.as_deref(),
    )
    .await?;

    Ok(Json(p.to_json()))
}

/// Update an existing policy. Master-only.
pub async fn update_policy(
    Auth(auth): Auth,
    State(state): State<PhylaxState>,
    Path(id): Path<i64>,
    Json(body): Json<UpdatePolicyRequest>,
) -> Result<impl IntoResponse, AppError> {
    if !auth.is_master() {
        return Err(CredError::PermissionDenied("master key required".into()).into());
    }

    validate_modes(&body.allowed_modes)?;
    validate_exec_allowlist(body.exec_allowlist.as_deref())?;
    policy::update_policy(
        &state.inner.db,
        id,
        body.require_approval,
        &body.allowed_modes,
        body.exec_allowlist.as_deref(),
    )
    .await?;

    Ok(Json(json!({ "ok": true })))
}

/// Delete a policy. Master-only.
pub async fn delete_policy(
    Auth(auth): Auth,
    State(state): State<PhylaxState>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    if !auth.is_master() {
        return Err(CredError::PermissionDenied("master key required".into()).into());
    }

    policy::delete_policy(&state.inner.db, id).await?;
    Ok(Json(json!({ "ok": true })))
}
