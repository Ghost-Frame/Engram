use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::time::Duration;
use tower_http::timeout::TimeoutLayer;

use crate::error::AppError;
use crate::extractors::{Auth, ResolvedDb};
use crate::state::AppState;
use kleos_lib::auth::Scope;
use kleos_lib::llm::{CallOptions, Priority};
use kleos_lib::skills::{
    self, aliases as skill_aliases, analyzer, bundles as skill_bundles, cloud, dashboard, evolver,
    materializations as skill_materializations,
    search::{find_skills, search_skills, FindOpts},
    CreateSkillRequest, UpdateSkillRequest,
};
use kleos_lib::validation::MAX_PAGINATION_OFFSET;

mod types;
use types::{
    AddAliasBody, AddBundleMemberBody, CaptureBody, CloudSearchBody, CloudUploadBody,
    CreateBundleBody, DeriveBody, EvolutionRecentParams, ExecuteSkillsBody, FindSkillsBody,
    GetExecutionsParams, JudgeBody, ListSkillsParams, RecordExecutionBody,
    RecordMaterializationBody, RecordToolQualityBody, ResolveAliasBody, SearchSkillsBody,
    StatsParams, SyncSkillsBody, UploadSkillBody,
};

/// Clamp a caller-supplied `limit` into [1, max] with a default when absent.
fn clamp_limit(raw: Option<usize>, default: usize, max: usize) -> Result<usize, AppError> {
    match raw {
        None => Ok(default.min(max).max(1)),
        Some(0) => Err(AppError::from(kleos_lib::EngError::InvalidInput(
            "limit must be >= 1".into(),
        ))),
        Some(n) => Ok(n.min(max)),
    }
}

/// Clamp a caller-supplied `offset`. No upper bound needed; just rejects absurd values.
fn clamp_offset(raw: Option<usize>) -> Result<usize, AppError> {
    match raw {
        None => Ok(0),
        Some(n) if n > MAX_PAGINATION_OFFSET => {
            Err(AppError::from(kleos_lib::EngError::InvalidInput(format!(
                "offset must be <= {}",
                MAX_PAGINATION_OFFSET
            ))))
        }
        Some(n) => Ok(n),
    }
}

