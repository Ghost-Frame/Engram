use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use engram_lib::skills::{
    self,
    search::search_skills,
    analyzer, dashboard, evolver, cloud,
    CreateSkillRequest, UpdateSkillRequest,
};

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        // CRUD
        .route("/skills", post(create_skill_handler).get(list_skills_handler))
        .route("/skills/search", post(search_skills_handler))
        .route("/skills/{id}", get(get_skill_handler).delete(delete_skill_handler))
        .route("/skills/{id}/update", post(update_skill_handler))
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
        // Evolution
        .route("/skills/evolve", post(evolve_handler))
        .route("/skills/{id}/fix", post(fix_handler))
        .route("/skills/derive", post(derive_handler))
        .route("/skills/capture", post(capture_handler))
        // Analyzer
        .route("/skills/usage-stats", get(usage_stats_handler))
        // Cloud
        .route("/skills/cloud/search", post(cloud_search_handler))
        .route("/skills/cloud/upload", post(cloud_upload_handler))
}
// ---------------------------------------------------------------------------
// Query / body structs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ListSkillsParams { limit: Option<usize>, offset: Option<usize>, agent: Option<String> }

#[derive(Debug, Deserialize)]
struct SearchSkillsBody { query: String, limit: Option<usize> }

#[derive(Debug, Deserialize)]
struct RecordExecutionBody { success: bool, duration_ms: Option<f64>, error_type: Option<String>, error_message: Option<String> }

#[derive(Debug, Deserialize)]
struct GetExecutionsParams { limit: Option<usize> }

#[derive(Debug, Deserialize)]
struct JudgeBody { judge_agent: String, score: f64, rationale: Option<String> }

#[derive(Debug, Deserialize)]
struct RecordToolQualityBody { tool_name: String, agent: String, success: bool, latency_ms: Option<f64>, error_type: Option<String> }

#[derive(Debug, Deserialize)]
struct StatsParams { sort_by: Option<String>, limit: Option<usize> }

#[derive(Debug, Deserialize)]
struct CaptureBody { description: String, agent: Option<String> }

#[derive(Debug, Deserialize)]
struct DeriveBody { parent_ids: Vec<i64>, direction: String, agent: Option<String> }

#[derive(Debug, Deserialize)]
struct CloudSearchBody { query: String, limit: Option<usize> }

#[derive(Debug, Deserialize)]
struct CloudUploadBody { name: String, description: String, content: String, category: String, tags: Option<Vec<String>> }

// ---------------------------------------------------------------------------
// CRUD handlers
// ---------------------------------------------------------------------------

async fn create_skill_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(mut req): Json<CreateSkillRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    req.user_id = Some(auth.user_id);
    let skill = skills::create_skill(&state.db, req).await?;
    Ok((StatusCode::CREATED, Json(json!(skill))))
}

async fn list_skills_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ListSkillsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(50);
    let offset = params.offset.unwrap_or(0);
    let skill_list = skills::list_skills(&state.db, auth.user_id, params.agent.as_deref(), limit, offset).await?;
    Ok(Json(json!({ "skills": skill_list, "count": skill_list.len() })))
}

async fn search_skills_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<SearchSkillsBody>,
) -> Result<Json<Value>, AppError> {
    let limit = body.limit.unwrap_or(20);
    let results = search_skills(&state.db, &body.query, auth.user_id, limit).await?;
    Ok(Json(json!({ "results": results, "count": results.len() })))
}

async fn get_skill_handler(
    State(state): State<AppState>, Auth(auth): Auth, Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let skill = skills::get_skill(&state.db, id, auth.user_id).await?;
    Ok(Json(json!(skill)))
}

async fn delete_skill_handler(
    State(state): State<AppState>, Auth(auth): Auth, Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    skills::delete_skill(&state.db, id, auth.user_id).await?;
    Ok(Json(json!({ "deleted": true, "id": id })))
}

