use axum::{
    extract::{Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{error::AppError, extractors::Auth, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sync/changes", get(get_sync_changes))
        .route("/sync/receive", post(sync_receive))
}

#[derive(Debug, Deserialize)]
struct SyncQuery {
    since: Option<String>,
    limit: Option<i64>,
}

#[derive(Deserialize)]
struct SyncReceiveBody {
    changes: Vec<engram_lib::sync::SyncReceiveChange>,
}

async fn sync_receive(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<SyncReceiveBody>,
) -> Result<Json<Value>, AppError> {
    let result = engram_lib::sync::receive_sync(&state.db, auth.user_id, body.changes).await?;
    Ok(Json(serde_json::to_value(result).map_err(|e| {
        AppError(engram_lib::EngError::Internal(e.to_string()))
    })?))
}

async fn get_sync_changes(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(q): Query<SyncQuery>,
) -> Result<Json<Value>, AppError> {
    let since = q.since.as_deref().unwrap_or("1970-01-01T00:00:00");
    let limit = q.limit.unwrap_or(100).min(1000);
    let changes =
        engram_lib::webhooks::get_changes_since(&state.db, since, auth.user_id, limit).await?;
    Ok(Json(json!({
        "changes": changes,
        "count": changes.len(),
        "since": since,
        "server_time": chrono::Utc::now().to_rfc3339(),
    })))
}
