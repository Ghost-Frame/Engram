use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_forge::code_context::{ContextPack, ContextQuery, ContextSnippet};
use arc_swap::ArcSwap;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    middleware,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::info;

use crate::auth::require_token;
use crate::metrics;
use crate::session::Observation;
use crate::{CodeContextMode, SidecarState};

/// Apply tiered Kleos auth (PIV/ed25519 signed headers, else bearer) to a request.
pub(crate) fn apply_kleos_auth(
    state: &SidecarState,
    req: reqwest::RequestBuilder,
    method: &str,
    path: &str,
    body: &[u8],
) -> reqwest::RequestBuilder {
    if let Some(signer) = &state.signer {
        if let Some(session) = signer.cached_session() {
            return req.header("X-Kleos-Session", session);
        }
        let (url_path, query) = match path.split_once('?') {
            Some((p, q)) => (p, q),
            None => (path, ""),
        };
        match signer.sign_request(method, url_path, query, body) {
            Ok(signed) => return signed.apply_headers(req),
            Err(e) => tracing::warn!(error = %e, "request signing failed; falling back to bearer"),
        }
    }
    if let Some(ref key) = state.kleos_api_key {
        return req.header("Authorization", format!("Bearer {}", key));
    }
    req
}

/// Returns whether the signer would authenticate with a cached server session.
fn would_use_cached_session(state: &SidecarState) -> bool {
    state
        .signer
        .as_ref()
        .and_then(|signer| signer.cached_session())
        .is_some()
}

/// Clears a cached session when Kleos rejects it so the next request re-signs.
fn clear_stale_session_after_unauthorized(
    signer: &Option<Arc<kleos_lib::auth_piv::RequestSigner>>,
    status: reqwest::StatusCode,
    used_cached_session: bool,
    context: &str,
) -> bool {
    if status != reqwest::StatusCode::UNAUTHORIZED || !used_cached_session {
        return false;
    }

    if let Some(signer) = signer {
        signer.clear_session();
        tracing::warn!(
            context = context,
            "kleos rejected cached session; cleared it and will retry with fresh signature"
        );
        return true;
    }

    false
}

/// Capture a server-issued session token into the signer.
fn capture_kleos_session(state: &SidecarState, resp: &reqwest::Response) {
    if let Some(signer) = &state.signer {
        if let Some(v) = resp.headers().get("x-kleos-session-issued") {
            if let Ok(t) = v.to_str() {
                signer.set_session(t.to_string());
            }
        }
    }
}

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

/// Resolve the canonical root of the nearest Git repository containing `path`.
fn resolve_git_root(path: Option<&str>) -> Option<String> {
    let supplied = path?.trim();
    if supplied.is_empty() {
        return None;
    }
    let candidate = std::fs::canonicalize(supplied).ok()?;
    let start = if candidate.is_file() {
        candidate.parent()?.to_path_buf()
    } else {
        candidate
    };
    start
        .ancestors()
        .find(|ancestor| ancestor.join(".git").exists())
        .map(|root| root.to_string_lossy().to_string())
}

/// Convert hook-provided paths into bounded repository-relative ranking hints.
fn repository_relative_paths(repo_root: Option<&str>, paths: &[String]) -> Vec<String> {
    let root = repo_root.map(std::path::Path::new);
    paths
        .iter()
        .filter_map(|path| {
            let trimmed = path.trim();
            if trimmed.is_empty() {
                return None;
            }
            let supplied = std::path::Path::new(trimmed);
            let relative = root
                .and_then(|root| supplied.strip_prefix(root).ok())
                .unwrap_or(supplied);
            Some(relative.to_string_lossy().replace('\\', "/"))
        })
        .take(64)
        .collect()
}

