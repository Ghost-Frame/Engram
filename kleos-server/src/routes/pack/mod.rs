use axum::{routing::post, Json, Router};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::{Auth, ResolvedDb};
use crate::state::AppState;

mod types;
use types::PackBody;

pub fn router() -> Router<AppState> {
    Router::new().route("/pack", post(pack_memories))
}

async fn pack_memories(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<PackBody>,
) -> Result<Json<Value>, AppError> {
    let context = body.context.as_deref().unwrap_or("");
    let budget = body.token_budget.unwrap_or(4000).clamp(100, 128000);
    let format = match body.format.as_deref() {
        Some("json") => kleos_lib::pack::PackFormat::Json,
        Some("xml") => kleos_lib::pack::PackFormat::Xml,
        _ => kleos_lib::pack::PackFormat::Text,
    };
    let result =
        kleos_lib::pack::pack_memories(&db, context, budget, format, auth.user_id).await?;
    Ok(Json(json!({
        "packed": result.packed,
        "memories_included": result.memories_included,
        "tokens_estimated": result.tokens_estimated,
        "token_budget": result.token_budget,
        "utilization": result.utilization,
    })))
}
