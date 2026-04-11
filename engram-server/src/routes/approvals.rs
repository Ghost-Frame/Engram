use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use engram_lib::approvals::{
    create_approval, decide, expire_stale_for_user, get_approval, list_pending,
    Approval, ApprovalDecision, CreateApprovalRequest, DecideRequest,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/approvals", post(create_handler))
        .route("/approvals/pending", get(list_pending_handler))
        .route("/approvals/{id}", get(get_handler))
        .route("/approvals/{id}/decide", post(decide_handler))
}

#[derive(Debug, Serialize)]
struct ApprovalResponse {
    #[serde(flatten)]
    approval: Approval,
    seconds_remaining: i64,
}

impl From<Approval> for ApprovalResponse {
    fn from(approval: Approval) -> Self {
        let seconds_remaining = approval.seconds_remaining();
        Self {
            approval,
            seconds_remaining,
        }
    }
}

async fn create_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<CreateApprovalRequest>,
) -> Result<(StatusCode, Json<ApprovalResponse>), AppError> {
    let approval = create_approval(&state.db, &body, auth.user_id).await?;

    // Notify any waiting watchers that a new approval is pending
    if let Some(ref tx) = state.approval_notify {
        let _ = tx.send(());
    }

    Ok((StatusCode::CREATED, Json(approval.into())))
}

async fn get_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<String>,
) -> Result<Json<ApprovalResponse>, AppError> {
    let approval = get_approval(&state.db, &id, auth.user_id)
        .await?
        .ok_or_else(|| engram_lib::EngError::NotFound(format!("approval {} not found", id)))?;

    Ok(Json(approval.into()))
}

async fn list_pending_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    // First expire any stale approvals for this user
    let expired_count = expire_stale_for_user(&state.db, auth.user_id).await?;

    let approvals = list_pending(&state.db, auth.user_id).await?;
    let responses: Vec<ApprovalResponse> = approvals.into_iter().map(Into::into).collect();

    Ok(Json(json!({
        "approvals": responses,
        "count": responses.len(),
        "expired_count": expired_count,
    })))
}

#[derive(Debug, Deserialize)]
struct DecideBody {
    decision: ApprovalDecision,
    decided_by: Option<String>,
    reason: Option<String>,
}

async fn decide_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<String>,
    Json(body): Json<DecideBody>,
) -> Result<Json<ApprovalResponse>, AppError> {
    let req = DecideRequest {
        decision: body.decision,
        decided_by: body.decided_by,
        reason: body.reason,
    };

    let approval = decide(&state.db, &id, &req, auth.user_id).await?;

    // Notify any waiting watchers that a decision was made
    if let Some(ref tx) = state.approval_notify {
        let _ = tx.send(());
    }

    Ok(Json(approval.into()))
}