/// Refresh the local index in a blocking worker without delaying hook responses.
fn spawn_code_refresh(state: &SidecarState, repo_root: Option<String>) {
    if matches!(state.code_context_mode, CodeContextMode::Off) {
        return;
    }
    let (Some(index), Some(repo_root)) = (state.code_index.clone(), repo_root) else {
        return;
    };
    {
        let mut refreshing = state
            .refreshing_repositories
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !refreshing.insert(repo_root.clone()) {
            metrics::inc_code_context("refresh_coalesced", state.code_context_mode.as_str());
            return;
        }
    }
    let mode = state.code_context_mode.as_str();
    let refreshing_repositories = state.refreshing_repositories.clone();
    tokio::task::spawn_blocking(move || {
        let started = Instant::now();
        match index.refresh(&repo_root) {
            Ok(report) => {
                metrics::inc_code_context("refresh_ok", mode);
                metrics::record_code_context_latency(started.elapsed().as_secs_f64());
                tracing::debug!(
                    repo_root = %repo_root,
                    revision = report.index_revision,
                    files_indexed = report.files_indexed,
                    files_removed = report.files_removed,
                    "local code index refreshed"
                );
            }
            Err(error) => {
                metrics::inc_code_context("refresh_error", mode);
                tracing::warn!(%error, repo_root = %repo_root, "local code refresh failed open");
            }
        }
        refreshing_repositories
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&repo_root);
    });
}

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
            .route("/end", post(end_session))
            .route("/session/start", post(start_session))
            .route("/session/{id}/resume", post(resume_session))
            .route("/sessions", get(list_sessions))
            .layer(middleware::from_fn_with_state(state.clone(), require_token))
            .with_state(state),
    )
}

/// Renders the current sidecar Prometheus metrics snapshot.
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

    let search_body = serde_json::to_vec(&search_req).unwrap_or_default();
    let req = apply_kleos_auth(
        &state,
        state.client.post(url).json(&search_req),
        "POST",
        "/memory/search",
        &search_body,
    );

    let stored_count = match req.send().await {
        Ok(resp) if resp.status().is_success() => {
            capture_kleos_session(&state, &resp);
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

/// Reports local sidecar health plus cached upstream Kleos reachability.
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

    Json(json!({
        "status": "ok",
        "upstream_reachable": upstream_reachable,
        "pending_depth": pending_depth,
        "dead_letter_depth": 0,
        "retry_in_flight": false,
        "active_sessions": active_sessions,
        "code_context_mode": state.code_context_mode.as_str(),
        "code_index_available": state.code_index.is_some(),
        "code_max_tokens": state.code_max_tokens,
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
            let req = apply_kleos_auth(state, state.client.head(url), "HEAD", "/health", b"");
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

// --- POST /session/start ---

#[derive(Debug, Deserialize, Default)]
struct StartSessionBody {
    pub session_id: Option<String>,
    /// Current working directory used to discover the active Git repository.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Optional agent identifier to associate with the session.
    #[serde(default)]
    pub agent: Option<String>,
    /// Optional origin label; takes precedence over `agent` if both present.
    #[serde(default)]
    pub origin: Option<String>,
}

/// Starts a tracked sidecar session and records its optional origin label.
async fn start_session(
    State(state): State<SidecarState>,
    Json(body): Json<StartSessionBody>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    let repo_root = resolve_git_root(body.cwd.as_deref());
    let session_id = body
        .session_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    // Resolve origin from body: origin > agent.
    let session_origin = body.origin.or(body.agent);

    let mut sessions = state.sessions.write().await;
    match sessions.start_session(session_id.clone()) {
        Ok(_) => {
            // start_session returns an immutable ref; get mutable ref to set origin.
            let session = sessions
                .get_mut(&session_id)
                .expect("session was just inserted");
            session.origin = session_origin;
            session.record_repository_activity(repo_root.clone(), &[]);
            let sid = session.id.clone();
            let started_at = session.started_at;
            let session_repo_root = session.repo_root.clone();
            spawn_code_refresh(&state, session_repo_root.clone());
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
                    "started_at": started_at,
                    "repo_root": session_repo_root,
                })),
            ))
        }
        Err(e) => Err((
            StatusCode::CONFLICT,
            Json(json!({ "error": e.to_string() })),
        )),
    }
}

// --- GET /sessions ---

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

// --- POST /observe ---

#[derive(Debug, Deserialize)]
struct ObserveBody {
    pub tool_name: Option<String>,
    pub tool: Option<String>,
    pub content: Option<String>,
    pub summary: Option<String>,
    pub role: Option<String>,
    #[serde(default = "default_importance")]
    pub importance: i32,
    #[serde(default = "default_category")]
    pub category: String,
    pub session_id: Option<String>,
    /// Current working directory used to associate the event with a repository.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Files read or changed by the tool event.
    #[serde(default)]
    pub touched_paths: Vec<String>,
    /// Whether the completed tool may have changed repository contents.
    #[serde(default)]
    pub may_modify_repo: bool,
    /// Optional agent identifier to stamp on stored observations.
    #[serde(default)]
    pub agent: Option<String>,
    /// Optional origin label; takes precedence over `agent` if both are set.
    #[serde(default)]
    pub origin: Option<String>,
}