/// Read the skill-sync allowlist from env. Empty means sync is disabled.
/// Each entry is canonicalized once at check time.
fn skill_sync_allowlist() -> Vec<std::path::PathBuf> {
    std::env::var("ENGRAM_SKILL_SYNC_PATHS")
        .ok()
        .map(|raw| {
            raw.split(':')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .filter_map(|s| std::fs::canonicalize(s).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// SECURITY: prevents an authenticated caller from pointing /skills/sync at
/// arbitrary directories on disk. Canonicalizes `dir` and checks it is a
/// descendant of (or equal to) any allowlisted root.
fn is_path_allowed(dir: &str, allowlist: &[std::path::PathBuf]) -> bool {
    let canon = match std::fs::canonicalize(dir) {
        Ok(p) => p,
        Err(_) => return false,
    };
    allowlist.iter().any(|root| canon.starts_with(root))
}

// --- Router ---

/// Register all `/skills`, `/tools`, and `/bundles` routes onto a shared Axum router.
pub fn router() -> Router<AppState> {
    Router::new()
        // CRUD
        .route(
            "/skills",
            post(create_skill_handler).get(list_skills_handler),
        )
        .route("/skills/search", post(search_skills_handler))
        .route("/skills/sync", post(sync_skills_handler))
        .route("/skills/upload", post(upload_skill_handler))
        .route(
            "/skills/{id}",
            get(get_skill_handler).delete(delete_skill_handler),
        )
        .route("/skills/{id}/update", post(update_skill_handler))
        .route("/skills/{id}/recompute", post(recompute_skill_handler))
        // Execution
        .route("/skills/{id}/execute", post(record_execution_handler))
        .route("/skills/{id}/executions", get(get_executions_handler))
        // Judgments
        .route("/skills/{id}/judge", post(judge_handler))
        .route("/skills/{id}/judgments", get(get_judgments_handler))
        // Tags, deps, lineage
        .route("/skills/{id}/tags", get(get_tags_handler))
        .route("/skills/{id}/deps", get(get_deps_handler))
        .route("/skills/{id}/lineage", get(get_lineage_handler))
        // Tool quality
        .route("/tools/quality", post(record_tool_quality_handler))
        .route("/tools/quality/{tool_name}", get(get_tool_quality_handler))
        // Dashboard
        .route("/skills/dashboard/health", get(health_handler))
        .route("/skills/dashboard/overview", get(overview_handler))
        .route("/skills/dashboard/stats", get(stats_handler))
        .route("/skills/{id}/detail", get(detail_handler))
        // Evolution (read-only)
        .route("/skills/evolution/recent", get(evolution_recent_handler))
        // Interactive skill execution (single LLM call, bounded lane).
        .merge(execute_route())
        // Evolution (LLM-backed, needs a longer timeout than the global default).
        .merge(llm_routes())
        // Analyzer
        .route("/skills/usage-stats", get(usage_stats_handler))
        // Cloud
        .route("/skills/cloud/search", post(cloud_search_handler))
        .route("/skills/cloud/upload", post(cloud_upload_handler))
        // Skills Cloud (v50+): hybrid find, aliases, bundles, materializations
        .route("/skills/find", post(find_skills_handler))
        .route("/skills/aliases/resolve", post(resolve_alias_handler))
        .route(
            "/skills/{id}/aliases",
            get(list_aliases_handler).post(add_alias_handler),
        )
        .route(
            "/skills/{id}/aliases/{alias}",
            axum::routing::delete(remove_alias_handler),
        )
        .route(
            "/skills/{id}/materialization",
            get(get_materialization_handler).delete(forget_materialization_handler),
        )
        .route(
            "/skills/{id}/materialize",
            post(record_materialization_handler),
        )
        .route(
            "/bundles",
            get(list_bundles_handler).post(create_bundle_handler),
        )
        .route(
            "/bundles/{id}",
            get(get_bundle_handler).delete(delete_bundle_handler),
        )
        .route(
            "/bundles/{id}/skills",
            get(list_bundle_members_handler).post(add_bundle_member_handler),
        )
        .route(
            "/bundles/{id}/skills/{skill_id}",
            axum::routing::delete(remove_bundle_member_handler),
        )
}

/// Per-call LLM timeout budget in milliseconds, from `OLLAMA_TIMEOUT_BG_MS`
/// (default 60s). Shared by every LLM-backed skill route so one env var governs
/// the whole family of timeouts.
fn per_call_timeout_ms() -> u64 {
    std::env::var("OLLAMA_TIMEOUT_BG_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60_000)
}

/// Builds the sub-router for LLM-backed evolution routes with a per-request timeout.
/// Some endpoints (fix, derive) make multiple sequential LLM calls, so the
/// route timeout must exceed `per_call_timeout * max_calls`. Fix makes 3 calls.
fn llm_routes() -> Router<AppState> {
    let route_timeout_ms = per_call_timeout_ms().saturating_mul(4);
    Router::new()
        .route("/skills/evolve", post(evolve_handler))
        .route("/skills/{id}/fix", post(fix_handler))
        .route("/skills/derive", post(derive_handler))
        .route("/skills/capture", post(capture_handler))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_millis(route_timeout_ms),
        ))
}

/// Builds the bounded sub-router for the interactive `/skills/execute` endpoint.
///
/// `execute` makes a single LLM completion, but with `Priority::Background` it can
/// park on the concurrency-limited Ollama semaphore with no per-wait deadline
/// (`kleos_lib::llm::local`), leaving the global 30-minute request timeout
/// (`server.rs`) as the only backstop -- i.e. a silent multi-minute hang under
/// load. Sizing this route for one call plus a short queue grace turns LLM
/// saturation into a prompt 408 the caller can record and fall back on.
fn execute_route() -> Router<AppState> {
    let route_timeout_ms = per_call_timeout_ms().saturating_mul(2);
    Router::new()
        .route("/skills/execute", post(execute_skills_handler))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_millis(route_timeout_ms),
        ))
}

// --- CRUD handlers ---

/// Create a new skill record owned by the authenticated user and return it with 201.
#[tracing::instrument(skip_all)]
async fn create_skill_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(mut req): Json<CreateSkillRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    req.user_id = Some(auth.effective_user_id());
    let skill = skills::create_skill(&db, req).await?;
    Ok((StatusCode::CREATED, Json(json!(skill))))
}

