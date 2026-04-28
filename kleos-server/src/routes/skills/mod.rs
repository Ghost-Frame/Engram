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
use kleos_lib::skills::{
    self, analyzer, cloud, dashboard, evolver, search::search_skills, CreateSkillRequest,
    UpdateSkillRequest,
};
use kleos_lib::validation::MAX_PAGINATION_OFFSET;

mod types;
use types::{
    CaptureBody, CloudSearchBody, CloudUploadBody, DeriveBody, EvolutionRecentParams,
    ExecuteSkillsBody, GetExecutionsParams, JudgeBody, ListSkillsParams, RecordExecutionBody,
    RecordToolQualityBody, SearchSkillsBody, StatsParams, SyncSkillsBody, UploadSkillBody,
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

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        // CRUD
        .route(
            "/skills",
            post(create_skill_handler).get(list_skills_handler),
        )
        .route("/skills/search", post(search_skills_handler))
        .route("/skills/sync", post(sync_skills_handler))
        .route("/skills/execute", post(execute_skills_handler))
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
        // Evolution (LLM-backed, needs longer timeout than the global 120s)
        .merge(llm_routes())
        // Analyzer
        .route("/skills/usage-stats", get(usage_stats_handler))
        // Cloud
        .route("/skills/cloud/search", post(cloud_search_handler))
        .route("/skills/cloud/upload", post(cloud_upload_handler))
}

fn llm_routes() -> Router<AppState> {
    let timeout_ms: u64 = std::env::var("OLLAMA_TIMEOUT_BG_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60_000);
    Router::new()
        .route("/skills/evolve", post(evolve_handler))
        .route("/skills/{id}/fix", post(fix_handler))
        .route("/skills/derive", post(derive_handler))
        .route("/skills/capture", post(capture_handler))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_millis(timeout_ms),
        ))
}

// ---------------------------------------------------------------------------
// CRUD handlers
// ---------------------------------------------------------------------------

async fn create_skill_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(mut req): Json<CreateSkillRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    req.user_id = Some(auth.user_id);
    let skill = skills::create_skill(&db, req).await?;
    Ok((StatusCode::CREATED, Json(json!(skill))))
}

async fn list_skills_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<ListSkillsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = clamp_limit(params.limit, 50, 1000)?;
    let offset = clamp_offset(params.offset)?;
    let skill_list =
        skills::list_skills(&db, auth.user_id, params.agent.as_deref(), limit, offset).await?;
    Ok(Json(
        json!({ "skills": skill_list, "count": skill_list.len() }),
    ))
}

async fn search_skills_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<SearchSkillsBody>,
) -> Result<Json<Value>, AppError> {
    let limit = clamp_limit(body.limit, 20, 1000)?;
    let results = search_skills(&db, &body.query, auth.user_id, limit).await?;
    Ok(Json(json!({ "results": results, "count": results.len() })))
}

async fn get_skill_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let skill = skills::get_skill(&db, id, auth.user_id).await?;
    Ok(Json(json!(skill)))
}

async fn delete_skill_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    skills::delete_skill(&db, id, auth.user_id).await?;
    Ok(Json(json!({ "deleted": true, "id": id })))
}

async fn update_skill_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
    Json(req): Json<UpdateSkillRequest>,
) -> Result<Json<Value>, AppError> {
    let skill = skills::update_skill(&db, id, req, auth.user_id).await?;
    Ok(Json(json!(skill)))
}

async fn recompute_skill_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let skill = skills::recompute_skill(&db, id, auth.user_id).await?;
    Ok(Json(json!({
        "recomputed": true,
        "skill": skill,
    })))
}

// ---------------------------------------------------------------------------
// Execution handlers
// ---------------------------------------------------------------------------

