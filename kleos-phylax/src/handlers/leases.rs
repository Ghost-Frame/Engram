//! Lease list and redemption handlers.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use kleos_cred::CredError;
use kleos_credd::auth::Auth;
use kleos_credd::handlers::AppError;

use crate::audit::{actions, log_phylax_audit};
use crate::models::lease;
use crate::state::PhylaxState;

/// Query params for listing leases.
#[derive(Deserialize)]
pub struct ListQuery {
    /// Filter by agent name.
    pub agent: Option<String>,
    /// Maximum number of results.
    pub limit: Option<i64>,
}

/// List active (unused, unexpired) leases.
pub async fn list_leases(
    Auth(auth): Auth,
    State(state): State<PhylaxState>,
    Query(query): Query<ListQuery>,
) -> Result<impl IntoResponse, AppError> {
    let limit = query.limit.unwrap_or(50);
    let leases = lease::list_active_leases(
        &state.inner.db,
        auth.user_id(),
        query.agent.as_deref(),
        limit,
    )
    .await?;

    let items: Vec<_> = leases.iter().map(|l| l.to_json()).collect();
    Ok(Json(json!({ "leases": items })))
}

/// Atomically redeem a lease and return metadata without exposing secret bytes.
pub async fn redeem_lease(
    Auth(auth): Auth,
    State(state): State<PhylaxState>,
    Path(jti): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let l = match lease::redeem_lease(&state.inner.db, &jti, auth.user_id()).await {
        Ok(lease) => lease,
        Err(CredError::Database(msg)) if msg.contains("already redeemed") => {
            let _ = log_phylax_audit(
                &state.inner.db,
                auth.user_id(),
                auth.agent_name(),
                None,
                None,
                None,
                None,
                actions::LEASE_REPLAY,
                "",
                "",
                false,
                None,
            )
            .await;
            return Ok((
                StatusCode::CONFLICT,
                Json(json!({ "error": "lease already redeemed" })),
            )
                .into_response());
        }
        Err(e) => return Err(e.into()),
    };

    let _ = log_phylax_audit(
        &state.inner.db,
        auth.user_id(),
        Some(&l.agent_name),
        None,
        None,
        None,
        None,
        actions::LEASE_REDEEMED,
        &l.category,
        &l.secret_name,
        true,
        l.correlation_id.as_deref(),
    )
    .await;

    Ok(Json(json!({
        "lease": l.to_json(),
        "status": "redeemed",
        "message": "plaintext delivery disabled until proxy delivery is enabled",
        "secret": null,
    }))
    .into_response())
}