/// List skills owned by the authenticated user with optional agent filter and pagination.
#[tracing::instrument(skip_all)]
async fn list_skills_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<ListSkillsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = clamp_limit(params.limit, 50, 1000)?;
    let offset = clamp_offset(params.offset)?;
    let skill_list = skills::list_skills(
        &db,
        auth.effective_user_id(),
        params.agent.as_deref(),
        limit,
        offset,
    )
    .await?;
    Ok(Json(
        json!({ "skills": skill_list, "count": skill_list.len() }),
    ))
}

/// Full-text and semantic search over the skill registry for the authenticated user.
#[tracing::instrument(skip_all)]
async fn search_skills_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<SearchSkillsBody>,
) -> Result<Json<Value>, AppError> {
    let limit = clamp_limit(body.limit, 20, 1000)?;
    let results = search_skills(&db, &body.query, auth.effective_user_id(), limit).await?;
    Ok(Json(json!({ "results": results, "count": results.len() })))
}

/// Fetch a single skill by ID, scoped to the authenticated user.
#[tracing::instrument(skip_all)]
async fn get_skill_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let skill = skills::get_skill(&db, id, auth.effective_user_id()).await?;
    Ok(Json(json!(skill)))
}

/// Permanently delete a skill owned by the authenticated user.
#[tracing::instrument(skip_all)]
async fn delete_skill_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    skills::delete_skill(&db, id, auth.effective_user_id()).await?;
    Ok(Json(json!({ "deleted": true, "id": id })))
}

/// Apply a partial update to an existing skill owned by the authenticated user.
#[tracing::instrument(skip_all)]
async fn update_skill_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
    Json(req): Json<UpdateSkillRequest>,
) -> Result<Json<Value>, AppError> {
    let skill = skills::update_skill(&db, id, req, auth.effective_user_id()).await?;
    Ok(Json(json!(skill)))
}

/// Recompute derived fields (embeddings, trust scores) for a skill by ID.
#[tracing::instrument(skip_all)]
async fn recompute_skill_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let skill = skills::recompute_skill(&db, id, auth.effective_user_id()).await?;
    Ok(Json(json!({
        "recomputed": true,
        "skill": skill,
    })))
}

// --- Execution handlers ---

#[tracing::instrument(skip_all)]
async fn record_execution_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
    Json(body): Json<RecordExecutionBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    skills::record_execution(
        &db,
        id,
        auth.effective_user_id(),
        body.success,
        body.duration_ms,
        body.error_type.as_deref(),
        body.error_message.as_deref(),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "recorded": true, "skill_id": id })),
    ))
}

/// List recent execution records for a skill, scoped to the authenticated user.
#[tracing::instrument(skip_all)]
async fn get_executions_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
    Query(params): Query<GetExecutionsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = clamp_limit(params.limit, 20, 1000)?;
    let executions = skills::get_executions(&db, id, auth.effective_user_id(), limit).await?;
    Ok(Json(
        json!({ "executions": executions, "count": executions.len() }),
    ))
}

// --- Judgment handlers ---

#[tracing::instrument(skip_all)]
async fn judge_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
    Json(body): Json<JudgeBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let judgment = skills::add_judgment(
        &db,
        id,
        auth.effective_user_id(),
        &body.judge_agent,
        body.score,
        body.rationale.as_deref(),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(json!(judgment))))
}

/// List all judge scores and rationales recorded for a skill.
#[tracing::instrument(skip_all)]
async fn get_judgments_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let judgments = skills::get_judgments(&db, id, auth.effective_user_id()).await?;
    Ok(Json(
        json!({ "judgments": judgments, "count": judgments.len() }),
    ))
}

// --- Tags, deps, lineage handlers ---

