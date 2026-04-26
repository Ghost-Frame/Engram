use std::sync::Arc;
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    middleware,
    routing::{get, post},
    Json, Router,
};
use kleos_lib::llm::{CallOptions, Priority};
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::info;

use crate::auth::require_token;
use crate::metrics;
use crate::session::Observation;
use crate::SidecarState;

// ---------------------------------------------------------------------------
// Health cache -- 5s TTL upstream probe result cached in an ArcSwap so
// concurrent /health requests share one upstream round-trip per window.
// ---------------------------------------------------------------------------

struct HealthCache {
    upstream_reachable: bool,
    fetched_at: Instant,
}

static HEALTH_CACHE: std::sync::LazyLock<ArcSwap<HealthCache>> = std::sync::LazyLock::new(|| {
    ArcSwap::from_pointee(HealthCache {
        upstream_reachable: false,
        // Expired so the first /health always probes.
        fetched_at: Instant::now()
            .checked_sub(Duration::from_secs(60))
            .unwrap_or_else(Instant::now),
    })
});

const HEALTH_CACHE_TTL: Duration = Duration::from_secs(5);
const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

// ---------------------------------------------------------------------------
// Router -- /metrics is outside the auth layer so Prometheus scrapers don't
// need the sidecar bearer token.
// ---------------------------------------------------------------------------

pub fn router(state: SidecarState) -> Router {
    Router::new().route("/metrics", get(metrics_handler)).merge(
        Router::new()
            .route("/health", get(health))
            .route("/observe", post(observe))
            .route("/recall", post(recall))
            .route("/compress", post(compress))
            .route("/end", post(end_session))
            .route("/session/start", post(start_session))
            .route("/session/{id}/resume", post(resume_session))
            .route("/sessions", get(list_sessions))
            .layer(middleware::from_fn_with_state(state.clone(), require_token))
            .with_state(state),
    )
}

async fn metrics_handler() -> (StatusCode, String) {
    (StatusCode::OK, metrics::render())
}

// ---------------------------------------------------------------------------
// POST /session/{id}/resume
//
// Previously rehydrated from SQLite. Now queries Kleos for observations stored
// under this session tag to rebuild metadata. The pending queue is always empty
// after resume -- in-flight observations from the previous run were lost when
// the process exited, which is the accepted trade-off for removing the
// local SQLite dependency.
// ---------------------------------------------------------------------------

async fn resume_session(
    State(state): State<SidecarState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    // Live in-memory copy is authoritative.
    {
        let sessions = state.sessions.read().await;
        if let Some(s) = sessions.get(&id) {
            return Ok(Json(json!({
                "session_id": s.id,
                "started_at": s.started_at,
                "observation_count": s.observation_count,
                "stored_count": s.stored_count,
                "pending_count": s.pending.len(),
                "ended": s.ended,
                "source": "in_memory",
            })));
        }
    }

    // Query Kleos for the count of observations stored for this session.
    let url_str = format!("{}/memory/search", state.kleos_url);
    let url = kleos_lib::net::validate_outbound_url(&url_str).map_err(|e| {
        tracing::error!(error = %e, "resume: kleos url rejected");
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": "kleos url invalid" })),
        )
    })?;

    let search_req = json!({
        "query": "",
        "limit": 1,
        "session_id": id,
        "tags": ["sidecar"],
        "include_forgotten": false,
        "latest_only": false,
        "count_only": true,
    });

    let mut req = state.client.post(url).json(&search_req);
    if let Some(ref api_key) = state.kleos_api_key {
        req = req.header("Authorization", format!("Bearer {}", api_key));
    }

    let stored_count = match req.send().await {
        Ok(resp) if resp.status().is_success() => {
            let body: Value = resp.json().await.unwrap_or_default();
            body.get("total").and_then(|v| v.as_u64()).unwrap_or(0) as usize
        }
        Ok(resp) => {
            tracing::warn!(
                session_id = %id,
                status = %resp.status(),
                "resume: Kleos search returned non-success; treating as 0 stored"
            );
            0
        }
        Err(e) => {
            tracing::warn!(session_id = %id, error = %e, "resume: Kleos unreachable");
            0
        }
    };

    {
        let mut sessions = state.sessions.write().await;
        let session = sessions.get_or_create(&id);
        session.stored_count = stored_count;
        session.observation_count = stored_count;
    }

    Ok(Json(json!({
        "session_id": id,
        "stored_count": stored_count,
        "observation_count": stored_count,
        "pending_count": 0,
        "ended": false,
        "source": "kleos",
    })))
}

