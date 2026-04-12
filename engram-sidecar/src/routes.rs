use axum::{
    extract::State,
    http::StatusCode,
    middleware,
    routing::{get, post},
    Json, Router,
};
use engram_lib::llm::local::{CallOptions, Priority};
use engram_lib::memory::{
    self,
    search::hybrid_search,
    types::{SearchRequest, StoreRequest},
};
use serde::Deserialize;
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
    let has_embedder = state.embedder.read().await.is_some();
    Json(json!({
        "status": "ok",
        "session_id": session.id,
        "observation_count": session.observation_count,
        "stored_count": session.stored_count,
        "ended": session.ended,
        "llm_available": llm_available,
        "embedder_available": has_embedder,
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
    let embedding = {
        let embedder_guard = state.embedder.read().await;
        if let Some(ref embedder) = *embedder_guard {
            match embedder.embed(&body.query).await {
                Ok(emb) => Some(emb),
                Err(e) => {
                    tracing::warn!(user_id = state.user_id, error = %e, "embedding failed for recall");
                    None
                }
            }
        } else {
            None
        }
    };

    let req = SearchRequest {
        query: body.query,
        embedding,
        // SECURITY (SEC-MED-6): clamp caller-supplied limit.
        limit: Some(body.limit.min(100)),
        category: None,
        source: None,
        tags: None,
        threshold: None,
        user_id: Some(state.user_id),
        space_id: None,
        include_forgotten: Some(false),
        mode: None,
        question_type: None,
        expand_relationships: false,
        include_links: false,
        latest_only: true,
        source_filter: None,
    };

    // SECURITY (SEC-MED-6): do not leak table/column names or the
    // inner error string to unauthenticated callers. Log server-side only.
    let results = hybrid_search(&state.db, req).await.map_err(|e| {
        tracing::error!(user_id = state.user_id, error = %e, "sidecar hybrid_search failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "internal error" })),
        )
    })?;

    let session_id = {
        let session = state.session.read().await;
        session.id.clone()
    };

    Ok(Json(json!({
        "results": results,
        "count": results.len(),
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

    let mut stored = 0usize;
    for obs in &observations {
        let embedding = {
            let embedder_guard = state.embedder.read().await;
            if let Some(ref embedder) = *embedder_guard {
                match embedder.embed(&obs.content).await {
                    Ok(emb) => Some(emb),
                    Err(e) => {
                        tracing::warn!(user_id = state.user_id, error = %e, "embedding failed for observation");
                        None
                    }
                }
            } else {
                None
            }
        };

        let req = StoreRequest {
            content: format!("[{}] {}", obs.tool_name, obs.content),
            category: obs.category.clone(),
            source: state.source.clone(),
            importance: obs.importance,
            tags: Some(vec!["sidecar".to_string(), obs.tool_name.clone()]),
            embedding,
            session_id: Some(session_id.clone()),
            is_static: None,
            user_id: Some(state.user_id),
            space_id: None,
            parent_memory_id: None,
        };

        match memory::store(&state.db, req).await {
            Ok(result) => {
                stored += 1;
                if result.created {
                    tracing::debug!(id = result.id, tool = %obs.tool_name, "observation stored");
                } else if let Some(dup) = result.duplicate_of {
                    tracing::debug!(dup_of = dup, tool = %obs.tool_name, "observation was duplicate");
                }
            }
            Err(e) => {
                tracing::error!(tool = %obs.tool_name, user_id = state.user_id, error = %e, "failed to store observation");
            }
        }
    }

    stored
}