/// Returns the default importance for observations forwarded to Kleos.
fn default_importance() -> i32 {
    3
}

/// Returns the default category for observations forwarded to Kleos.
fn default_category() -> String {
    "discovery".to_string()
}

/// Accepts a tool or conversation observation and queues it for batched storage.
async fn observe(
    State(state): State<SidecarState>,
    Json(body): Json<ObserveBody>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    let supplied_repo_root = resolve_git_root(body.cwd.as_deref());
    let (session_id, repo_root) = {
        let mut sessions = state.sessions.write().await;
        let sid = sessions.resolve_id(body.session_id.as_deref()).to_string();
        let session = sessions.get_or_create(&sid);
        let root = supplied_repo_root
            .clone()
            .or_else(|| session.repo_root.clone());
        let touched_paths = repository_relative_paths(root.as_deref(), &body.touched_paths);
        session.record_repository_activity(root.clone(), &touched_paths);
        (sid, root)
    };
    if body.may_modify_repo {
        spawn_code_refresh(&state, repo_root);
    }

    let tool_name = body
        .tool_name
        .or(body.tool)
        .unwrap_or_else(|| "unknown".to_string());
    let content = body.content.or(body.summary).unwrap_or_default();
    let role = body.role.unwrap_or_else(|| "tool".to_string());

    if (!state.retain_tool_calls && role == "tool")
        || !state.retain_roles.iter().any(|allowed| allowed == &role)
    {
        return Ok((
            StatusCode::ACCEPTED,
            Json(json!({
                "accepted": true,
                "skipped": true,
                "reason": "role filtered",
            })),
        ));
    }

    // Resolve origin: body.origin wins over body.agent; session origin is the fallback.
    let body_origin = body.origin.or(body.agent);

    let mut obs = Observation {
        tool_name,
        content,
        role,
        importance: body.importance,
        category: body.category,
        origin: body_origin,
        timestamp: chrono::Utc::now(),
    };

    let (pending_count, session_id, flush_batch) = {
        let mut sessions = state.sessions.write().await;
        let session = sessions.get_or_create(&session_id);
        // Fall back to the session's origin if no per-observation origin was given.
        if obs.origin.is_none() {
            obs.origin = session.origin.clone();
        }

        if session.ended {
            return Err((
                StatusCode::GONE,
                Json(json!({
                    "error": "session has ended",
                    "session_id": session_id,
                })),
            ));
        }

        // Hard cap -- loud 503 rather than unbounded queue growth when upstream is down.
        if session.pending.len() >= state.max_pending_per_session {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "pending queue full -- upstream may be down",
                    "session_id": session_id,
                    "pending": session.pending.len(),
                    "limit": state.max_pending_per_session,
                })),
            ));
        }

        let count = session.add_observation(obs);
        let mut flush_batch = Vec::new();
        if session.turn_count % state.retain_every_n == 0 || count >= state.batch_size {
            flush_batch = session.drain_with_overlap(state.overlap_turns);
        }
        (session.pending.len(), session_id, flush_batch)
    };

    metrics::inc_observations(1);

    let flushed = flush_batch.len();
    if !flush_batch.is_empty() {
        let flush_state = state.clone();
        let flush_session_id = session_id.clone();
        tokio::spawn(async move {
            let _ = flush_observations(&flush_state, &flush_session_id, flush_batch).await;
        });
    }

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

// --- POST /recall ---

#[derive(Debug, Deserialize)]
struct RecallBody {
    pub query: Option<String>,
    pub message: Option<String>,
    #[serde(default = "default_recall_limit")]
    pub limit: usize,
    pub budget: Option<String>,
    pub context_turns: Option<usize>,
    pub max_tokens: Option<usize>,
    pub max_query_chars: Option<usize>,
    pub session_id: Option<String>,
    /// Current working directory used to discover the active repository.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Paths relevant to the current prompt.
    #[serde(default)]
    pub touched_paths: Vec<String>,
    /// Whether work immediately before recall may have changed repository files.
    #[serde(default)]
    pub may_modify_repo: bool,
    /// Optional per-request override for the code-only token allowance.
    pub code_max_tokens: Option<usize>,
}

/// Returns the default maximum number of memories to recall.
fn default_recall_limit() -> usize {
    10
}

/// Returns the default number of recent observations that should shape recall.
fn default_context_turns() -> usize {
    1
}

/// Returns the default token cap for injected recall context.
fn default_recall_max_tokens() -> usize {
    1024
}