#[tracing::instrument(skip_all)]
async fn get_tags_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let tags = skills::get_skill_tags(&db, id, auth.effective_user_id()).await?;
    Ok(Json(json!({ "tags": tags })))
}

/// Return the tool dependency list declared for a skill.
#[tracing::instrument(skip_all)]
async fn get_deps_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let deps = skills::get_tool_deps(&db, id, auth.effective_user_id()).await?;
    Ok(Json(json!({ "deps": deps })))
}

/// Return the parent/child lineage chain for a skill.
#[tracing::instrument(skip_all)]
async fn get_lineage_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let lineage = skills::get_lineage(&db, id, auth.effective_user_id()).await?;
    Ok(Json(json!({ "lineage": lineage })))
}

// --- Tool quality handlers ---

// SECURITY: relies on ResolvedDb shard isolation (Phase 5+) to scope to the caller's tenant. Do not add state.db calls here without re-binding auth.
#[tracing::instrument(skip_all)]
async fn record_tool_quality_handler(
    Auth(_auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<RecordToolQualityBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    skills::record_tool_quality(
        &db,
        &body.tool_name,
        &body.agent,
        body.success,
        body.latency_ms,
        body.error_type.as_deref(),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "recorded": true, "tool_name": body.tool_name })),
    ))
}

// SECURITY: relies on ResolvedDb shard isolation (Phase 5+) to scope to the caller's tenant. Do not add state.db calls here without re-binding auth.
#[tracing::instrument(skip_all)]
async fn get_tool_quality_handler(
    Auth(_auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(tool_name): Path<String>,
) -> Result<Json<Value>, AppError> {
    let quality = skills::get_tool_quality(&db, &tool_name).await?;
    Ok(Json(json!(quality)))
}

// --- Dashboard handlers ---

// SECURITY: relies on ResolvedDb shard isolation (Phase 5+) to scope to the caller's tenant. Do not add state.db calls here without re-binding auth.
#[tracing::instrument(skip_all)]
async fn health_handler(
    Auth(_auth): Auth,
    ResolvedDb(db): ResolvedDb,
) -> Result<Json<Value>, AppError> {
    let health = dashboard::health_check(&db).await?;
    Ok(Json(health))
}

/// Return aggregate health and usage overview for the authenticated user's skills.
#[tracing::instrument(skip_all)]
async fn overview_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
) -> Result<Json<Value>, AppError> {
    let overview = dashboard::get_overview(&db, auth.effective_user_id()).await?;
    Ok(Json(json!(overview)))
}

/// Return per-skill execution statistics, sortable by the provided field.
#[tracing::instrument(skip_all)]
async fn stats_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<StatsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = clamp_limit(params.limit, 50, 1000)?;
    let stats = dashboard::get_skill_stats(
        &db,
        auth.effective_user_id(),
        params.sort_by.as_deref(),
        limit,
    )
    .await?;
    Ok(Json(json!({ "stats": stats, "count": stats.len() })))
}

/// Return full dashboard detail for a single skill including stats and recent executions.
#[tracing::instrument(skip_all)]
async fn detail_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    skills::get_skill(&db, id, auth.effective_user_id()).await?;
    let detail = dashboard::get_skill_detail(&db, id).await?;
    Ok(Json(detail))
}

// --- Evolution handlers (hybrid: need state.llm for LLM-driven transforms) ---

#[tracing::instrument(skip_all)]
async fn evolve_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(req): Json<evolver::EvolutionRequest>,
) -> Result<Json<Value>, AppError> {
    let llm = state.llm.as_deref();
    let result = evolver::evolve(&db, llm, &req, "system", auth.effective_user_id()).await?;
    Ok(Json(json!(result)))
}

/// Run LLM-driven repair on a broken skill identified by ID.
#[tracing::instrument(skip_all)]
async fn fix_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let llm = state.llm.as_deref();
    let result = evolver::fix_skill(&db, llm, id, "system", auth.effective_user_id()).await?;
    Ok(Json(json!(result)))
}

