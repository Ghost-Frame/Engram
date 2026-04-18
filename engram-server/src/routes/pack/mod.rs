use axum::{extract::State, routing::post, Json, Router};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;

mod types;
use types::PackBody;

pub fn router() -> Router<AppState> {
    Router::new().route("/pack", post(pack_memories))
}

async fn pack_memories(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Json(body): Json<PackBody>,
) -> Result<Json<Value>, AppError> {
    let context = body.context.as_deref().unwrap_or("");
    let budget = body.token_budget.unwrap_or(4000).clamp(100, 128000);
    let format = match body.format.as_deref() {
        Some("json") => engram_lib::pack::PackFormat::Json,
        Some("xml") => engram_lib::pack::PackFormat::Xml,
        _ => engram_lib::pack::PackFormat::Text,
    };
    let result =
        engram_lib::pack::pack_memories(&state.db, context, budget, format, auth.user_id).await?;
    Ok(Json(json!({
        "packed": result.packed,
        "memories_included": result.memories_included,
        "tokens_estimated": result.tokens_estimated,
        "token_budget": result.token_budget,
        "utilization": result.utilization,
    })))
}
