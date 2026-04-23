use axum::{
    extract::Query,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};

use crate::{
    error::AppError,
    extractors::{Auth, ResolvedDb},
    state::AppState,
};

mod types;
use types::{SyncQuery, SyncReceiveBody};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sync/changes", get(get_sync_changes))
        .route("/sync/receive", post(sync_receive))
}

async fn sync_receive(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<SyncReceiveBody>,
) -> Result<Json<Value>, AppError> {
    let result = kleos_lib::sync::receive_sync(&db, auth.user_id, body.changes).await?;
    Ok(Json(serde_json::to_value(result).map_err(|e| {
        AppError(kleos_lib::EngError::Internal(e.to_string()))
    })?))
}

async fn get_sync_changes(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(q): Query<SyncQuery>,
) -> Result<Json<Value>, AppError> {
    let since = q.since.as_deref().unwrap_or("1970-01-01T00:00:00");
    let limit = q.limit.unwrap_or(100).min(1000);
    let changes = kleos_lib::webhooks::get_changes_since(&db, since, auth.user_id, limit).await?;
    Ok(Json(json!({
        "changes": changes,
        "count": changes.len(),
        "since": since,
        "server_time": chrono::Utc::now().to_rfc3339(),
    })))
}