async fn record_execution_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
    Json(body): Json<RecordExecutionBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    skills::record_execution(
        &db,
        id,
        auth.user_id,
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

async fn get_executions_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
    Query(params): Query<GetExecutionsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = clamp_limit(params.limit, 20, 1000)?;
    let executions = skills::get_executions(&db, id, auth.user_id, limit).await?;
    Ok(Json(
        json!({ "executions": executions, "count": executions.len() }),
    ))
}

// ---------------------------------------------------------------------------
// Judgment handlers
// ---------------------------------------------------------------------------

async fn judge_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
    Json(body): Json<JudgeBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let judgment = skills::add_judgment(
        &db,
        id,
        auth.user_id,
        &body.judge_agent,
        body.score,
        body.rationale.as_deref(),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(json!(judgment))))
}

async fn get_judgments_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let judgments = skills::get_judgments(&db, id, auth.user_id).await?;
    Ok(Json(
        json!({ "judgments": judgments, "count": judgments.len() }),
    ))
}

// ---------------------------------------------------------------------------
// Tags, deps, lineage handlers
// ---------------------------------------------------------------------------

async fn get_tags_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let tags = skills::get_skill_tags(&db, id, auth.user_id).await?;
    Ok(Json(json!({ "tags": tags })))
}

async fn get_deps_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let deps = skills::get_tool_deps(&db, id, auth.user_id).await?;
    Ok(Json(json!({ "deps": deps })))
}

async fn get_lineage_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let lineage = skills::get_lineage(&db, id, auth.user_id).await?;
    Ok(Json(json!({ "lineage": lineage })))
}

// ---------------------------------------------------------------------------
// Tool quality handlers
// ---------------------------------------------------------------------------

// SECURITY: relies on ResolvedDb shard isolation (Phase 5+) to scope to the caller's tenant. Do not add state.db calls here without re-binding auth.
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
async fn get_tool_quality_handler(
    Auth(_auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(tool_name): Path<String>,
) -> Result<Json<Value>, AppError> {
    let quality = skills::get_tool_quality(&db, &tool_name).await?;
    Ok(Json(json!(quality)))
}

// ---------------------------------------------------------------------------
// Dashboard handlers
// ---------------------------------------------------------------------------

// SECURITY: relies on ResolvedDb shard isolation (Phase 5+) to scope to the caller's tenant. Do not add state.db calls here without re-binding auth.
async fn health_handler(
    Auth(_auth): Auth,
    ResolvedDb(db): ResolvedDb,
) -> Result<Json<Value>, AppError> {
    let health = dashboard::health_check(&db).await?;
    Ok(Json(health))
}

async fn overview_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
) -> Result<Json<Value>, AppError> {
    let overview = dashboard::get_overview(&db, auth.user_id).await?;
    Ok(Json(json!(overview)))
}

async fn stats_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<StatsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = clamp_limit(params.limit, 50, 1000)?;
    let stats =
        dashboard::get_skill_stats(&db, auth.user_id, params.sort_by.as_deref(), limit).await?;
    Ok(Json(json!({ "stats": stats, "count": stats.len() })))
}

async fn detail_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    skills::get_skill(&db, id, auth.user_id).await?;
    let detail = dashboard::get_skill_detail(&db, id).await?;
    Ok(Json(detail))
}

// ---------------------------------------------------------------------------
// Evolution handlers (hybrid: need state.llm for LLM-driven transforms)
// ---------------------------------------------------------------------------

async fn evolve_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(req): Json<evolver::EvolutionRequest>,
) -> Result<Json<Value>, AppError> {
    let llm = state.llm.as_deref();
    let result = evolver::evolve(&db, llm, &req, "system", auth.user_id).await?;
    Ok(Json(json!(result)))
}

async fn fix_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let llm = state.llm.as_deref();
    let result = evolver::fix_skill(&db, llm, id, "system", auth.user_id).await?;
    Ok(Json(json!(result)))
}

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
        auth.user_id,
    )
    .await?;
    Ok(Json(json!(result)))
}