/// Derive a new skill from one or more parent skills using LLM-guided synthesis.
#[tracing::instrument(skip_all)]
async fn derive_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<DeriveBody>,
) -> Result<Json<Value>, AppError> {
    let agent = body.agent.as_deref().unwrap_or("system");
    let llm = state.llm.as_deref();
    let result = evolver::derive_skill(
        &db,
        llm,
        &body.parent_ids,
        &body.direction,
        agent,
        auth.effective_user_id(),
    )
    .await?;
    Ok(Json(json!(result)))
}

/// Capture a new skill from a natural-language description using the LLM.
#[tracing::instrument(skip_all)]
async fn capture_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<CaptureBody>,
) -> Result<Json<Value>, AppError> {
    let agent = body.agent.as_deref().unwrap_or("system");
    let llm = state.llm.as_deref();
    let result =
        evolver::capture_skill(&db, llm, &body.description, agent, auth.effective_user_id())
            .await?;
    Ok(Json(json!(result)))
}

/// List skills that were recently evolved within the requested time window.
#[tracing::instrument(skip_all)]
async fn evolution_recent_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<EvolutionRecentParams>,
) -> Result<Json<Value>, AppError> {
    let hours = params.hours.unwrap_or(24).clamp(1, 24 * 30);
    let limit = clamp_limit(params.limit, 50, 500)?;
    let rows = skills::list_recent_evolutions(&db, auth.effective_user_id(), hours, limit).await?;
    Ok(Json(json!({ "recent": rows, "count": rows.len() })))
}

// --- Analyzer handlers ---

#[tracing::instrument(skip_all)]
async fn usage_stats_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
) -> Result<Json<Value>, AppError> {
    let stats = analyzer::get_usage_stats(&db, auth.effective_user_id()).await?;
    Ok(Json(stats))
}

// --- Cloud handlers (no DB access, just external HTTP) ---

#[tracing::instrument(skip_all)]
async fn cloud_search_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<CloudSearchBody>,
) -> Result<Json<Value>, AppError> {
    let limit = clamp_limit(body.limit, 20, 100)?;
    let results =
        cloud::search_skills_cloud(&db, &body.query, limit, auth.effective_user_id()).await?;
    Ok(Json(json!({ "results": results, "count": results.len() })))
}

/// Publish a skill to the local Skills Cloud (v50) registry. The legacy
/// route name (`/skills/cloud/upload`) is preserved for backward
/// compatibility with clients that still target the federated-cloud path
/// shape; the implementation now writes to the local skill registry.
#[tracing::instrument(skip_all)]
async fn cloud_upload_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<CloudUploadBody>,
) -> Result<Json<Value>, AppError> {
    let tags = body.tags.unwrap_or_default();
    // Source the agent slot from the PIV identity context when present
    // (canonical for signed sessions), falling back to the API key's
    // display name (the slot label set when the key was minted).
    let agent = auth
        .identity
        .as_ref()
        .map(|i| i.agent.clone())
        .unwrap_or_else(|| auth.key.name.clone());
    let result = cloud::upload_skill_to_cloud(
        &db,
        &agent,
        auth.effective_user_id(),
        &body.name,
        &body.description,
        &body.content,
        &body.category,
        &tags,
    )
    .await?;
    Ok(Json(json!({ "uploaded": true, "id": result })))
}

// --- Sync, Execute, Upload handlers (parity with original kleos) ---

