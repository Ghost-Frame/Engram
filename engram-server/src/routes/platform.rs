use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use kleos_lib::artifacts::{self, CreateArtifactRequest};
use kleos_lib::facts::{self, CreateFactRequest};
use kleos_lib::preferences;
use kleos_lib::projects::{self, CreateProjectRequest};
use kleos_lib::scratchpad;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{error::AppError, extractors::Auth, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/artifacts",
            post(create_artifact_handler).get(list_artifacts_handler),
        )
        .route("/artifacts/search", post(search_artifacts_handler))
        .route(
            "/artifacts/{id}",
            get(get_artifact_handler).delete(delete_artifact_handler),
        )
        .route("/facts", post(create_fact_handler).get(list_facts_handler))
        .route("/facts/{id}", axum::routing::delete(delete_fact_handler))
        .route("/state", post(set_state_handler))
        .route("/state/{agent}/{key}", get(get_state_handler))
        .route("/state/{agent}", get(list_state_handler))
        .route(
            "/preferences",
            post(set_preference_handler).get(list_preferences_handler),
        )
        .route(
            "/preferences/{key}",
            get(get_preference_handler).delete(delete_preference_handler),
        )
        .route(
            "/projects",
            post(create_project_handler).get(list_projects_handler),
        )
        .route(
            "/projects/{id}",
            get(get_project_handler).delete(delete_project_handler),
        )
        .route("/projects/{id}/link", post(link_memory_handler))
        .route("/projects/{id}/memories", get(project_memories_handler))
        .route("/scratchpad", post(scratchpad_set_handler))
        .route(
            "/scratchpad/{agent}/{key}",
            get(scratchpad_get_handler).delete(scratchpad_delete_handler),
        )
        .route("/scratchpad/{agent}", get(scratchpad_list_handler))
}

#[derive(Debug, Deserialize)]
struct ListArtifactsParams {
    limit: Option<usize>,
    offset: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct SearchArtifactsBody {
    query: String,
    limit: Option<usize>,
}

async fn create_artifact_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(mut req): Json<CreateArtifactRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    req.user_id = Some(auth.user_id);
    let artifact = artifacts::create_artifact(&state.db, req).await?;
    Ok((StatusCode::CREATED, Json(json!(artifact))))
}

async fn list_artifacts_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ListArtifactsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(50);
    let offset = params.offset.unwrap_or(0);
    let items = artifacts::list_artifacts(&state.db, auth.user_id, limit, offset).await?;
    Ok(Json(json!({ "artifacts": items, "count": items.len() })))
}

async fn get_artifact_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let artifact = artifacts::get_artifact(&state.db, id).await?;
    Ok(Json(json!(artifact)))
}

async fn delete_artifact_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    artifacts::delete_artifact(&state.db, id).await?;
    Ok(Json(json!({ "deleted": true, "id": id })))
}

async fn search_artifacts_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<SearchArtifactsBody>,
) -> Result<Json<Value>, AppError> {
    let limit = body.limit.unwrap_or(20);
    let results = artifacts::search_artifacts(&state.db, &body.query, auth.user_id, limit).await?;
    Ok(Json(json!({ "results": results, "count": results.len() })))
}

#[derive(Debug, Deserialize)]
struct ListFactsParams {
    limit: Option<usize>,
    memory_id: Option<i64>,
}

async fn create_fact_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(mut req): Json<CreateFactRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    req.user_id = Some(auth.user_id);
    let fact = facts::create_fact(&state.db, req).await?;
    Ok((StatusCode::CREATED, Json(json!(fact))))
}

async fn list_facts_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ListFactsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(50);
    let items = facts::list_facts(&state.db, auth.user_id, params.memory_id, limit).await?;
    Ok(Json(json!({ "facts": items, "count": items.len() })))
}

async fn delete_fact_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    facts::delete_fact(&state.db, id).await?;
    Ok(Json(json!({ "deleted": true, "id": id })))
}

#[derive(Debug, Deserialize)]
struct SetStateBody {
    agent: String,
    key: String,
    value: String,
}