async fn capture_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<CaptureBody>,
) -> Result<Json<Value>, AppError> {
    let agent = body.agent.as_deref().unwrap_or("system");
    let llm = state.llm.as_deref();
    let result = evolver::capture_skill(&db, llm, &body.description, agent, auth.user_id).await?;
    Ok(Json(json!(result)))
}

async fn evolution_recent_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<EvolutionRecentParams>,
) -> Result<Json<Value>, AppError> {
    let hours = params.hours.unwrap_or(24).clamp(1, 24 * 30);
    let limit = clamp_limit(params.limit, 50, 500)?;
    let rows = skills::list_recent_evolutions(&db, auth.user_id, hours, limit).await?;
    Ok(Json(json!({ "recent": rows, "count": rows.len() })))
}

// ---------------------------------------------------------------------------
// Analyzer handlers
// ---------------------------------------------------------------------------

async fn usage_stats_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
) -> Result<Json<Value>, AppError> {
    let stats = analyzer::get_usage_stats(&db, auth.user_id).await?;
    Ok(Json(stats))
}

// ---------------------------------------------------------------------------
// Cloud handlers (no DB access, just external HTTP)
// ---------------------------------------------------------------------------

async fn cloud_search_handler(
    Auth(_auth): Auth,
    Json(body): Json<CloudSearchBody>,
) -> Result<Json<Value>, AppError> {
    let limit = clamp_limit(body.limit, 20, 100)?;
    let results = cloud::search_skills_cloud(&body.query, limit).await?;
    Ok(Json(json!({ "results": results, "count": results.len() })))
}

async fn cloud_upload_handler(
    Auth(_auth): Auth,
    Json(body): Json<CloudUploadBody>,
) -> Result<Json<Value>, AppError> {
    let tags = body.tags.unwrap_or_default();
    let result = cloud::upload_skill_to_cloud(
        &body.name,
        &body.description,
        &body.content,
        &body.category,
        &tags,
    )
    .await?;
    Ok(Json(json!({ "uploaded": true, "id": result })))
}

// ---------------------------------------------------------------------------
// Sync, Execute, Upload handlers (parity with original kleos)
// ---------------------------------------------------------------------------

/// Sync skills from filesystem directories.
/// Note: In the Rust version, skills are primarily stored in the database.
/// This endpoint scans specified directories for skill files and imports them.
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
                            user_id: Some(auth.user_id),
                            tags: Some(vec!["imported".to_string()]),
                            tool_deps: None,
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
    let search_results = search_skills(&db, task, auth.user_id, 5).await?;
    let skill_names: Vec<String> = search_results.iter().map(|r| r.name.clone()).collect();

    // Build context from top skills
    let mut skill_context = String::new();
    for result in search_results.iter().take(3) {
        if let Ok(skill) = skills::get_skill(&db, result.id, auth.user_id).await {
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

    // Call LLM
    let response = llm
        .call(&system, task, None)
        .await
        .map_err(|e| AppError::from(kleos_lib::EngError::Internal(e.to_string())))?;

    Ok(Json(json!({
        "response": response,
        "skills_used": skill_names,
    })))
}

/// Upload a skill from a local directory to the cloud.
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
    let skills_list = skills::list_skills(&db, auth.user_id, None, 100, 0).await?;
    let skill = skills_list.into_iter().find(|s| s.name == name);

    let Some(skill) = skill else {
        return Err(AppError::from(kleos_lib::EngError::NotFound(format!(
            "No skill found matching: {}. Run /skills/sync first.",
            skill_dir
        ))));
    };

    // Upload to cloud
    let tags = body.tags.unwrap_or_default();
    let description = skill.description.as_deref().unwrap_or("");
    let category = &skill.language;
    let result =
        cloud::upload_skill_to_cloud(&skill.name, description, &skill.code, category, &tags)
            .await?;

    Ok(Json(json!({
        "uploaded": true,
        "skill_id": skill.id,
        "cloud_id": result,
    })))
}
