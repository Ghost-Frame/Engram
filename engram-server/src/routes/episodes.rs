use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use engram_lib::episodes::{
    self, CreateEpisodeRequest, UpdateEpisodeRequest, AssignMemoriesRequest,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/episodes", post(create_episode).get(list_episodes))
        .route("/episodes/{id}", get(get_episode).patch(update_episode))
        .route("/episodes/{id}/memories", post(assign_memories))
        .route("/episodes/{id}/finalize", post(finalize_episode))
}

// POST /episodes
async fn create_episode(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<CreateEpisodeRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let ep = episodes::create_episode(&state.db, body, auth.user_id).await?;
    Ok((StatusCode::CREATED, Json(json!({
        "created": true, "id": ep.id, "started_at": ep.started_at, "summary": ep.summary
    }))))
}

// GET /episodes
#[derive(Debug, Deserialize)]
struct ListEpisodesParams {
    limit: Option<usize>,
    query: Option<String>,
    after: Option<String>,
    before: Option<String>,
}

async fn list_episodes(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ListEpisodesParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(20).min(100);

    // Temporal search
    if params.after.is_some() || params.before.is_some() {
        let after = params.after.as_deref().unwrap_or("2000-01-01");
        let before = params.before.as_deref().unwrap_or("2099-12-31");
        let eps = episodes::list_episodes_by_time_range(
            &state.db, auth.user_id, after, before, limit,
        ).await?;
        return Ok(Json(json!({ "episodes": eps })));
    }

    // FTS search
    if let Some(ref query) = params.query {
        let eps = episodes::search_episodes_fts(&state.db, query, auth.user_id, limit).await?;
        return Ok(Json(json!({ "episodes": eps })));
    }

    // Default: list recent
    let eps = episodes::list_episodes(&state.db, auth.user_id, limit).await?;
    Ok(Json(json!({ "episodes": eps })))
}

// GET /episodes/{id}
async fn get_episode(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let episode = episodes::get_episode_for_user(&state.db, id, auth.user_id).await?;
    let memories = episodes::get_episode_memories(&state.db, id, auth.user_id).await?;
    Ok(Json(json!({
        "id": episode.id, "title": episode.title, "session_id": episode.session_id,
        "agent": episode.agent, "summary": episode.summary, "user_id": episode.user_id,
        "memory_count": episode.memory_count, "duration_seconds": episode.duration_seconds,
        "decay_score": episode.decay_score, "started_at": episode.started_at,
        "ended_at": episode.ended_at, "created_at": episode.created_at,
        "memories": memories,
    })))
}

// PATCH /episodes/{id}
async fn update_episode(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<UpdateEpisodeRequest>,
) -> Result<Json<Value>, AppError> {
    // Verify episode exists for this user
    episodes::get_episode_for_user(&state.db, id, auth.user_id).await?;
    episodes::update_episode_for_user(&state.db, id, auth.user_id, &body).await?;
    Ok(Json(json!({ "updated": true, "id": id })))
}

// POST /episodes/{id}/memories
async fn assign_memories(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<AssignMemoriesRequest>,
) -> Result<Json<Value>, AppError> {
    // Verify episode exists for this user
    episodes::get_episode_for_user(&state.db, id, auth.user_id).await?;
    let assigned = episodes::assign_memories_to_episode(
        &state.db, id, &body.memory_ids, auth.user_id,
    ).await?;
    Ok(Json(json!({ "assigned": assigned, "episode_id": id })))
}

// POST /episodes/{id}/finalize
async fn finalize_episode(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let ep = episodes::finalize_episode(&state.db, id, auth.user_id).await?;
    Ok(Json(json!({
        "finalized": true, "id": ep.id, "summary": ep.summary, "memory_count": ep.memory_count
    })))
}
