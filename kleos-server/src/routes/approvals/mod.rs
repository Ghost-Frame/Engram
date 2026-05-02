use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::{Auth, ResolvedDb};
use crate::state::AppState;
use kleos_lib::approvals::{
    create_approval, decide, expire_stale, get_approval, list_pending, CreateApprovalRequest,
    DecideRequest,
};

mod types;
use types::{ApprovalResponse, DecideBody};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/approvals", post(create_handler))
        .route("/approvals/pending", get(list_pending_handler))
        .route("/approvals/{id}", get(get_handler))
        .route("/approvals/{id}/decide", post(decide_handler))
}

async fn create_handler(
    State(state): State<AppState>,
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Json(body): Json<CreateApprovalRequest>,
) -> Result<(StatusCode, Json<ApprovalResponse>), AppError> {
    let approval = create_approval(&db, &body, auth.user_id).await?;

    // Notify any waiting watchers that a new approval is pending
    if let Some(ref tx) = state.approval_notify {
        let _ = tx.send(());
    }

    Ok((StatusCode::CREATED, Json(approval.into())))
}

async fn get_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Path(id): Path<String>,
) -> Result<Json<ApprovalResponse>, AppError> {
    let approval = get_approval(&db, &id, auth.user_id)
        .await?
        .ok_or_else(|| kleos_lib::EngError::NotFound(format!("approval {} not found", id)))?;

    Ok(Json(approval.into()))
}

async fn list_pending_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    // First expire any stale approvals for this user
    let expired_count = expire_stale(&db).await?;

    let approvals = list_pending(&db, auth.user_id).await?;
    let responses: Vec<ApprovalResponse> = approvals.into_iter().map(Into::into).collect();

    Ok(Json(json!({
        "approvals": responses,
        "count": responses.len(),
        "expired_count": expired_count,
    })))
}

async fn decide_handler(
    State(state): State<AppState>,
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Path(id): Path<String>,
    Json(body): Json<DecideBody>,
) -> Result<Json<ApprovalResponse>, AppError> {
    let req = DecideRequest {
        decision: body.decision,
        decided_by: body.decided_by,
        reason: body.reason,
    };

    let approval = decide(&db, &id, &req, auth.user_id).await?;

    // Notify any waiting watchers that a decision was made
    if let Some(ref tx) = state.approval_notify {
        let _ = tx.send(());
    }

    Ok(Json(approval.into()))
}