/// Sync skills from filesystem directories.
/// Note: In the Rust version, skills are primarily stored in the database.
/// This endpoint scans specified directories for skill files and imports them.
#[tracing::instrument(skip_all)]
async fn sync_skills_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<SyncSkillsBody>,
) -> Result<Json<Value>, AppError> {
    // SECURITY: /skills/sync walks arbitrary filesystem paths and reads their
    // contents into the DB. Gate it to admin scope and enforce an env-driven
    // allowlist so a compromised read/write key cannot exfiltrate files.
    if !auth.has_scope(&Scope::Admin) {
        return Err(AppError(kleos_lib::EngError::Auth(
            "admin scope required for skill sync".into(),
        )));
    }
    let allowlist = skill_sync_allowlist();
    if allowlist.is_empty() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "skill sync disabled: set ENGRAM_SKILL_SYNC_PATHS to a colon-separated list of allowed roots".into(),
        )));
    }

    let dirs = body.dirs.unwrap_or_default();
    if dirs.is_empty() {
        return Ok(Json(json!({
            "synced": 0,
            "message": "No directories specified. Provide dirs array to sync from."
        })));
    }

    let mut synced = 0;
    let mut errors = Vec::new();

    for dir in &dirs {
        // SECURITY: never echo the requested path or the allowlist back to
        // the caller -- it leaks server filesystem layout. Log internally
        // and return an opaque rejection to the client.
        if !is_path_allowed(dir, &allowlist) {
            tracing::warn!(dir = %dir, "sync_skills: directory not in allowlist");
            errors.push("directory not permitted".to_string());
            continue;
        }
        let path = std::path::Path::new(dir);
        if !path.exists() || !path.is_dir() {
            tracing::warn!(dir = %dir, "sync_skills: directory not found");
            errors.push("directory not permitted".to_string());
            continue;
        }

        // Scan for .md files (skill format)
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let entry_path = entry.path();
                if entry_path.extension().map(|e| e == "md").unwrap_or(false) {
                    if let Ok(content) = std::fs::read_to_string(&entry_path) {
                        let name = entry_path
                            .file_stem()
                            .and_then(|n| n.to_str())
                            .unwrap_or("unknown")
                            .to_string();

                        // Create or update skill
                        let req = skills::CreateSkillRequest {
                            name: name.clone(),
                            description: Some(format!("Imported from {}", dir)),
                            code: content,
                            language: Some("markdown".to_string()),
                            agent: "system".to_string(),
                            parent_skill_id: None,
                            metadata: None,
                            user_id: Some(auth.effective_user_id()),
                            tags: Some(vec!["imported".to_string()]),
                            tool_deps: None,
                            // Skills Cloud (v50+): legacy /skills/sync path
                            // doesn't carry plugin provenance; leave defaults.
                            kind: None,
                            source_plugin: None,
                            source_path: None,
                            content_hash: None,
                        };

                        if skills::create_skill(&db, req).await.is_ok() {
                            synced += 1;
                        }
                    }
                }
            }
        }
    }

    Ok(Json(json!({
        "synced": synced,
        "dirs_scanned": dirs.len(),
        "errors": errors,
    })))
}

/// Execute a task using relevant skills as context.
/// Hybrid: needs state.llm for prompt completion.
#[tracing::instrument(skip_all)]
async fn execute_skills_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<ExecuteSkillsBody>,
) -> Result<Json<Value>, AppError> {
    let task = body.task.trim();
    if task.is_empty() {
        return Err(AppError::from(kleos_lib::EngError::InvalidInput(
            "task is required".into(),
        )));
    }

    // Check if LLM is available
    let Some(ref llm) = state.llm else {
        return Err(AppError::from(kleos_lib::EngError::Internal(
            "No LLM configured".into(),
        )));
    };

    // Search for relevant skills
    let search_results = search_skills(&db, task, auth.effective_user_id(), 5).await?;
    let skill_names: Vec<String> = search_results.iter().map(|r| r.name.clone()).collect();

    // Build context from top skills
    let mut skill_context = String::new();
    for result in search_results.iter().take(3) {
        if let Ok(skill) = skills::get_skill(&db, result.id, auth.effective_user_id()).await {
            skill_context.push_str(&format!(
                "<skill name=\"{}\">\n{}\n</skill>\n\n",
                skill.name, skill.code
            ));
        }
    }

    // Build system prompt
    let system = if skill_context.is_empty() {
        "You are a skilled assistant.".to_string()
    } else {
        format!(
            "You are a skilled assistant. Use the following skills as guidance:\n\n{}",
            skill_context
        )
    };

    // Call LLM with an explicit bounded budget. Background priority keeps the
    // shared Ollama queue fair; the per-call timeout bounds the HTTP request and
    // the route-level deadline (`execute_route`) bounds the queue wait, so a
    // saturated model surfaces as a prompt error rather than an open-ended hang.
    let opts = CallOptions {
        priority: Priority::Background,
        timeout_ms: Some(per_call_timeout_ms()),
        ..Default::default()
    };
    let response = llm
        .call(&system, task, Some(opts))
        .await
        .map_err(|e| AppError::from(kleos_lib::EngError::Internal(e.to_string())))?;

    Ok(Json(json!({
        "response": response,
        "skills_used": skill_names,
    })))
}