async fn health(State(state): State<SidecarState>) -> Json<Value> {
    let upstream_reachable = probe_upstream_cached(&state).await;

    let (pending_depth, active_sessions) = {
        let sessions = state.sessions.read().await;
        let pending: usize = sessions.list().iter().map(|s| s.pending_count).sum();
        let active = sessions.active_count();
        (pending, active)
    };

    metrics::set_active_sessions(active_sessions as f64);
    metrics::set_pending_depth(pending_depth as f64);

    let llm_available = state.llm.is_available();
    Json(json!({
        "status": "ok",
        "upstream_reachable": upstream_reachable,
        "pending_depth": pending_depth,
        "dead_letter_depth": 0,
        "retry_in_flight": false,
        "active_sessions": active_sessions,
        "llm_available": llm_available,
        "kleos_url": state.kleos_url,
    }))
}

/// Probe upstream /health with a 2s timeout. Cached for 5s to avoid hammering.
async fn probe_upstream_cached(state: &SidecarState) -> bool {
    let cached = HEALTH_CACHE.load();
    if cached.fetched_at.elapsed() < HEALTH_CACHE_TTL {
        return cached.upstream_reachable;
    }

    let url_str = format!("{}/health", state.kleos_url);
    let reachable = match kleos_lib::net::validate_outbound_url(&url_str) {
        Ok(url) => {
            let mut req = state.client.head(url);
            if let Some(ref api_key) = state.kleos_api_key {
                req = req.header("Authorization", format!("Bearer {}", api_key));
            }
            match tokio::time::timeout(HEALTH_PROBE_TIMEOUT, req.send()).await {
                // 405 Method Not Allowed means the endpoint exists but doesn't support HEAD -- upstream is reachable.
                Ok(Ok(r)) => r.status().is_success() || r.status().as_u16() == 405,
                _ => false,
            }
        }
        Err(_) => false,
    };

    let result_label = if reachable { "ok" } else { "fail" };
    metrics::inc_health_probe(result_label);

    HEALTH_CACHE.store(Arc::new(HealthCache {
        upstream_reachable: reachable,
        fetched_at: Instant::now(),
    }));

    reachable
}

// ---------------------------------------------------------------------------
// POST /session/start
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct StartSessionBody {
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
            let sid = session.id.clone();
            info!(session_id = %sid, "session started");
            state
                .syntheos
                .upsert_chiasm_task(&sid, "active", "session started");
            state.syntheos.publish_axon(
                "sidecar:sessions",
                "started",
                json!({ "session_id": sid }),
            );
            Ok((
                StatusCode::CREATED,
                Json(json!({
                    "session_id": sid,
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
// GET /sessions
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
    pub tool_name: Option<String>,
    pub tool: Option<String>,
    pub content: Option<String>,
    pub summary: Option<String>,
    #[serde(default = "default_importance")]
    pub importance: i32,
    #[serde(default = "default_category")]
    pub category: String,
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

        // Hard cap -- loud 503 rather than unbounded queue growth when upstream is down.
        if session.pending.len() >= state.max_pending_per_session {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "pending queue full -- upstream may be down",
                    "session_id": sid,
                    "pending": session.pending.len(),
                    "limit": state.max_pending_per_session,
                })),
            ));
        }

        let count = session.add_observation(obs);
        (count, sid)
    };

    metrics::inc_observations(1);

    let flushed = if pending_count >= state.batch_size {
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
    pub query: Option<String>,
    pub message: Option<String>,
    #[serde(default = "default_recall_limit")]
    pub limit: usize,
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
    let url_str = format!("{}{}", state.kleos_url, primary);
    let url = kleos_lib::net::validate_outbound_url(&url_str).map_err(|e| {
        tracing::error!(error = %e, "kleos url rejected");
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": "kleos url invalid" })),
        )
    })?;
    let mut req = state.client.post(url).json(body);
    if let Some(ref api_key) = state.kleos_api_key {
        req = req.header("Authorization", format!("Bearer {}", api_key));
    }

    let response = req.send().await.map_err(|e| {
        tracing::error!(error = %e, "kleos server request failed");
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": "kleos server unreachable" })),
        )
    })?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        tracing::debug!(primary = %primary, fallback = %fallback, "trying fallback path");
        let url_str = format!("{}{}", state.kleos_url, fallback);
        let url = kleos_lib::net::validate_outbound_url(&url_str).map_err(|e| {
            tracing::error!(error = %e, "kleos fallback url rejected");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "kleos url invalid" })),
            )
        })?;
        let mut req = state.client.post(url).json(body);
        if let Some(ref api_key) = state.kleos_api_key {
            req = req.header("Authorization", format!("Bearer {}", api_key));
        }
        return req.send().await.map_err(|e| {
            tracing::error!(error = %e, "kleos server fallback request failed");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "kleos server unreachable" })),
            )
        });
    }

    Ok(response)
}

