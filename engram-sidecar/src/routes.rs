use axum::{
    extract::State,
    http::StatusCode,
    middleware,
    routing::{get, post},
    Json, Router,
};
use engram_lib::llm::local::{CallOptions, Priority};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::info;

use crate::auth::require_token;
use crate::session::Observation;
use crate::SidecarState;

const FLUSH_THRESHOLD: usize = 5;

/// Byte threshold below which /compress passes content through without LLM.
const COMPRESS_PASSTHROUGH_BYTES: usize = 2000;

/// Maximum bytes of tool_output we will send to the LLM for compression.
/// Anything beyond this is truncated before prompting.
const COMPRESS_MAX_INPUT_BYTES: usize = 50_000;

pub fn router(state: SidecarState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/observe", post(observe))
        .route("/recall", post(recall))
        .route("/compress", post(compress))
        .route("/end", post(end_session))
        .layer(middleware::from_fn_with_state(state.clone(), require_token))
        .with_state(state)
}

async fn health(State(state): State<SidecarState>) -> Json<Value> {
    let session = state.session.read().await;
    let llm_available = state.llm.as_ref().map(|l| l.is_available()).unwrap_or(false);
    // SECURITY: only expose liveness-level info without auth. Internal state
    // (session_id, counters) is stripped to avoid leaking operational details.
    Json(json!({
        "status": "ok",
        "ended": session.ended,
        "llm_available": llm_available,
        "engram_url": state.engram_url,
    }))
}

#[derive(Debug, Deserialize)]
struct ObserveBody {
    /// Current format field name
    pub tool_name: Option<String>,
    /// Legacy mnemonic format field name (alias for tool_name)
    pub tool: Option<String>,
    /// Current format field name
    pub content: Option<String>,
    /// Legacy mnemonic format field name (alias for content)
    pub summary: Option<String>,
    #[serde(default = "default_importance")]
    pub importance: i32,
    #[serde(default = "default_category")]
    pub category: String,
}

fn default_importance() -> i32 {
    3
}

fn default_category() -> String {
    "discovery".to_string()
}

async fn observe(
    State(state): State<SidecarState>,
    Json(body): Json<ObserveBody>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    // Accept both formats: {tool_name, content} (current) or {tool, summary} (legacy)
    let tool_name = body
        .tool_name
        .or(body.tool)
        .unwrap_or_else(|| "unknown".to_string());
    let content = body
        .content
        .or(body.summary)
        .unwrap_or_default();

    let obs = Observation {
        tool_name,
        content,
        importance: body.importance,
        category: body.category,
        timestamp: chrono::Utc::now(),
    };

    let pending_count = {
        let mut session = state.session.write().await;
        if session.ended {
            return Err((
                StatusCode::GONE,
                Json(json!({ "error": "session has ended" })),
            ));
        }
        session.add_observation(obs)
    };

    let flushed = if pending_count >= FLUSH_THRESHOLD {
        flush_pending(&state).await
    } else {
        0
    };

    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "accepted": true,
            "pending": pending_count.saturating_sub(flushed),
            "flushed": flushed,
        })),
    ))
}

#[derive(Debug, Deserialize)]
struct RecallBody {
    pub query: String,
    #[serde(default = "default_recall_limit")]
    pub limit: usize,
}

fn default_recall_limit() -> usize {
    10
}

async fn recall(
    State(state): State<SidecarState>,
    Json(body): Json<RecallBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    // Build search request for Engram server
    let search_req = json!({
        "query": body.query,
        "limit": body.limit.min(100),
        "user_id": state.user_id,
        "include_forgotten": false,
        "latest_only": true,
    });

    let url = format!("{}/memory/search", state.engram_url);
    let mut req = state.client.post(&url).json(&search_req);
    if let Some(ref api_key) = state.engram_api_key {
        req = req.header("Authorization", format!("Bearer {}", api_key));
    }

    let response = req.send().await.map_err(|e| {
        tracing::error!(user_id = state.user_id, error = %e, "engram server request failed");
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": "engram server unreachable" })),
        )
    })?;

    if !response.status().is_success() {
        let status = response.status();
        tracing::error!(user_id = state.user_id, status = %status, "engram server returned error");
        return Err((
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": format!("engram server error: {}", status) })),
        ));
    }

    let results: Value = response.json().await.map_err(|e| {
        tracing::error!(user_id = state.user_id, error = %e, "failed to parse engram response");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "invalid response from engram server" })),
        )
    })?;

    let session_id = {
        let session = state.session.read().await;
        session.id.clone()
    };

    Ok(Json(json!({
        "results": results.get("results").unwrap_or(&json!([])),
        "count": results.get("results").and_then(|r| r.as_array()).map(|a| a.len()).unwrap_or(0),
        "session_id": session_id,
    })))
}

// ---------------------------------------------------------------------------
// POST /compress -- LLM-based file content summarization
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CompressBody {
    pub tool_name: String,
    pub tool_input: Option<Value>,
    pub tool_output: String,
}

const COMPRESS_SYSTEM_PROMPT: &str = "\
You are a code summarizer for an AI coding agent's memory system. \
Given the contents of a file that was read by a tool, produce a concise summary that captures: \
1) What the file is (type, purpose) \
2) Key structures, functions, or classes defined \
3) Important configuration values or constants \
4) Any notable patterns or dependencies \
\
Be extremely concise. Output ONLY the summary, no preamble. \
Target 200-400 words. Preserve exact names of functions, types, and variables.";