/// Upload a skill from a local directory to the cloud.
#[tracing::instrument(skip_all)]
async fn upload_skill_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<UploadSkillBody>,
) -> Result<Json<Value>, AppError> {
    let skill_dir = body.skill_dir.trim();
    if skill_dir.is_empty() {
        return Err(AppError::from(kleos_lib::EngError::InvalidInput(
            "skill_dir is required".into(),
        )));
    }

    // Try to find skill by path/name
    let path = std::path::Path::new(skill_dir);
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    // Search for matching skill in DB
    let skills_list = skills::list_skills(&db, auth.effective_user_id(), None, 100, 0).await?;
    let skill = skills_list.into_iter().find(|s| s.name == name);

    let Some(skill) = skill else {
        return Err(AppError::from(kleos_lib::EngError::NotFound(format!(
            "No skill found matching: {}. Run /skills/sync first.",
            skill_dir
        ))));
    };

    // With Skills Cloud v50, the local skill registry IS the cloud target,
    // so a successful lookup means the skill is already published. No
    // duplicate row is written. `body.tags` is accepted for backward
    // compatibility with clients that send it but is ignored (the local
    // skill's own tag set is the canonical one).
    let _ = body.tags;

    Ok(Json(json!({
        "uploaded": true,
        "skill_id": skill.id,
        "cloud_id": skill.id.to_string(),
    })))
}

// --- Skills Cloud (v50+) handlers ---

// POST /skills/find -- hybrid search returning ranked candidates with
// per-signal score breakdown. Replaces the role of /skills/search for the
// dispatch UX while leaving the legacy FTS-only route in place.
#[tracing::instrument(skip_all)]
async fn find_skills_handler(
    Auth(_auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<FindSkillsBody>,
) -> Result<Json<Value>, AppError> {
    let opts = FindOpts {
        kind: body.kind,
        plugin: body.plugin,
        tag: body.tag,
        limit: body.limit,
        include_deprecated: body.include_deprecated,
    };
    let results = find_skills(&db, &body.query, opts).await?;
    Ok(Json(json!({ "results": results, "count": results.len() })))
}

// POST /skills/aliases/resolve -- resolve a fuzzy alias string into ranked
// candidates without running the full hybrid pipeline. Cheaper for dispatch.
#[tracing::instrument(skip_all)]
async fn resolve_alias_handler(
    Auth(_auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<ResolveAliasBody>,
) -> Result<Json<Value>, AppError> {
    let limit = body.limit.unwrap_or(10).clamp(1, 100);
    let matches = skill_aliases::resolve_alias(&db, &body.query, limit).await?;
    Ok(Json(json!({ "matches": matches, "count": matches.len() })))
}

// GET /skills/{id}/aliases -- list every alias attached to a skill.
#[tracing::instrument(skip_all)]
async fn list_aliases_handler(
    Auth(_auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let aliases = skill_aliases::list_for_skill(&db, id).await?;
    Ok(Json(json!({ "aliases": aliases, "count": aliases.len() })))
}

// POST /skills/{id}/aliases -- add a user-driven alias. Confidence defaults
// to 1.0; auto-aliases (importer) come in via the lib API directly.
#[tracing::instrument(skip_all)]
async fn add_alias_handler(
    Auth(_auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
    Json(body): Json<AddAliasBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let confidence = body.confidence.unwrap_or(1.0).clamp(0.0, 1.0);
    // The importer marks aliases as 'auto' so the rebuild path can wipe
    // them without losing user-curated nicknames.
    let source = body.source.as_deref().unwrap_or("user");
    let source = match source {
        "auto" | "user" => source,
        _ => "user",
    };
    let alias_id = skill_aliases::add_alias(&db, id, &body.alias, confidence, source).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "id": alias_id, "skill_id": id, "alias": body.alias })),
    ))
}