/// Returns the default character cap applied to the search query sent upstream.
fn default_recall_max_query_chars() -> usize {
    800
}

/// Truncates text by character count without splitting UTF-8 code units.
fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(text.len().min(max_chars));
    for ch in text.chars().take(max_chars) {
        out.push(ch);
    }
    out
}

/// Formats the newest observations into recall query context.
fn format_recent_context(observations: &[Observation]) -> String {
    observations
        .iter()
        .map(|obs| {
            format!(
                "[{}:{}] {}",
                obs.role,
                obs.tool_name,
                truncate_chars(&obs.content, 180)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Build a stable identity for one snippet version and exact source range.
fn code_snippet_cache_key(snippet: &ContextSnippet) -> String {
    format!(
        "{}:{}:{}:{}",
        snippet.content_hash, snippet.path, snippet.start_line, snippet.end_line
    )
}

/// Estimate one rendered snippet's token cost using the indexer's conservative rule.
fn code_snippet_tokens(snippet: &ContextSnippet) -> usize {
    snippet.text.chars().count().div_ceil(4).saturating_add(24)
}

/// Remove unchanged repeat snippets while preserving exact-symbol requests.
fn suppress_unchanged_code(pack: &mut ContextPack, previous: &HashSet<String>) -> HashSet<String> {
    let selected = pack
        .snippets
        .iter()
        .map(code_snippet_cache_key)
        .collect::<HashSet<_>>();
    pack.snippets.retain(|snippet| {
        snippet
            .reason
            .split(", ")
            .any(|reason| reason == "exact symbol")
            || !previous.contains(&code_snippet_cache_key(snippet))
    });
    pack.estimated_tokens = pack.snippets.iter().map(code_snippet_tokens).sum();
    selected
}

/// Default minimum cosine similarity a memory must clear to be injected into a prompt.
fn default_recall_min_semantic() -> f64 {
    0.55
}

/// Default comma-separated categories excluded from per-prompt recall injection.
fn default_recall_excluded_categories() -> &'static str {
    "general,state"
}

/// Reads the semantic-relevance floor for recall injection from the environment.
fn recall_min_semantic() -> f64 {
    std::env::var("KLEOS_RECALL_MIN_SEMANTIC")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(default_recall_min_semantic)
}

/// Reads the lowercased set of categories excluded from recall injection.
fn recall_excluded_categories() -> std::collections::HashSet<String> {
    std::env::var("KLEOS_RECALL_EXCLUDE_CATEGORIES")
        .unwrap_or_else(|_| default_recall_excluded_categories().to_string())
        .split(',')
        .map(|c| c.trim().to_ascii_lowercase())
        .filter(|c| !c.is_empty())
        .collect()
}

/// Returns true for curated sources that are exempt from the category denylist.
///
/// The plans ingest pipeline labels memories `plan:<relpath>`, but the server's
/// auto-categorizer overrides the explicit `--category plan` and relabels the
/// chunks general/reference. Without this exemption the recall denylist would
/// drop the very plan documents the ingest exists to surface. The semantic
/// floor still applies, so off-topic plans are not injected.
fn is_curated_source(source: &str) -> bool {
    source.starts_with("plan:")
}

/// Decides whether a single search result is relevant enough to inject.
///
/// Gates on the raw cosine `semantic_score` rather than the compound `score`,
/// because the compound value is inflated by recency / decay / personality
/// boosts -- so a "hot" but off-topic memory (Discord banter, personal facts)
/// scores high on `score` while its cosine to the prompt is near zero. Results
/// with no `semantic_score` are kept only when they are an exact lexical hit
/// (`fts_score` present), which is itself a strong relevance signal. Curated
/// plan chunks still require semantic evidence because broad evidence files can
/// contain exact conversational phrases. Purely graph- or boost-derived
/// candidates are dropped.
fn recall_result_is_relevant(result: &Value, min_semantic: f64) -> bool {
    match result.get("semantic_score").and_then(|v| v.as_f64()) {
        Some(sem) => sem >= min_semantic,
        None => {
            let source = result.get("source").and_then(|v| v.as_str()).unwrap_or("");
            !is_curated_source(source) && result.get("fts_score").and_then(|v| v.as_f64()).is_some()
        }
    }
}

/// Formats returned memories into Claude additionalContext text under a char budget.
fn format_recall_context(results: &[Value], max_chars: usize) -> String {
    let mut lines = Vec::new();
    let mut used = 0usize;

    // Relevance policy read once per call; both knobs are env-tunable so retuning
    // never requires a rebuild of the sidecar.
    let min_semantic = recall_min_semantic();
    let excluded = recall_excluded_categories();

    for result in results {
        let Some(content) = result.get("content").and_then(|v| v.as_str()) else {
            continue;
        };
        let category = result
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("general");
        let source = result.get("source").and_then(|v| v.as_str()).unwrap_or("");
        // Curated plan documents ingest as `plan:<path>` but the server's
        // auto-categorizer relabels them general/reference, so the category
        // denylist would silently drop them. Exempt plan sources from category
        // exclusion -- they still must clear the semantic floor below.
        if !is_curated_source(source) && excluded.contains(&category.to_ascii_lowercase()) {
            continue;
        }
        // Drop memories that are not semantically about the prompt.
        if !recall_result_is_relevant(result, min_semantic) {
            continue;
        }
        let line = format!("[{}] {}", category, truncate_chars(content, 220));
        let extra = if lines.is_empty() {
            line.len()
        } else {
            line.len() + 1
        };
        if used + extra > max_chars {
            break;
        }
        used += extra;
        lines.push(line);
    }

    if lines.is_empty() {
        String::new()
    } else {
        format!("Relevant memories:\n{}", lines.join("\n"))
    }
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
    let body_bytes = serde_json::to_vec(body).unwrap_or_default();
    let req = apply_kleos_auth(
        state,
        state.client.post(url.clone()).json(body),
        "POST",
        primary,
        &body_bytes,
    );
    let used_cached_session = would_use_cached_session(state);

    let mut response = req.send().await.map_err(|e| {
        tracing::error!(error = %e, "kleos server request failed");
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": "kleos server unreachable" })),
        )
    })?;
    if clear_stale_session_after_unauthorized(
        &state.signer,
        response.status(),
        used_cached_session,
        primary,
    ) {
        let retry_req = apply_kleos_auth(
            state,
            state.client.post(url.clone()).json(body),
            "POST",
            primary,
            &body_bytes,
        );
        response = retry_req.send().await.map_err(|e| {
            tracing::error!(error = %e, "kleos server retry failed");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "kleos server unreachable" })),
            )
        })?;
    }
    capture_kleos_session(state, &response);

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
        let req = apply_kleos_auth(
            state,
            state.client.post(url.clone()).json(body),
            "POST",
            fallback,
            &body_bytes,
        );
        let used_cached_session = would_use_cached_session(state);
        let mut fb_resp = req.send().await.map_err(|e| {
            tracing::error!(error = %e, "kleos server fallback request failed");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "kleos server unreachable" })),
            )
        })?;
        if clear_stale_session_after_unauthorized(
            &state.signer,
            fb_resp.status(),
            used_cached_session,
            fallback,
        ) {
            let retry_req = apply_kleos_auth(
                state,
                state.client.post(url.clone()).json(body),
                "POST",
                fallback,
                &body_bytes,
            );
            fb_resp = retry_req.send().await.map_err(|e| {
                tracing::error!(error = %e, "kleos server fallback retry failed");
                (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({ "error": "kleos server unreachable" })),
                )
            })?;
        }
        capture_kleos_session(state, &fb_resp);
        return Ok(fb_resp);
    }

    Ok(response)
}

