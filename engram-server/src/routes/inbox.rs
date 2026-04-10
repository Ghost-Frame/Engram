use axum::{routing::{get, post}, extract::{State, Path, Query}, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::extractors::Auth;
use crate::error::AppError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/inbox", get(list_inbox))
        .route("/inbox/{id}/approve", post(approve))
        .route("/inbox/{id}/reject", post(reject))
        .route("/inbox/{id}/edit", post(edit))
        .route("/inbox/bulk", post(bulk_action))
        .route("/pending", get(list_pending_legacy))
}

#[derive(Deserialize)]
struct PagingQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn list_inbox(
    Auth(auth): Auth, State(state): State<AppState>, Query(q): Query<PagingQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = q.limit.unwrap_or(50).min(200);
    let offset = q.offset.unwrap_or(0);
    let pending = engram_lib::inbox::list_pending(&state.db, auth.user_id, limit, offset).await?;
    let total = engram_lib::inbox::count_pending(&state.db, auth.user_id).await?;
    Ok(Json(json!({ "pending": pending, "count": pending.len(), "total": total, "offset": offset, "limit": limit })))
}

async fn approve(
    Auth(auth): Auth, State(state): State<AppState>, Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    engram_lib::inbox::approve_memory(&state.db, id, auth.user_id).await?;
    Ok(Json(json!({ "approved": true, "id": id })))
}

#[derive(Deserialize)]
struct RejectBody {
    reason: Option<String>,
}

async fn reject(
    Auth(auth): Auth, State(state): State<AppState>, Path(id): Path<i64>, Json(body): Json<RejectBody>,
) -> Result<Json<Value>, AppError> {
    engram_lib::inbox::reject_memory(&state.db, id, auth.user_id).await?;
    if let Some(reason) = &body.reason {
        let _ = engram_lib::inbox::set_forget_reason(&state.db, id, reason, auth.user_id).await;
    }
    Ok(Json(json!({ "rejected": true, "id": id })))
}

#[derive(Deserialize)]
struct EditBody {
    content: Option<String>,
    category: Option<String>,
    importance: Option<i64>,
    tags: Option<String>,
}

async fn edit(
    Auth(auth): Auth, State(state): State<AppState>, Path(id): Path<i64>, Json(body): Json<EditBody>,
) -> Result<Json<Value>, AppError> {
    engram_lib::inbox::edit_and_approve(
        &state.db, id,
        body.content.as_deref(), body.category.as_deref(),
        body.importance, body.tags.as_deref(), auth.user_id,
    ).await?;
    Ok(Json(json!({ "approved": true, "edited": true, "id": id })))
}

#[derive(Deserialize)]
struct BulkBody {
    ids: Vec<i64>,
    action: String,
}

async fn bulk_action(
    Auth(auth): Auth, State(state): State<AppState>, Json(body): Json<BulkBody>,
) -> Result<Json<Value>, AppError> {
    let mut count = 0;
    for id in &body.ids {
        match body.action.as_str() {
            "approve" => { engram_lib::inbox::approve_memory(&state.db, *id, auth.user_id).await?; count += 1; }
            "reject" => { engram_lib::inbox::reject_memory(&state.db, *id, auth.user_id).await?; count += 1; }
            _ => return Err(AppError(engram_lib::EngError::InvalidInput("action must be approve or reject".into()))),
        }
    }
    Ok(Json(json!({ "action": body.action, "count": count, "ids": body.ids })))
}

async fn list_pending_legacy(
    Auth(auth): Auth, State(state): State<AppState>, Query(q): Query<PagingQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = q.limit.unwrap_or(50).min(200);
    let offset = q.offset.unwrap_or(0);
    let pending = engram_lib::inbox::list_pending(&state.db, auth.user_id, limit, offset).await?;
    let total = engram_lib::inbox::count_pending(&state.db, auth.user_id).await?;
    Ok(Json(json!({ "pending": pending, "count": pending.len(), "total": total, "offset": offset, "limit": limit })))
}
