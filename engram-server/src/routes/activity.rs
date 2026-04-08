use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use engram_lib::activity::{process_activity, ActivityReport};

pub fn router() -> Router<AppState> {
    Router::new().route("/activity", post(report_activity))
}

async fn report_activity(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<ActivityReport>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let memory_id = process_activity(&state.db, &body, auth.user_id).await?;
    Ok((StatusCode::CREATED, Json(json!({ "ok": true, "memory_id": memory_id }))))
}
