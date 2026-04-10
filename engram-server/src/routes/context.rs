// ============================================================================
// CONTEXT ROUTES -- POST /context
// ============================================================================

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use engram_lib::context::{assemble_context, ContextOptions};

pub fn router() -> Router<AppState> {
    Router::new().route("/context", post(build_context))
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

    let result = assemble_context(&state.db, body, auth.user_id, state.embedder.clone(), state.llm.clone()).await?;

    Ok(Json(json!(result)))
}