async fn recall(
    State(state): State<SidecarState>,
    Json(body): Json<RecallBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
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

    let response = post_with_fallback(&state, "/search", "/memory/search", &search_req).await?;

    if !response.status().is_success() {
        let status = response.status();
        tracing::error!(user_id = state.user_id, status = %status, "kleos server returned error");
        return Err((
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": format!("kleos server error: {}", status) })),
        ));
    }

    let results: Value = response.json().await.map_err(|e| {
        tracing::error!(user_id = state.user_id, error = %e, "failed to parse kleos response");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "invalid response from kleos server" })),
        )
    })?;

    let session_id = {
        let sessions = state.sessions.read().await;
        sessions.resolve_id(body.session_id.as_deref()).to_string()
    };

    let empty_arr = json!([]);
    let results_arr = results.get("results").unwrap_or(&empty_arr);
    let count = results_arr.as_array().map(|a| a.len()).unwrap_or(0);

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
// POST /compress
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CompressBody {
    pub tool_name: String,
    pub tool_input: Option<Value>,
    pub tool_output: String,
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
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let output = &body.tool_output;
    let file_path = body
        .tool_input
        .as_ref()
        .and_then(|v| v.get("filePath").or_else(|| v.get("file_path")))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    let t0 = Instant::now();

    // Reject payloads that exceed the configured LLM input cap.
    if output.len() > state.compress_max_input_bytes {
        tracing::warn!(
            file = %file_path,
            bytes = output.len(),
            limit = state.compress_max_input_bytes,
            "compress: payload too large"
        );
        metrics::inc_compress("too_large");
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({
                "error": "payload too large for compression",
                "bytes": output.len(),
                "limit": state.compress_max_input_bytes,
            })),
        ));
    }

    if output.len() <= state.compress_passthrough_bytes {
        tracing::debug!(
            file = %file_path,
            bytes = output.len(),
            "compress: passthrough (below threshold)"
        );
        metrics::inc_compress("passthrough");
        metrics::record_compress_latency(t0.elapsed().as_secs_f64());
        return Ok(Json(json!({
            "compressed_output": null,
            "passthrough": true,
            "reason": "below_threshold",
        })));
    }

    if !state.llm.is_available() {
        tracing::debug!(file = %file_path, "compress: LLM not available, re-probing");
        if !state.llm.probe().await {
            tracing::debug!(
                file = %file_path,
                "compress: LLM still unavailable after re-probe, fail-open"
            );
            metrics::inc_compress("no_llm");
            metrics::record_compress_latency(t0.elapsed().as_secs_f64());
            return Ok(Json(json!({
                "compressed_output": null,
                "passthrough": true,
                "reason": "no_llm",
            })));
        }
        tracing::info!("compress: LLM now available after re-probe");
    }

    let input_for_llm = output.as_str();

    let user_prompt = format!(
        "File: {}\nTool: {}\n\n---\n{}",
        file_path, body.tool_name, input_for_llm
    );

    let opts = CallOptions {
        max_tokens: Some(800),
        temperature: Some(0.1),
        priority: Priority::Hot,
        timeout_ms: Some(state.compress_timeout_ms),
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

            {
                let mut sessions = state.sessions.write().await;
                let sid = sessions.resolve_id(body.session_id.as_deref()).to_string();
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

            metrics::inc_compress("ok");
            metrics::record_compress_latency(t0.elapsed().as_secs_f64());

            let compress_narrative = format!(
                "compressed {} ({} -> {} bytes, {:.1}% savings)",
                file_path, input_bytes, output_bytes, savings_pct
            );
            state
                .syntheos
                .log_broca("compression", "compressed", &compress_narrative);

            Ok(Json(json!({
                "compressed_output": summary,
                "passthrough": false,
                "strategy": "llm_summary",
                "savings_pct": savings_pct,
                "input_bytes": input_bytes,
                "output_bytes": output_bytes,
            })))
        }
        Err(e) => {
            tracing::warn!(
                file = %file_path,
                error = %e,
                "compress: LLM failed, fail-open"
            );
            metrics::inc_compress("llm_error");
            metrics::record_compress_latency(t0.elapsed().as_secs_f64());
            Ok(Json(json!({
                "compressed_output": null,
                "passthrough": true,
                "reason": format!("llm_error: {}", e),
            })))
        }
    }
}

