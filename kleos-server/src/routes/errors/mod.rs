use axum::{
    extract::{Query, State},
    routing::{get, post},
    Json, Router,
};
use serde_json::Value;

use crate::{error::AppError, extractors::Auth, state::AppState};
use kleos_lib::errors_log::{self, ListErrorsRequest, LogErrorRequest};

#[allow(dead_code)]
mod types;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/errors", post(post_error))
        .route("/errors", get(get_errors))
}

async fn post_error(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<LogErrorRequest>,
) -> Result<Json<Value>, AppError> {
    let id = errors_log::log_error(&state.db, body, Some(&auth.user_id.to_string())).await?;
    Ok(Json(serde_json::json!({ "id": id })))
}

async fn get_errors(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(query): Query<ListErrorsRequest>,
) -> Result<Json<Value>, AppError> {
    let events = errors_log::list_errors(&state.db, &auth.user_id.to_string(), query).await?;
    Ok(Json(
        serde_json::to_value(events).map_err(kleos_lib::EngError::Serialization)?,
    ))
}
