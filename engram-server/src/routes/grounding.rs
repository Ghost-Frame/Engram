use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use engram_lib::grounding::{GroundingClient, ToolQualityManager};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/grounding/tools", get(list_tools_handler))
        .route("/grounding/execute", post(execute_handler))
        .route("/grounding/sessions", get(list_sessions_handler))
        .route("/grounding/quality/{tool_name}", get(quality_handler))
        .route("/grounding/quality/degraded", get(degraded_handler))
}

#[derive(Debug, Deserialize)]
struct ExecuteBody {
    tool: String,
    args: Option<serde_json::Value>,
    timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct DegradedQuery {
    threshold: Option<f64>,
}

async fn list_tools_handler(Auth(_auth): Auth) -> Result<Json<Value>, AppError> {
    let client = GroundingClient::new();
    Ok(Json(json!({ "tools": client.get_all_tools() })))
}

async fn execute_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<ExecuteBody>,
) -> Result<Json<Value>, AppError> {
    let client = GroundingClient::new();
    let args = body.args.unwrap_or_else(|| json!({}));
    let result = client
        .execute_tool(&body.tool, &args, body.timeout_ms)
        .await;

    let manager = ToolQualityManager::new(None);
    let success = result.status == engram_lib::grounding::ToolStatus::Success;
    let latency_ms = result.execution_time_ms.unwrap_or_default() as f64;
    let error_type = result.error.as_deref();
    manager
        .record_execution(
            &state.db.conn,
            &body.tool,
            &format!("user:{}", auth.user_id),
            success,
            latency_ms,
            error_type,
        )
        .await?;

    Ok(Json(json!(result)))
}

async fn list_sessions_handler(Auth(_auth): Auth) -> Result<Json<Value>, AppError> {
    let client = GroundingClient::new();
    let sessions: Vec<_> = client.list_sessions().into_iter().cloned().collect();
    Ok(Json(json!({ "sessions": sessions })))
}

async fn quality_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(tool_name): Path<String>,
) -> Result<Json<Value>, AppError> {
    let manager = ToolQualityManager::new(None);
    let score = manager.get_quality_score(&state.db.conn, &tool_name).await?;
    Ok(Json(json!({ "tool_name": tool_name, "score": score })))
}

async fn degraded_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Query(query): Query<DegradedQuery>,
) -> Result<Json<Value>, AppError> {
    let manager = ToolQualityManager::new(query.threshold);
    let tools = manager.get_degraded_tools(&state.db.conn).await?;
    Ok(Json(json!({ "degraded_tools": tools })))
}