async fn compress(
    State(state): State<SidecarState>,
    Json(body): Json<CompressBody>,
) -> Json<Value> {
    let output = &body.tool_output;
    let file_path = body
        .tool_input
        .as_ref()
        .and_then(|v| v.get("filePath").or_else(|| v.get("file_path")))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    // Short content: pass through without LLM
    if output.len() <= COMPRESS_PASSTHROUGH_BYTES {
        tracing::debug!(
            file = %file_path,
            bytes = output.len(),
            "compress: passthrough (below threshold)"
        );
        return Json(json!({
            "compressed_output": null,
            "passthrough": true,
            "reason": "below_threshold",
        }));
    }

    // No LLM available: fail open
    let Some(ref llm) = state.llm else {
        tracing::debug!(
            file = %file_path,
            "compress: no LLM available, fail-open"
        );
        return Json(json!({
            "compressed_output": null,
            "passthrough": true,
            "reason": "no_llm",
        }));
    };

    // Truncate input if enormous
    let input_for_llm = if output.len() > COMPRESS_MAX_INPUT_BYTES {
        &output[..COMPRESS_MAX_INPUT_BYTES]
    } else {
        output.as_str()
    };

    let user_prompt = format!(
        "File: {}\nTool: {}\n\n---\n{}",
        file_path, body.tool_name, input_for_llm
    );

    let opts = CallOptions {
        max_tokens: Some(800),
        temperature: Some(0.1),
        priority: Priority::Hot,
        timeout_ms: Some(10_000),
        ..Default::default()
    };

    match llm.call(COMPRESS_SYSTEM_PROMPT, &user_prompt, Some(opts)).await {
        Ok(summary) => {
            tracing::info!(
                file = %file_path,
                input_bytes = output.len(),
                output_bytes = summary.len(),
                "compress: summarized"
            );

            // Also record as an observation for session tracking
            let obs = Observation {
                tool_name: body.tool_name.clone(),
                content: format!("[compressed {}] {}", file_path, &summary[..summary.len().min(200)]),
                importance: 2,
                category: "discovery".to_string(),
                timestamp: chrono::Utc::now(),
            };
            {
                let mut session = state.session.write().await;
                session.add_observation(obs);
            }

            Json(json!({
                "compressed_output": summary,
                "passthrough": false,
                "input_bytes": output.len(),
                "output_bytes": summary.len(),
            }))
        }
        Err(e) => {
            tracing::warn!(
                file = %file_path,
                error = %e,
                "compress: LLM failed, fail-open"
            );
            Json(json!({
                "compressed_output": null,
                "passthrough": true,
                "reason": format!("llm_error: {}", e),
            }))
        }
    }
}

// ---------------------------------------------------------------------------
// POST /end -- finalize session
// ---------------------------------------------------------------------------

async fn end_session(
    State(state): State<SidecarState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let flushed = flush_pending(&state).await;

    let mut session = state.session.write().await;
    session.end();

    let duration = chrono::Utc::now()
        .signed_duration_since(session.started_at)
        .num_seconds();

    info!(
        session_id = %session.id,
        user_id = state.user_id,
        observations = session.observation_count,
        stored = session.stored_count,
        duration_secs = duration,
        "session ended"
    );

    Ok(Json(json!({
        "ended": true,
        "session_id": session.id,
        "flushed": flushed,
        "observation_count": session.observation_count,
        "stored_count": session.stored_count,
        "duration_secs": duration,
    })))
}

/// Request body for Engram server /memory/store endpoint
#[derive(Serialize)]
struct StoreRequest {
    content: String,
    category: String,
    source: String,
    importance: i32,
    tags: Vec<String>,
    session_id: Option<String>,
    user_id: Option<i64>,
}

async fn flush_pending(state: &SidecarState) -> usize {
    let observations = {
        let mut session = state.session.write().await;
        session.drain_pending()
    };

    if observations.is_empty() {
        return 0;
    }

    let session_id = {
        let session = state.session.read().await;
        session.id.clone()
    };

    let url = format!("{}/memory/store", state.engram_url);
    let mut stored = 0usize;

    for obs in &observations {
        let req = StoreRequest {
            content: format!("[{}] {}", obs.tool_name, obs.content),
            category: obs.category.clone(),
            source: state.source.clone(),
            importance: obs.importance,
            tags: vec!["sidecar".to_string(), obs.tool_name.clone()],
            session_id: Some(session_id.clone()),
            user_id: Some(state.user_id),
        };

        let mut http_req = state.client.post(&url).json(&req);
        if let Some(ref api_key) = state.engram_api_key {
            http_req = http_req.header("Authorization", format!("Bearer {}", api_key));
        }

        match http_req.send().await {
            Ok(response) if response.status().is_success() => {
                stored += 1;
                tracing::debug!(tool = %obs.tool_name, "observation stored via engram server");
            }
            Ok(response) => {
                tracing::error!(
                    tool = %obs.tool_name,
                    status = %response.status(),
                    "engram server rejected observation"
                );
            }
            Err(e) => {
                tracing::error!(
                    tool = %obs.tool_name,
                    user_id = state.user_id,
                    error = %e,
                    "failed to send observation to engram server"
                );
            }
        }
    }

    stored
}
