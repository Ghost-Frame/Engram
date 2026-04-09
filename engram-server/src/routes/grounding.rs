use std::sync::OnceLock;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use engram_lib::grounding::{GroundingClient, SessionConfig, BackendType};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::{error::AppError, extractors::Auth, state::AppState};

fn client() -> &'static RwLock<GroundingClient> {
    static CLIENT: OnceLock<RwLock<GroundingClient>> = OnceLock::new();
    CLIENT.get_or_init(|| RwLock::new(GroundingClient::new()))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/grounding/sessions", post(create_session).get(list_sessions))
        .route(
            "/grounding/sessions/{id}",
            get(get_session).delete(destroy_session),
        )
        .route("/grounding/tools", get(list_tools))
        .route("/grounding/execute", post(execute_tool))
        .route("/grounding/quality", get(get_quality))
        .route("/grounding/providers", get(list_providers))
}

#[derive(Debug, Deserialize)]
struct CreateSessionBody {
    pub name: Option<String>,
    pub backend: Option<String>,
    pub timeout_ms: Option<u64>,
    pub max_retries: Option<u32>,
    pub metadata: Option<Value>,
}

async fn create_session(
    Auth(_auth): Auth,
    Json(body): Json<CreateSessionBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let backend = match body.backend.as_deref() {
        Some("mcp") => BackendType::Mcp,
        Some("web") => BackendType::Web,
        Some("gui") => BackendType::Gui,
        Some("system") => BackendType::System,
        _ => BackendType::Shell,
    };

    let name = body
        .name
        .unwrap_or_else(|| format!("session-{}", chrono::Utc::now().timestamp_millis()));

    let config = SessionConfig {
        name,
        backend,
        timeout_ms: body.timeout_ms,
        max_retries: body.max_retries,
        metadata: body.metadata,
    };

    let session = client().write().await.create_session(&config);
    Ok((
        StatusCode::CREATED,
        Json(serde_json::to_value(session).unwrap_or(json!({}))),
    ))
}

async fn list_sessions(
    Auth(_auth): Auth,
) -> Result<Json<Value>, AppError> {
    let guard = client().read().await;
    let sessions = guard.list_sessions();
    let session_values: Vec<Value> = sessions
        .iter()
        .map(|s| serde_json::to_value(s).unwrap_or(json!({})))
        .collect();
    let count = session_values.len();
    Ok(Json(json!({ "sessions": session_values, "count": count })))
}

async fn get_session(
    Auth(_auth): Auth,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let guard = client().read().await;
    let sessions = guard.list_sessions();
    let session = sessions
        .iter()
        .find(|s| s.id == id)
        .ok_or_else(|| AppError(engram_lib::EngError::NotFound("Session not found".into())))?;
    Ok(Json(serde_json::to_value(session).unwrap_or(json!({}))))
}

async fn destroy_session(
    Auth(_auth): Auth,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    client().write().await.destroy_session(&id);
    Ok(Json(json!({ "destroyed": true, "id": id })))
}

#[derive(Debug, Deserialize)]
struct ToolsQuery {
    #[allow(dead_code)]
    pub refresh: Option<String>,
}

async fn list_tools(
    Auth(_auth): Auth,
    Query(_params): Query<ToolsQuery>,
) -> Result<Json<Value>, AppError> {
    let guard = client().read().await;
    let tools = guard.get_all_tools();
    let tools_json: Vec<Value> = tools
        .iter()
        .map(|t| serde_json::to_value(t).unwrap_or(json!({})))
        .collect();
    let count = tools_json.len();
    Ok(Json(json!({ "tools": tools_json, "count": count })))
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ExecuteBody {
    pub tool: String,
    pub args: Option<Value>,
    pub session_id: Option<String>,
    pub timeout_ms: Option<u64>,
}

async fn execute_tool(
    Auth(_auth): Auth,
    Json(body): Json<ExecuteBody>,
) -> Result<Json<Value>, AppError> {
    let tool_name = body.tool.trim();
    if tool_name.is_empty() {
        return Err(AppError(engram_lib::EngError::InvalidInput(
            "tool is required".into(),
        )));
    }

    let args = body.args.unwrap_or(json!({}));
    let guard = client().read().await;
    let result = guard.execute_tool(tool_name, &args, body.timeout_ms).await;
    Ok(Json(serde_json::to_value(result).unwrap_or(json!({}))))
}

async fn get_quality(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Query(params): Query<QualityQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(50).min(200);
    let degraded_only = params.degraded.as_deref() == Some("true");

    let qm = engram_lib::grounding::ToolQualityManager::new(None);

    if degraded_only {
        let tools = qm.get_degraded_tools(&state.db.conn).await.map_err(|e| {
            AppError(engram_lib::EngError::Internal(e.to_string()))
        })?;
        let records: Vec<Value> = tools
            .iter()
            .map(|(name, score)| json!({ "tool_name": name, "quality_score": score }))
            .collect();
        let count = records.len();
        Ok(Json(json!({ "records": records, "count": count })))
    } else {
        // Return all quality records up to limit
        let mut rows = state
            .db
            .conn
            .query(
                "SELECT tool_name, COUNT(*) as total, SUM(CASE WHEN success THEN 1 ELSE 0 END) as successes, AVG(latency_ms) as avg_latency FROM tool_quality_records GROUP BY tool_name ORDER BY total DESC LIMIT ?1",
                libsql::params![limit as i64],
            )
            .await
            .map_err(engram_lib::EngError::Database)?;

        let mut records = Vec::new();
        while let Some(r) = rows.next().await.map_err(engram_lib::EngError::Database)? {
            let total: i64 = r.get(0).unwrap_or(0);
            let successes: i64 = r.get(1).unwrap_or(0);
            let avg_latency: f64 = r.get(2).unwrap_or(0.0);
            let name: String = r.get::<String>(0).unwrap_or_default();
            let score = if total > 0 {
                successes as f64 / total as f64
            } else {
                1.0
            };
            records.push(json!({
                "tool_name": name,
                "total_calls": total,
                "total_successes": successes,
                "quality_score": score,
                "avg_execution_ms": avg_latency,
            }));
        }
        let count = records.len();
        Ok(Json(json!({ "records": records, "count": count })))
    }
}

#[derive(Debug, Deserialize)]
struct QualityQuery {
    pub limit: Option<usize>,
    pub degraded: Option<String>,
}

async fn list_providers(
    Auth(_auth): Auth,
) -> Result<Json<Value>, AppError> {
    // Currently only shell provider is registered
    Ok(Json(json!({
        "providers": [{ "name": "shell", "type": "shell", "status": "connected" }],
        "count": 1,
    })))
}