async fn update_skill_handler(
    State(state): State<AppState>, Auth(auth): Auth, Path(id): Path<i64>,
    Json(req): Json<UpdateSkillRequest>,
) -> Result<Json<Value>, AppError> {
    let skill = skills::update_skill(&state.db, id, req, auth.user_id).await?;
    Ok(Json(json!(skill)))
}
// ---------------------------------------------------------------------------
// Execution handlers
// ---------------------------------------------------------------------------

async fn record_execution_handler(
    State(state): State<AppState>, Auth(auth): Auth, Path(id): Path<i64>,
    Json(body): Json<RecordExecutionBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    skills::get_skill(&state.db, id, auth.user_id).await?;
    skills::record_execution(&state.db, id, body.success, body.duration_ms, body.error_type.as_deref(), body.error_message.as_deref()).await?;
    Ok((StatusCode::CREATED, Json(json!({ "recorded": true, "skill_id": id }))))
}

async fn get_executions_handler(
    State(state): State<AppState>, Auth(auth): Auth, Path(id): Path<i64>,
    Query(params): Query<GetExecutionsParams>,
) -> Result<Json<Value>, AppError> {
    skills::get_skill(&state.db, id, auth.user_id).await?;
    let limit = params.limit.unwrap_or(20);
    let executions = skills::get_executions(&state.db, id, limit).await?;
    Ok(Json(json!({ "executions": executions, "count": executions.len() })))
}

// ---------------------------------------------------------------------------
// Judgment handlers
// ---------------------------------------------------------------------------

async fn judge_handler(
    State(state): State<AppState>, Auth(auth): Auth, Path(id): Path<i64>,
    Json(body): Json<JudgeBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    skills::get_skill(&state.db, id, auth.user_id).await?;
    let judgment = skills::add_judgment(&state.db, id, &body.judge_agent, body.score, body.rationale.as_deref()).await?;
    Ok((StatusCode::CREATED, Json(json!(judgment))))
}

async fn get_judgments_handler(
    State(state): State<AppState>, Auth(auth): Auth, Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    skills::get_skill(&state.db, id, auth.user_id).await?;
    let judgments = skills::get_judgments(&state.db, id).await?;
    Ok(Json(json!({ "judgments": judgments, "count": judgments.len() })))
}

// ---------------------------------------------------------------------------
// Tags, deps, lineage handlers
// ---------------------------------------------------------------------------

async fn get_tags_handler(
    State(state): State<AppState>, Auth(auth): Auth, Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    skills::get_skill(&state.db, id, auth.user_id).await?;
    let tags = skills::get_skill_tags(&state.db, id).await?;
    Ok(Json(json!({ "tags": tags })))
}

async fn get_deps_handler(
    State(state): State<AppState>, Auth(auth): Auth, Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    skills::get_skill(&state.db, id, auth.user_id).await?;
    let deps = skills::get_tool_deps(&state.db, id).await?;
    Ok(Json(json!({ "deps": deps })))
}

async fn get_lineage_handler(
    State(state): State<AppState>, Auth(auth): Auth, Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    skills::get_skill(&state.db, id, auth.user_id).await?;
    let lineage = skills::get_lineage(&state.db, id).await?;
    Ok(Json(json!({ "lineage": lineage })))
}

// ---------------------------------------------------------------------------
// Tool quality handlers
// ---------------------------------------------------------------------------

async fn record_tool_quality_handler(
    State(state): State<AppState>, Auth(_auth): Auth,
    Json(body): Json<RecordToolQualityBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    skills::record_tool_quality(&state.db, &body.tool_name, &body.agent, body.success, body.latency_ms, body.error_type.as_deref()).await?;
    Ok((StatusCode::CREATED, Json(json!({ "recorded": true, "tool_name": body.tool_name }))))
}