/// Recall memories and deterministic code context with independent fail-open behavior.
async fn recall(
    State(state): State<SidecarState>,
    Json(body): Json<RecallBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let query = body.query.or(body.message).unwrap_or_default();
    let code_mode = state.code_context_mode;

    if query.is_empty() {
        return Ok(Json(json!({
            "results": [],
            "count": 0,
            "context": "",
            "code_context": "",
            "code_pack": ContextPack::default(),
            "code_mode": code_mode.as_str(),
        })));
    }

    let supplied_repo_root = resolve_git_root(body.cwd.as_deref());
    let (session_id, repo_root, recent_paths, previous_code_hashes) = {
        let mut sessions = state.sessions.write().await;
        let sid = sessions.resolve_id(body.session_id.as_deref()).to_string();
        let session = sessions.get_or_create(&sid);
        let root = supplied_repo_root
            .clone()
            .or_else(|| session.repo_root.clone());
        let touched_paths = repository_relative_paths(root.as_deref(), &body.touched_paths);
        session.record_repository_activity(root.clone(), &touched_paths);
        (
            sid,
            root,
            session.recent_paths.clone(),
            session.last_injected_hashes.clone(),
        )
    };
    let context_turns = body.context_turns.unwrap_or_else(default_context_turns);
    let max_tokens = body.max_tokens.unwrap_or_else(default_recall_max_tokens);
    let max_query_chars = body
        .max_query_chars
        .unwrap_or_else(default_recall_max_query_chars);

    let recent_context = {
        let sessions = state.sessions.read().await;
        sessions
            .get(&session_id)
            .map(|session| session.recent_observations(context_turns))
            .unwrap_or_default()
    };
    let combined_query = if recent_context.is_empty() {
        truncate_chars(&query, max_query_chars)
    } else {
        truncate_chars(
            &format!("{}\n\n{}", query, format_recent_context(&recent_context)),
            max_query_chars,
        )
    };

    if body.may_modify_repo {
        spawn_code_refresh(&state, repo_root.clone());
    }

    let focus_paths = repository_relative_paths(repo_root.as_deref(), &body.touched_paths);
    let code_task = match (code_mode, state.code_index.clone(), repo_root.clone()) {
        (CodeContextMode::Off, _, _) => None,
        (_, Some(index), Some(repo_root)) => {
            let code_query = ContextQuery {
                repo_root,
                query: query.clone(),
                max_tokens: body.code_max_tokens.unwrap_or(state.code_max_tokens).max(1),
                focus_paths,
                recent_paths,
            };
            Some(tokio::task::spawn_blocking(move || {
                let started = Instant::now();
                let result = index.context_from_index(&code_query);
                (result, started.elapsed().as_secs_f64())
            }))
        }
        (_, None, _) => {
            metrics::inc_code_context("index_unavailable", code_mode.as_str());
            None
        }
        (_, _, None) => {
            metrics::inc_code_context("no_repository", code_mode.as_str());
            None
        }
    };

    let search_req = json!({
        "query": combined_query,
        "limit": body.limit.min(100),
        "include_forgotten": false,
        "latest_only": true,
        "budget": body.budget,
    });

    let (results_arr, memory_error) = match post_with_fallback(
        &state,
        "/search",
        "/memory/search",
        &search_req,
    )
    .await
    {
        Ok(response) if response.status().is_success() => match response.json::<Value>().await {
            Ok(results) => (
                results
                    .get("results")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
                None,
            ),
            Err(error) => {
                tracing::warn!(%error, "memory recall response was invalid; failing open");
                (Vec::new(), Some("invalid response from Kleos".to_string()))
            }
        },
        Ok(response) => {
            let status = response.status();
            tracing::warn!(%status, "memory recall failed; local code retrieval continues");
            (Vec::new(), Some(format!("Kleos returned {status}")))
        }
        Err((status, Json(error))) => {
            tracing::warn!(%status, "memory recall unavailable; local code retrieval continues");
            let detail = error
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("Kleos request failed");
            (Vec::new(), Some(detail.to_string()))
        }
    };

    let memory_context = format_recall_context(&results_arr, max_tokens.saturating_mul(4));
    let mut code_pack = ContextPack::default();
    let mut selected_code_hashes = HashSet::new();
    let mut code_error = None;
    if let Some(task) = code_task {
        match task.await {
            Ok((Ok(mut pack), elapsed)) => {
                if matches!(code_mode, CodeContextMode::Inject) {
                    selected_code_hashes =
                        suppress_unchanged_code(&mut pack, &previous_code_hashes);
                }
                metrics::record_code_context_latency(elapsed);
                metrics::record_code_context_snippets(pack.snippets.len() as f64);
                metrics::record_code_context_tokens(pack.estimated_tokens as f64);
                let outcome = if pack.snippets.is_empty() {
                    "abstained"
                } else {
                    "selected"
                };
                metrics::inc_code_context(outcome, code_mode.as_str());
                code_pack = pack;
            }
            Ok((Err(error), elapsed)) => {
                metrics::record_code_context_latency(elapsed);
                metrics::inc_code_context("query_error", code_mode.as_str());
                tracing::warn!(%error, "local code retrieval failed open");
                code_error = Some(error.to_string());
            }
            Err(error) => {
                metrics::inc_code_context("worker_error", code_mode.as_str());
                tracing::warn!(%error, "local code retrieval worker failed open");
                code_error = Some(error.to_string());
            }
        }
    }

    let rendered_code = code_pack.render();
    let code_context = if rendered_code.is_empty() {
        String::new()
    } else {
        format!("Relevant code:\n{rendered_code}")
    };
    let context = match (
        memory_context.is_empty(),
        code_context.is_empty() || !matches!(code_mode, CodeContextMode::Inject),
    ) {
        (false, false) => format!("{memory_context}\n\n{code_context}"),
        (false, true) => memory_context,
        (true, false) => code_context.clone(),
        (true, true) => String::new(),
    };

    if matches!(code_mode, CodeContextMode::Inject) {
        let mut sessions = state.sessions.write().await;
        if let Some(session) = sessions.get_mut(&session_id) {
            session.last_injected_hashes = selected_code_hashes;
        }
    }

    let count = results_arr.len();

    Ok(Json(json!({
        "results": results_arr,
        "count": count,
        "context": context,
        "session_id": session_id,
        "memory_error": memory_error,
        "code_context": code_context,
        "code_pack": code_pack,
        "code_mode": code_mode.as_str(),
        "code_error": code_error,
    })))
}

