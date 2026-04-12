// ============================================================================
// CONTEXT ROUTES -- POST /context
// ============================================================================

use axum::extract::{DefaultBodyLimit, State};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use std::time::Duration;
use tower_http::timeout::TimeoutLayer;

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use engram_lib::context::{assemble_context, ContextOptions};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/context", post(build_context))
        // S7-26: context assembly may run LLM inference + embedding; 30s cap.
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        // S7-27: context query payloads are small; 64 KB is ample.
        .layer(DefaultBodyLimit::max(64 * 1024))
}

async fn build_context(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<ContextOptions>,
) -> Result<Json<Value>, AppError> {
    if body.query.trim().is_empty() {
        return Err(AppError(engram_lib::EngError::InvalidInput(
            "query (string) required".to_string(),
        )));
    }

    let result = assemble_context(
        &state.db,
        body,
        auth.user_id,
        state.embedder.clone(),
        state.llm.clone(),
    )
    .await?;

    Ok(Json(json!(result)))
}