// ---------------------------------------------------------------------------
// POST /end
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct EndSessionBody {
    pub session_id: Option<String>,
}

async fn end_session(
    State(state): State<SidecarState>,
    Json(body): Json<EndSessionBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let session_id = {
        let sessions = state.sessions.read().await;
        sessions.resolve_id(body.session_id.as_deref()).to_string()
    };

    let flushed = flush_pending(&state, &session_id).await;

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

            let final_summary = format!(
                "session ended: {} observations stored in {}s",
                session_info.stored_count, duration
            );
            state
                .syntheos
                .upsert_chiasm_task(&session_info.id, "completed", &final_summary);
            state.syntheos.publish_axon(
                "sidecar:sessions",
                "ended",
                json!({
                    "session_id": session_info.id,
                    "flushed": flushed,
                    "observation_count": session_info.observation_count,
                    "stored_count": session_info.stored_count,
                    "duration_secs": duration,
                }),
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
// flush_pending -- drain and store observations with retry + metric recording
//
// Wraps the upstream POST in retry_with_backoff (3 tries, 100ms base, 2x).
// On exhausted retries or partial upstream success the failed observations
// are requeued at the head of session.pending so the next flush cycle retries
// them. stored_count is bumped only for observations the upstream confirmed.
// ---------------------------------------------------------------------------

pub async fn flush_pending(state: &SidecarState, session_id: &str) -> usize {
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

    let t0 = Instant::now();

    let ops: Vec<Value> = observations
        .iter()
        .map(|obs| {
            json!({
                "op": "store",
                "body": {
                    "content": format!("[{}] {}", obs.tool_name, obs.content),
                    "category": obs.category,
                    "source": state.source,
                    "importance": obs.importance,
                    "tags": vec!["sidecar".to_string(), obs.tool_name.clone()],
                    "session_id": session_id,
                }
            })
        })
        .collect();
    let batch_req = json!({ "ops": ops });

    let url_str = format!("{}/batch", state.kleos_url);
    let url = match kleos_lib::net::validate_outbound_url(&url_str) {
        Ok(u) => u,
        Err(e) => {
            tracing::error!(error = %e, "batch flush: kleos url rejected");
            finalize_flush(state, session_id, Vec::new(), observations).await;
            metrics::inc_flush("fail");
            return 0;
        }
    };

    let total = observations.len();
    let client = state.client.clone();
    let api_key = state.kleos_api_key.clone();
    let sid = session_id.to_string();

    // Closure returns Option<Vec<bool>>: None means /batch is unavailable
    // (server returned 404) so the caller should fall back to /store. Some
    // returns a per-op success flag matching the request order. The server
    // may stop on first failure, so the flags vector can be shorter than
    // the request; callers must treat missing indices as failures.
    let result = kleos_lib::resilience::retry_with_backoff(3, Duration::from_millis(100), || {
        let url = url.clone();
        let batch_req = batch_req.clone();
        let api_key = api_key.clone();
        let client = client.clone();
        let sid = sid.clone();
        async move {
            let mut req = client.post(url).json(&batch_req);
            if let Some(ref key) = api_key {
                req = req.header("Authorization", format!("Bearer {}", key));
            }

            let response = req.send().await.map_err(|e| {
                tracing::warn!(session_id = %sid, error = %e, "batch flush: kleos unreachable");
                e.to_string()
            })?;

            // 404 means old server -- sentinel None tells caller to use per-obs fallback.
            if response.status() == reqwest::StatusCode::NOT_FOUND {
                return Ok::<Option<Vec<bool>>, String>(None);
            }

            let status = response.status();
            if !status.is_success() && status != reqwest::StatusCode::MULTI_STATUS {
                return Err(format!("batch rejected: {}", status));
            }

            let body: Value = response
                .json()
                .await
                .map_err(|e| format!("parse /batch response: {e}"))?;

            let flags = body
                .get("results")
                .and_then(|r| r.as_array())
                .map(|arr| {
                    arr.iter()
                        .map(|r| r.get("success").and_then(|v| v.as_bool()).unwrap_or(false))
                        .collect::<Vec<bool>>()
                })
                .unwrap_or_default();

            Ok(Some(flags))
        }
    })
    .await;

    metrics::record_flush_latency(t0.elapsed().as_secs_f64());

    match result {
        Ok(Some(flags)) => {
            let (successful, failed) = partition_by_flags(observations, &flags, total);
            let n = successful.len();
            let failed_count = failed.len();

            tracing::debug!(
                session_id = %session_id,
                total,
                stored = n,
                requeued = failed_count,
                "batch flush complete"
            );

            if failed_count == 0 {
                metrics::inc_flush("ok");
            } else if n > 0 {
                metrics::inc_flush("partial");
            } else {
                metrics::inc_flush("fail");
            }

            if n > 0 {
                fire_syntheos_flush_hooks(state, session_id, n, &successful);
            }
            finalize_flush(state, session_id, successful, failed).await;
            n
        }
        Ok(None) => {
            tracing::debug!(
                session_id = %session_id,
                "batch flush: /batch not available, falling back to /store"
            );
            let (successful, failed) =
                flush_pending_fallback(state, session_id, observations).await;
            let n = successful.len();
            let failed_count = failed.len();

            if failed_count == 0 && n > 0 {
                metrics::inc_flush("ok");
            } else if n > 0 {
                metrics::inc_flush("partial");
            } else {
                metrics::inc_flush("fail");
            }

            if n > 0 {
                fire_syntheos_flush_hooks(state, session_id, n, &successful);
            }
            finalize_flush(state, session_id, successful, failed).await;
            n
        }
        Err(e) => {
            tracing::error!(
                session_id = %session_id,
                error = %e,
                "batch flush: all retries exhausted, restoring to pending"
            );
            finalize_flush(state, session_id, Vec::new(), observations).await;
            metrics::inc_flush("fail");
            0
        }
    }
}

/// Split `observations` into (successful, failed) using per-op `flags` from
/// the /batch response. Any index beyond `flags.len()` is treated as failed
/// because the server stops on first failure and truncates the results array.
fn partition_by_flags(
    observations: Vec<Observation>,
    flags: &[bool],
    total: usize,
) -> (Vec<Observation>, Vec<Observation>) {
    let mut successful = Vec::with_capacity(total);
    let mut failed = Vec::with_capacity(total);
    for (i, obs) in observations.into_iter().enumerate() {
        if flags.get(i).copied().unwrap_or(false) {
            successful.push(obs);
        } else {
            failed.push(obs);
        }
    }
    (successful, failed)
}

/// Fire Syntheos hooks after a successful batch flush. Separated so both the
/// primary batch path and the per-observation fallback path share the same calls.
fn fire_syntheos_flush_hooks(
    state: &SidecarState,
    session_id: &str,
    count: usize,
    observations: &[Observation],
) {
    let tools: Vec<String> = {
        let mut seen = std::collections::HashSet::new();
        observations
            .iter()
            .filter(|o| seen.insert(o.tool_name.clone()))
            .map(|o| o.tool_name.clone())
            .collect()
    };

    state.syntheos.publish_axon(
        "sidecar:observations",
        "flushed",
        json!({
            "session_id": session_id,
            "count": count,
            "tools": tools,
        }),
    );

    let narrative = format!("flushed {} observations for session {}", count, session_id);
    state
        .syntheos
        .log_broca("observation", "flushed", &narrative);

    let summary = format!("flushed {} obs", count);
    state
        .syntheos
        .upsert_chiasm_task(session_id, "active", &summary);
}

/// Record the outcome of a flush cycle under a single write lock. Bumps
/// `stored_count` by the number of confirmed successes and requeues any
/// failed observations at the head of the pending queue.
async fn finalize_flush(
    state: &SidecarState,
    session_id: &str,
    successful: Vec<Observation>,
    failed: Vec<Observation>,
) {
    if successful.is_empty() && failed.is_empty() {
        return;
    }
    let mut sessions = state.sessions.write().await;
    if let Some(session) = sessions.get_mut(session_id) {
        session.record_stored(successful.len());
        session.requeue(failed);
    }
}

/// Per-observation fallback for servers without /batch. Returns the
/// successfully stored observations and the ones the caller must requeue.
async fn flush_pending_fallback(
    state: &SidecarState,
    session_id: &str,
    observations: Vec<Observation>,
) -> (Vec<Observation>, Vec<Observation>) {
    let mut successful = Vec::with_capacity(observations.len());
    let mut failed = Vec::new();
    for obs in observations.into_iter() {
        let req = json!({
            "content": format!("[{}] {}", obs.tool_name, obs.content),
            "category": obs.category,
            "source": state.source,
            "importance": obs.importance,
            "tags": vec!["sidecar".to_string(), obs.tool_name.clone()],
            "session_id": session_id,
            "user_id": state.user_id,
        });

        match post_with_fallback(state, "/store", "/memory/store", &req).await {
            Ok(response) if response.status().is_success() => {
                successful.push(obs);
            }
            Ok(response) => {
                tracing::error!(
                    tool = %obs.tool_name,
                    session_id = %session_id,
                    status = %response.status(),
                    "fallback flush: kleos server rejected observation"
                );
                failed.push(obs);
            }
            Err(_) => {
                tracing::error!(
                    tool = %obs.tool_name,
                    session_id = %session_id,
                    user_id = state.user_id,
                    "fallback flush: failed to send observation"
                );
                failed.push(obs);
            }
        }
    }
    (successful, failed)
}

// ---------------------------------------------------------------------------
// flush_all_sessions -- called from graceful shutdown handler.
// Drains all sessions with pending observations. The 10s deadline is managed
// by the caller via tokio::time::timeout.
// ---------------------------------------------------------------------------

pub async fn flush_all_sessions(state: &SidecarState) {
    let candidates: Vec<String> = {
        let guard = state.sessions.read().await;
        guard
            .list()
            .into_iter()
            .filter(|info| info.pending_count > 0)
            .map(|info| info.id)
            .collect()
    };

    if candidates.is_empty() {
        return;
    }

    tracing::info!(
        count = candidates.len(),
        "graceful shutdown: flushing sessions"
    );

    let tasks: Vec<_> = candidates
        .into_iter()
        .map(|sid| {
            let state = state.clone();
            tokio::spawn(async move {
                let n = flush_pending(&state, &sid).await;
                tracing::info!(session_id = %sid, flushed = n, "graceful flush done");
            })
        })
        .collect();

    for task in tasks {
        let _ = task.await;
    }
}