// --- POST /end ---

#[derive(Debug, Deserialize)]
struct EndSessionBody {
    pub session_id: Option<String>,
}

/// Flushes pending observations and marks the selected session ended.
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
    flush_observations(state, session_id, observations).await
}

/// Flushes an already selected observation batch to Kleos and requeues failures.
async fn flush_observations(
    state: &SidecarState,
    session_id: &str,
    observations: Vec<Observation>,
) -> usize {
    if observations.is_empty() {
        return 0;
    }

    let t0 = Instant::now();

    let ops: Vec<Value> = observations
        .iter()
        .map(|obs| {
            let mut tags = vec!["sidecar".to_string(), obs.tool_name.clone()];
            if let Some(o) = &obs.origin {
                tags.push(format!("origin:{}", o));
            }
            json!({
                "op": "store",
                "body": {
                    "content": format!("[{}] {}", obs.tool_name, obs.content),
                    "category": obs.category,
                    "source": state.source,
                    "importance": obs.importance,
                    "tags": tags,
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
    let signer = state.signer.clone();
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
        let signer = signer.clone();
        let client = client.clone();
        let sid = sid.clone();
        async move {
            // Apply tiered auth inline (mirrors apply_kleos_auth but uses
            // captured clones rather than a &SidecarState reference, which
            // cannot cross the async closure boundary).
            let batch_body = serde_json::to_vec(&batch_req).unwrap_or_default();
            let mut req = client.post(url.clone()).json(&batch_req);
            let used_cached_session = signer
                .as_ref()
                .and_then(|s| s.cached_session())
                .is_some();
            if let Some(ref s) = signer {
                if let Some(session_tok) = s.cached_session() {
                    req = req.header("X-Kleos-Session", session_tok);
                } else {
                    match s.sign_request("POST", "/batch", "", &batch_body) {
                        Ok(signed) => req = signed.apply_headers(req),
                        Err(e) => {
                            tracing::warn!(error = %e, "batch flush: signing failed; using bearer");
                            if let Some(ref key) = api_key {
                                req = req.header("Authorization", format!("Bearer {}", key));
                            }
                        }
                    }
                }
            } else if let Some(ref key) = api_key {
                req = req.header("Authorization", format!("Bearer {}", key));
            }

            let mut response = req.send().await.map_err(|e| {
                tracing::warn!(session_id = %sid, error = %e, "batch flush: kleos unreachable");
                e.to_string()
            })?;
            if clear_stale_session_after_unauthorized(
                &signer,
                response.status(),
                used_cached_session,
                "/batch",
            ) {
                let mut retry_req = client.post(url.clone()).json(&batch_req);
                if let Some(ref s) = signer {
                    match s.sign_request("POST", "/batch", "", &batch_body) {
                        Ok(signed) => retry_req = signed.apply_headers(retry_req),
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "batch flush: retry signing failed; using bearer"
                            );
                            if let Some(ref key) = api_key {
                                retry_req =
                                    retry_req.header("Authorization", format!("Bearer {}", key));
                            }
                        }
                    }
                } else if let Some(ref key) = api_key {
                    retry_req = retry_req.header("Authorization", format!("Bearer {}", key));
                }
                response = retry_req.send().await.map_err(|e| {
                    tracing::warn!(session_id = %sid, error = %e, "batch flush: kleos retry unreachable");
                    e.to_string()
                })?;
            }

            // Capture any session token issued by the server.
            if let Some(ref s) = signer {
                if let Some(v) = response.headers().get("x-kleos-session-issued") {
                    if let Ok(t) = v.to_str() {
                        s.set_session(t.to_string());
                    }
                }
            }

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
        let mut tags = vec!["sidecar".to_string(), obs.tool_name.clone()];
        if let Some(o) = &obs.origin {
            tags.push(format!("origin:{}", o));
        }
        let req = json!({
            "content": format!("[{}] {}", obs.tool_name, obs.content),
            "category": obs.category,
            "source": state.source,
            "importance": obs.importance,
            "tags": tags,
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

/// Regression tests for per-prompt recall relevance filtering.
#[cfg(test)]
/// Unit tests for memory recall filtering policy.
mod recall_gate_tests {
    use super::*;
    use serde_json::json;

    /// A high-cosine on-topic memory passes the default semantic floor.
    #[test]
    fn keeps_semantically_relevant() {
        let r = json!({"content": "zellij layout KDL", "category": "technical", "semantic_score": 0.71});
        assert!(recall_result_is_relevant(&r, default_recall_min_semantic()));
    }

    /// A boosted-but-off-topic memory (high compound score, low cosine) is dropped.
    #[test]
    fn drops_boosted_offtopic() {
        let r = json!({"content": "demoted lol", "category": "general", "score": 1.5, "semantic_score": 0.12});
        assert!(!recall_result_is_relevant(
            &r,
            default_recall_min_semantic()
        ));
    }

    /// An exact lexical hit with no embedding is kept as a strong signal.
    #[test]
    fn keeps_exact_lexical_when_no_embedding() {
        let r = json!({"content": "exact term", "category": "technical", "fts_score": 4.2});
        assert!(recall_result_is_relevant(&r, default_recall_min_semantic()));
    }

    /// A lexical-only plan chunk is not strong enough to inject without semantic evidence.
    #[test]
    fn drops_plan_lexical_when_no_embedding() {
        let r = json!({
            "content": "it worked before",
            "category": "general",
            "fts_score": 20.0,
            "source": "plan:website/live-blog.txt"
        });
        assert!(!recall_result_is_relevant(
            &r,
            default_recall_min_semantic()
        ));
    }

    /// A graph/boost-only candidate with no semantic or lexical signal is dropped.
    #[test]
    fn drops_signalless_candidate() {
        let r = json!({"content": "tangential", "category": "technical", "score": 0.9});
        assert!(!recall_result_is_relevant(
            &r,
            default_recall_min_semantic()
        ));
    }

    /// The full formatter excludes noise categories and weak matches together.
    #[test]
    fn formatter_filters_noise_and_weak() {
        let results = vec![
            json!({"content": "Zan likes spicy food", "category": "state", "semantic_score": 0.9}),
            json!({"content": "They finally demoted me lol", "category": "general", "semantic_score": 0.8}),
            json!({"content": "weak match", "category": "technical", "semantic_score": 0.30}),
            json!({"content": "zellij uses WASM plugins", "category": "technical", "semantic_score": 0.66}),
        ];
        let out = format_recall_context(&results, 10_000);
        assert!(out.contains("zellij uses WASM plugins"));
        assert!(!out.contains("spicy food"));
        assert!(!out.contains("demoted"));
        assert!(!out.contains("weak match"));
    }

    /// With every result filtered out, the formatter returns an empty string
    /// (no "Relevant memories:" header injected for an off-topic prompt).
    #[test]
    fn formatter_empty_when_all_filtered() {
        let results = vec![
            json!({"content": "banter", "category": "general", "semantic_score": 0.9}),
            json!({"content": "tangent", "category": "technical", "semantic_score": 0.10}),
        ];
        assert!(format_recall_context(&results, 10_000).is_empty());
    }

    /// A curated plan source survives the category denylist (it ingests as
    /// plan:<path> but is auto-relabeled general), while a non-plan general
    /// memory and a sub-floor plan are both still dropped.
    #[test]
    fn formatter_exempts_plan_sources_from_category_denylist() {
        let results = vec![
            json!({"content": "BAV documents feature design", "category": "general", "semantic_score": 0.78, "source": "plan:bav-assistant/design.md"}),
            json!({"content": "off-topic banter", "category": "general", "semantic_score": 0.80, "source": "discord"}),
            json!({"content": "stale plan chunk", "category": "general", "semantic_score": 0.20, "source": "plan:old/thing.md"}),
        ];
        let out = format_recall_context(&results, 10_000);
        assert!(out.contains("BAV documents feature design"));
        assert!(!out.contains("off-topic banter"));
        assert!(!out.contains("stale plan chunk"));
    }
}