async fn set_state_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<SetStateBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let entry = facts::set_state(&state.db, &body.agent, &body.key, &body.value, auth.user_id)
        .await?;
    Ok((StatusCode::CREATED, Json(json!(entry))))
}

async fn get_state_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path((agent, key)): Path<(String, String)>,
) -> Result<Json<Value>, AppError> {
    let entry = facts::get_state(&state.db, &agent, &key, auth.user_id).await?;
    Ok(Json(json!(entry)))
}

async fn list_state_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(agent): Path<String>,
) -> Result<Json<Value>, AppError> {
    let entries = facts::list_state(&state.db, &agent, auth.user_id).await?;
    Ok(Json(json!({ "entries": entries, "count": entries.len() })))
}

#[derive(Debug, Deserialize)]
struct SetPreferenceBody {
    key: String,
    value: String,
}

async fn set_preference_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<SetPreferenceBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let pref = preferences::set_preference(&state.db, auth.user_id, &body.key, &body.value).await?;
    Ok((StatusCode::CREATED, Json(json!(pref))))
}

async fn list_preferences_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let prefs = preferences::list_preferences(&state.db, auth.user_id).await?;
    Ok(Json(json!({ "preferences": prefs })))
}

async fn get_preference_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(key): Path<String>,
) -> Result<Json<Value>, AppError> {
    let pref = preferences::get_preference(&state.db, auth.user_id, &key).await?;
    Ok(Json(json!(pref)))
}

async fn delete_preference_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(key): Path<String>,
) -> Result<Json<Value>, AppError> {
    preferences::delete_preference(&state.db, auth.user_id, &key).await?;
    Ok(Json(json!({ "deleted": true, "key": key })))
}

#[derive(Debug, Deserialize)]
struct ListProjectsParams {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct LinkMemoryBody {
    memory_id: i64,
}

async fn create_project_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(mut req): Json<CreateProjectRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    req.user_id = Some(auth.user_id);
    let project = projects::create_project(&state.db, req).await?;
    Ok((StatusCode::CREATED, Json(json!(project))))
}

async fn list_projects_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ListProjectsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(50);
    let items = projects::list_projects(&state.db, auth.user_id, limit).await?;
    Ok(Json(json!({ "projects": items, "count": items.len() })))
}

async fn get_project_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let project = projects::get_project(&state.db, id).await?;
    Ok(Json(json!(project)))
}

async fn delete_project_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    projects::delete_project(&state.db, id).await?;
    Ok(Json(json!({ "deleted": true, "id": id })))
}

async fn link_memory_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<LinkMemoryBody>,
) -> Result<Json<Value>, AppError> {
    projects::link_memory_to_project(&state.db, body.memory_id, id).await?;
    Ok(Json(json!({ "linked": true, "project_id": id, "memory_id": body.memory_id })))
}

async fn project_memories_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let memory_ids = projects::get_project_memories(&state.db, id).await?;
    Ok(Json(json!({ "memory_ids": memory_ids })))
}

#[derive(Debug, Deserialize)]
struct ScratchpadSetBody {
    agent: String,
    key: String,
    content: String,
    ttl_seconds: Option<i64>,
}

async fn scratchpad_set_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<ScratchpadSetBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let entry = scratchpad::set(
        &state.db,
        &body.agent,
        &body.key,
        &body.content,
        auth.user_id,
        body.ttl_seconds,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(json!(entry))))
}

async fn scratchpad_get_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path((agent, key)): Path<(String, String)>,
) -> Result<Json<Value>, AppError> {
    let entry = scratchpad::get(&state.db, &agent, &key, auth.user_id).await?;
    Ok(Json(json!(entry)))
}

async fn scratchpad_list_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(agent): Path<String>,
) -> Result<Json<Value>, AppError> {
    let entries = scratchpad::list(&state.db, &agent, auth.user_id).await?;
    Ok(Json(json!({ "entries": entries, "count": entries.len() })))
}

async fn scratchpad_delete_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path((agent, key)): Path<(String, String)>,
) -> Result<Json<Value>, AppError> {
    scratchpad::delete(&state.db, &agent, &key, auth.user_id).await?;
    Ok(Json(json!({ "deleted": true, "agent": agent, "key": key })))
}
