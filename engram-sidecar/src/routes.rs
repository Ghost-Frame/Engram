use axum::{
    extract::State,
    http::StatusCode,
    middleware,
    routing::{get, post},
    Json, Router,
};
use engram_lib::llm::local::{CallOptions, Priority};
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::info;

use crate::auth::require_token;
use crate::session::Observation;
use crate::SidecarState;

const FLUSH_THRESHOLD: usize = 1;

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
        .route("/session/start", post(start_session))
        .route("/sessions", get(list_sessions))
        .layer(middleware::from_fn_with_state(state.clone(), require_token))
        .with_state(state)
}

async fn health(State(state): State<SidecarState>) -> Json<Value> {
    let sessions = state.sessions.read().await;
    let llm_available = state.llm.is_available();
    Json(json!({
        "status": "ok",
        "active_sessions": sessions.active_count(),
        "total_sessions": sessions.total_count(),
        "default_session_id": sessions.default_session_id,
        "llm_available": llm_available,
        "engram_url": state.engram_url,
    }))
}

// ---------------------------------------------------------------------------
// POST /session/start -- explicitly create a new session
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct StartSessionBody {
    /// Session ID. If omitted, a new UUID is generated.
    pub session_id: Option<String>,
}

async fn start_session(
    State(state): State<SidecarState>,
    Json(body): Json<StartSessionBody>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    let session_id = body
        .session_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let mut sessions = state.sessions.write().await;
    match sessions.start_session(session_id.clone()) {
        Ok(session) => {
            info!(session_id = %session.id, "session started");
            Ok((
                StatusCode::CREATED,
                Json(json!({
                    "session_id": session.id,
                    "started_at": session.started_at,
                })),
            ))
        }
        Err(e) => Err((
            StatusCode::CONFLICT,
            Json(json!({ "error": e.to_string() })),
        )),
    }
}

// ---------------------------------------------------------------------------
// GET /sessions -- list all sessions
// ---------------------------------------------------------------------------

async fn list_sessions(State(state): State<SidecarState>) -> Json<Value> {
    let sessions = state.sessions.read().await;
    let all = sessions.list();
    let active = sessions.active_count();

    Json(json!({
        "sessions": all,
        "active_count": active,
        "total_count": all.len(),
        "default_session_id": sessions.default_session_id,
    }))
}

// ---------------------------------------------------------------------------
// POST /observe
// ---------------------------------------------------------------------------

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
    /// Target session. If omitted, uses default session.
    pub session_id: Option<String>,
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
    let content = body.content.or(body.summary).unwrap_or_default();

    let obs = Observation {
        tool_name,
        content,
        importance: body.importance,
        category: body.category,
        timestamp: chrono::Utc::now(),
    };

    // Resolve session_id and add observation (auto-creates session if needed)
    let (pending_count, session_id) = {
        let mut sessions = state.sessions.write().await;
        let sid = sessions.resolve_id(body.session_id.as_deref()).to_string();
        let session = sessions.get_or_create(&sid);

        if session.ended {
            return Err((
                StatusCode::GONE,
                Json(json!({
                    "error": "session has ended",
                    "session_id": sid,
                })),
            ));
        }
        let count = session.add_observation(obs);
        (count, sid)
    };

    let flushed = if pending_count >= FLUSH_THRESHOLD {
        flush_pending(&state, &session_id).await
    } else {
        0
    };

    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "accepted": true,
            "session_id": session_id,
            "pending": pending_count.saturating_sub(flushed),
            "flushed": flushed,
        })),
    ))
}

// ---------------------------------------------------------------------------
// POST /recall
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RecallBody {
    /// Primary field name
    pub query: Option<String>,
    /// Legacy mnemonic field name (alias for query)
    pub message: Option<String>,
    #[serde(default = "default_recall_limit")]
    pub limit: usize,
    /// Optional session_id for response tagging (does not filter results).
    pub session_id: Option<String>,
}

fn default_recall_limit() -> usize {
    10
}