// DELETE /skills/{id}/aliases/{alias} -- remove a single alias.
#[tracing::instrument(skip_all)]
async fn remove_alias_handler(
    Auth(_auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path((id, alias)): Path<(i64, String)>,
) -> Result<Json<Value>, AppError> {
    let n = skill_aliases::remove_alias(&db, id, &alias).await?;
    Ok(Json(
        json!({ "deleted": n, "skill_id": id, "alias": alias }),
    ))
}

// GET /skills/{id}/materialization -- check whether (and where) an agent
// has been written to disk.
#[tracing::instrument(skip_all)]
async fn get_materialization_handler(
    Auth(_auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let row = skill_materializations::get(&db, id).await?;
    Ok(Json(json!({ "materialization": row })))
}

// POST /skills/{id}/materialize -- the CLI calls this AFTER writing the
// agent .md so the DB row tracks where on disk it landed.
#[tracing::instrument(skip_all)]
async fn record_materialization_handler(
    Auth(_auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
    Json(body): Json<RecordMaterializationBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    skill_materializations::record(&db, id, &body.target_path, &body.content_hash).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "recorded": true, "skill_id": id })),
    ))
}

// DELETE /skills/{id}/materialization -- forget the materialization row.
// Caller is expected to delete the on-disk .md separately.
#[tracing::instrument(skip_all)]
async fn forget_materialization_handler(
    Auth(_auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let n = skill_materializations::forget(&db, id).await?;
    Ok(Json(json!({ "deleted": n, "skill_id": id })))
}

// GET /bundles -- list bundles with member counts.
#[tracing::instrument(skip_all)]
async fn list_bundles_handler(
    Auth(_auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<ListSkillsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = clamp_limit(params.limit, 50, 500)?;
    let bundles = skill_bundles::list_bundles(&db, limit).await?;
    Ok(Json(json!({ "bundles": bundles, "count": bundles.len() })))
}

// POST /bundles -- create (or upsert by name) a bundle.
#[tracing::instrument(skip_all)]
async fn create_bundle_handler(
    Auth(_auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<CreateBundleBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let id = skill_bundles::create_bundle(
        &db,
        &body.name,
        body.description.as_deref(),
        body.auto_generated.unwrap_or(false),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "id": id, "name": body.name })),
    ))
}

// GET /bundles/{id} -- bundle metadata only; members come from the dedicated route.
#[tracing::instrument(skip_all)]
async fn get_bundle_handler(
    Auth(_auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let bundle = skill_bundles::get_bundle(&db, id).await?;
    Ok(Json(json!(bundle)))
}

// DELETE /bundles/{id} -- drops the bundle and (via FK cascade) its members.
#[tracing::instrument(skip_all)]
async fn delete_bundle_handler(
    Auth(_auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    skill_bundles::delete_bundle(&db, id).await?;
    Ok(Json(json!({ "deleted": true, "id": id })))
}

// GET /bundles/{id}/skills -- ordered list of member skill ids.
#[tracing::instrument(skip_all)]
async fn list_bundle_members_handler(
    Auth(_auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let ids = skill_bundles::list_members(&db, id).await?;
    Ok(Json(
        json!({ "skill_ids": ids, "count": ids.len(), "bundle_id": id }),
    ))
}

// POST /bundles/{id}/skills -- add one skill to a bundle.
#[tracing::instrument(skip_all)]
async fn add_bundle_member_handler(
    Auth(_auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
    Json(body): Json<AddBundleMemberBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    skill_bundles::add_member(&db, id, body.skill_id).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "added": true, "bundle_id": id, "skill_id": body.skill_id })),
    ))
}

// DELETE /bundles/{id}/skills/{skill_id} -- remove one skill from a bundle.
#[tracing::instrument(skip_all)]
async fn remove_bundle_member_handler(
    Auth(_auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path((bundle_id, skill_id)): Path<(i64, i64)>,
) -> Result<Json<Value>, AppError> {
    let n = skill_bundles::remove_member(&db, bundle_id, skill_id).await?;
    Ok(Json(json!({
        "deleted": n,
        "bundle_id": bundle_id,
        "skill_id": skill_id,
    })))
}
