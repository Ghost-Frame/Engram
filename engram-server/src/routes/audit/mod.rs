use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use engram_lib::audit::{count_audit_entries, list_audit_entries};

mod types;
use types::AuditQueryParams;

pub fn router() -> Router<AppState> {
    Router::new().route("/audit", get(get_audit))
}

async fn get_audit(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<AuditQueryParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(50).clamp(1, 500);
    let offset = params.offset.unwrap_or(0).max(0);

    let entries = list_audit_entries(&state.db, auth.user_id, limit, offset).await?;
    let total = count_audit_entries(&state.db, auth.user_id).await?;

    Ok(Json(json!({
        "entries": entries,
        "total": total,
        "limit": limit,
        "offset": offset,
    })))
}