/// Try POST to primary path, fall back to alternate on 404.
async fn post_with_fallback(
    state: &SidecarState,
    primary: &str,
    fallback: &str,
    body: &Value,
) -> Result<reqwest::Response, (StatusCode, Json<Value>)> {
    let url = format!("{}{}", state.engram_url, primary);
    let mut req = state.client.post(&url).json(body);
    if let Some(ref api_key) = state.engram_api_key {
        req = req.header("Authorization", format!("Bearer {}", api_key));
    }

    let response = req.send().await.map_err(|e| {
        tracing::error!(error = %e, "engram server request failed");
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": "engram server unreachable" })),
        )
    })?;

    // If 404, try fallback path (supports both Node.js and Rust server)
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        tracing::debug!(primary = %primary, fallback = %fallback, "trying fallback path");
        let url = format!("{}{}", state.engram_url, fallback);
        let mut req = state.client.post(&url).json(body);
        if let Some(ref api_key) = state.engram_api_key {
            req = req.header("Authorization", format!("Bearer {}", api_key));
        }
        return req.send().await.map_err(|e| {
            tracing::error!(error = %e, "engram server fallback request failed");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "engram server unreachable" })),
            )
        });
    }

    Ok(response)
}

async fn recall(
    State(state): State<SidecarState>,
    Json(body): Json<RecallBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    // Accept both {query: "..."} (current) and {message: "..."} (legacy mnemonic)
    let query = body.query.or(body.message).unwrap_or_default();

    if query.is_empty() {
        return Ok(Json(json!({
            "results": [],
            "count": 0,
            "context": "",
        })));
    }

    let search_req = json!({
        "query": query,
        "limit": body.limit.min(100),
        "user_id": state.user_id,
        "include_forgotten": false,
        "latest_only": true,
    });

    // Try /search (Node.js), fall back to /memory/search (Rust)
    let response = post_with_fallback(&state, "/search", "/memory/search", &search_req).await?;

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

    // Resolve session_id for the response
    let session_id = {
        let sessions = state.sessions.read().await;
        sessions
            .resolve_id(body.session_id.as_deref())
            .to_string()
    };

    // Extract results array
    let empty_arr = json!([]);
    let results_arr = results.get("results").unwrap_or(&empty_arr);
    let count = results_arr.as_array().map(|a| a.len()).unwrap_or(0);

    // Build a "context" string for legacy hook consumers (mnemonic format)
    let context = if let Some(arr) = results_arr.as_array() {
        let lines: Vec<String> = arr
            .iter()
            .filter_map(|m| {
                let content = m.get("content")?.as_str()?;
                let cat = m
                    .get("category")
                    .and_then(|c| c.as_str())
                    .unwrap_or("general");
                let truncated = if content.len() > 180 {
                    format!("{}...", &content[..177])
                } else {
                    content.to_string()
                };
                Some(format!("[{}] {}", cat, truncated))
            })
            .take(5)
            .collect();
        if lines.is_empty() {
            String::new()
        } else {
            format!("Relevant memories:\n{}", lines.join("\n"))
        }
    } else {
        String::new()
    };

    Ok(Json(json!({
        "results": results_arr,
        "count": count,
        "context": context,
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
    /// Target session. If omitted, uses default session.
    pub session_id: Option<String>,
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

    // No LLM available: try re-probing once (Ollama may have started after sidecar)
    if !state.llm.is_available() {
        tracing::debug!(file = %file_path, "compress: LLM not available, re-probing");
        if !state.llm.probe().await {
            tracing::debug!(
                file = %file_path,
                "compress: LLM still unavailable after re-probe, fail-open"
            );
            return Json(json!({
                "compressed_output": null,
                "passthrough": true,
                "reason": "no_llm",
            }));
        }
        tracing::info!("compress: LLM now available after re-probe");
    }

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

    match state
        .llm
        .call(COMPRESS_SYSTEM_PROMPT, &user_prompt, Some(opts))
        .await
    {
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
                content: format!(
                    "[compressed {}] {}",
                    file_path,
                    &summary[..summary.len().min(200)]
                ),
                importance: 2,
                category: "discovery".to_string(),
                timestamp: chrono::Utc::now(),
            };

            // Add observation to the target session
            {
                let mut sessions = state.sessions.write().await;
                let sid = sessions
                    .resolve_id(body.session_id.as_deref())
                    .to_string();
                let session = sessions.get_or_create(&sid);
                session.add_observation(obs);
            }

            let input_bytes = output.len();
            let output_bytes = summary.len();
            let savings_pct = if input_bytes > 0 {
                100.0 * (1.0 - (output_bytes as f32 / input_bytes as f32))
            } else {
                0.0
            };

            Json(json!({
                "compressed_output": summary,
                "passthrough": false,
                "strategy": "llm_summary",
                "savings_pct": savings_pct,
                "input_bytes": input_bytes,
                "output_bytes": output_bytes,
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
// POST /end -- finalize a session
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct EndSessionBody {
    /// Session to end. If omitted, ends the default session.
    pub session_id: Option<String>,
}

async fn end_session(
    State(state): State<SidecarState>,
    Json(body): Json<EndSessionBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    // Resolve session_id
    let session_id = {
        let sessions = state.sessions.read().await;
        sessions.resolve_id(body.session_id.as_deref()).to_string()
    };

    // Flush pending observations for this session
    let flushed = flush_pending(&state, &session_id).await;

    // End the session
    let mut sessions = state.sessions.write().await;
    match sessions.end_session(&session_id) {
        Ok(session_info) => {
            let duration = chrono::Utc::now()
                .signed_duration_since(session_info.started_at)
                .num_seconds();

            info!(
                session_id = %session_info.id,
                user_id = state.user_id,
                observations = session_info.observation_count,
                stored = session_info.stored_count,
                duration_secs = duration,
                active_remaining = sessions.active_count(),
                "session ended"
            );

            Ok(Json(json!({
                "ended": true,
                "session_id": session_info.id,
                "flushed": flushed,
                "observation_count": session_info.observation_count,
                "stored_count": session_info.stored_count,
                "duration_secs": duration,
                "active_sessions_remaining": sessions.active_count(),
            })))
        }
        Err(e) => {
            let status = match &e {
                crate::session::SessionError::NotFound(_) => StatusCode::NOT_FOUND,
                crate::session::SessionError::AlreadyEnded(_) => StatusCode::GONE,
                crate::session::SessionError::AlreadyExists(_) => StatusCode::CONFLICT,
            };
            Err((status, Json(json!({ "error": e.to_string() }))))
        }
    }
}

// ---------------------------------------------------------------------------
// flush_pending -- drain and store observations for a specific session
// ---------------------------------------------------------------------------

async fn flush_pending(state: &SidecarState, session_id: &str) -> usize {
    let observations = {
        let mut sessions = state.sessions.write().await;
        match sessions.get_mut(session_id) {
            Some(session) => session.drain_pending(),
            None => return 0,
        }
    };

    if observations.is_empty() {
        return 0;
    }

    let mut stored = 0usize;

    for obs in &observations {
        let req = json!({
            "content": format!("[{}] {}", obs.tool_name, obs.content),
            "category": obs.category,
            "source": state.source,
            "importance": obs.importance,
            "tags": vec!["sidecar".to_string(), obs.tool_name.clone()],
            "session_id": session_id,
            "user_id": state.user_id,
        });

        // Try /store (Node.js), fall back to /memory/store (Rust)
        match post_with_fallback(state, "/store", "/memory/store", &req).await {
            Ok(response) if response.status().is_success() => {
                stored += 1;
                tracing::debug!(
                    tool = %obs.tool_name,
                    session_id = %session_id,
                    "observation stored via engram server"
                );
            }
            Ok(response) => {
                tracing::error!(
                    tool = %obs.tool_name,
                    session_id = %session_id,
                    status = %response.status(),
                    "engram server rejected observation"
                );
            }
            Err(_) => {
                tracing::error!(
                    tool = %obs.tool_name,
                    session_id = %session_id,
                    user_id = state.user_id,
                    "failed to send observation to engram server"
                );
            }
        }
    }

    stored
}