async fn get_tool_quality_handler(
    State(state): State<AppState>, Auth(_auth): Auth, Path(tool_name): Path<String>,
) -> Result<Json<Value>, AppError> {
    let quality = skills::get_tool_quality(&state.db, &tool_name).await?;
    Ok(Json(json!(quality)))
}
// ---------------------------------------------------------------------------
// Dashboard handlers
// ---------------------------------------------------------------------------

async fn health_handler(
    State(state): State<AppState>, Auth(_auth): Auth,
) -> Result<Json<Value>, AppError> {
    let health = dashboard::health_check(&state.db).await?;
    Ok(Json(health))
}

async fn overview_handler(
    State(state): State<AppState>, Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let overview = dashboard::get_overview(&state.db, auth.user_id).await?;
    Ok(Json(json!(overview)))
}

async fn stats_handler(
    State(state): State<AppState>, Auth(auth): Auth,
    Query(params): Query<StatsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(50);
    let stats = dashboard::get_skill_stats(&state.db, auth.user_id, params.sort_by.as_deref(), limit).await?;
    Ok(Json(json!({ "stats": stats, "count": stats.len() })))
}

async fn detail_handler(
    State(state): State<AppState>, Auth(auth): Auth, Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    skills::get_skill(&state.db, id, auth.user_id).await?;
    let detail = dashboard::get_skill_detail(&state.db, id).await?;
    Ok(Json(detail))
}

// ---------------------------------------------------------------------------
// Evolution handlers
// ---------------------------------------------------------------------------

async fn evolve_handler(
    State(state): State<AppState>, Auth(auth): Auth,
    Json(req): Json<evolver::EvolutionRequest>,
) -> Result<Json<Value>, AppError> {
    let result = evolver::evolve(&state.db, &req, "system", auth.user_id).await?;
    Ok(Json(json!(result)))
}

async fn fix_handler(
    State(state): State<AppState>, Auth(auth): Auth, Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let result = evolver::fix_skill(&state.db, id, "system", auth.user_id).await?;
    Ok(Json(json!(result)))
}

async fn derive_handler(
    State(state): State<AppState>, Auth(auth): Auth,
    Json(body): Json<DeriveBody>,
) -> Result<Json<Value>, AppError> {
    let agent = body.agent.as_deref().unwrap_or("system");
    let result = evolver::derive_skill(&state.db, &body.parent_ids, &body.direction, agent, auth.user_id).await?;
    Ok(Json(json!(result)))
}

async fn capture_handler(
    State(state): State<AppState>, Auth(auth): Auth,
    Json(body): Json<CaptureBody>,
) -> Result<Json<Value>, AppError> {
    let agent = body.agent.as_deref().unwrap_or("system");
    let result = evolver::capture_skill(&state.db, &body.description, agent, auth.user_id).await?;
    Ok(Json(json!(result)))
}

// ---------------------------------------------------------------------------
// Analyzer handlers
// ---------------------------------------------------------------------------

async fn usage_stats_handler(
    State(state): State<AppState>, Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let stats = analyzer::get_usage_stats(&state.db, auth.user_id).await?;
    Ok(Json(stats))
}

// ---------------------------------------------------------------------------
// Cloud handlers
// ---------------------------------------------------------------------------

async fn cloud_search_handler(
    State(_state): State<AppState>, Auth(_auth): Auth,
    Json(body): Json<CloudSearchBody>,
) -> Result<Json<Value>, AppError> {
    let results = cloud::search_skills_cloud(&body.query, body.limit.unwrap_or(20)).await?;
    Ok(Json(json!({ "results": results, "count": results.len() })))
}

async fn cloud_upload_handler(
    State(_state): State<AppState>, Auth(_auth): Auth,
    Json(body): Json<CloudUploadBody>,
) -> Result<Json<Value>, AppError> {
    let tags = body.tags.unwrap_or_default();
    let result = cloud::upload_skill_to_cloud(&body.name, &body.description, &body.content, &body.category, &tags).await?;
    Ok(Json(json!({ "uploaded": true, "id": result })))
}