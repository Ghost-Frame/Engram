use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use kleos_lib::handoffs::{HandoffFilters, HandoffsDb, StoreParams};
use kleos_lib::EngError;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/handoffs", post(store_handoff).get(list_handoffs))
        .route("/handoffs/latest", get(get_latest))
        .route("/handoffs/search", get(search_handoffs))
        .route("/handoffs/stats", get(get_stats))
        .route("/handoffs/gc", post(run_gc))
        .route("/handoffs/{id}", delete(delete_handoff))
}

fn get_db(state: &AppState) -> Result<&Arc<HandoffsDb>, AppError> {
    state.handoffs_db.as_ref().ok_or_else(|| {
        AppError(EngError::NotImplemented(
            "handoffs subsystem not enabled".to_string(),
        ))
    })
}

async fn store_handoff(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Json(params): Json<StoreParams>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let db = get_db(&state)?;
    let result = db.store(params).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "ok": true, "id": result.id, "skipped": result.skipped })),
    ))
}

async fn list_handoffs(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Query(filters): Query<HandoffFilters>,
) -> Result<Json<Value>, AppError> {
    let db = get_db(&state)?;
    let handoffs = db.list(filters).await?;
    let count = handoffs.len();
    Ok(Json(json!({ "handoffs": handoffs, "count": count })))
}

async fn get_latest(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Query(filters): Query<HandoffFilters>,
) -> Result<Json<Value>, AppError> {
    let db = get_db(&state)?;
    match db.get_latest(filters).await? {
        Some(handoff) => Ok(Json(json!(handoff))),
        None => Err(AppError(EngError::NotFound("no handoff found".to_string()))),
    }
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
    project: Option<String>,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    10
}

async fn search_handoffs(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Query(params): Query<SearchQuery>,
) -> Result<Json<Value>, AppError> {
    let db = get_db(&state)?;
    let results = db
        .search(&params.q, params.project.as_deref(), params.limit as i64)
        .await?;
    let count = results.len();
    Ok(Json(json!({ "results": results, "count": count })))
}

async fn get_stats(
    State(state): State<AppState>,
    Auth(_auth): Auth,
) -> Result<Json<Value>, AppError> {
    let db = get_db(&state)?;
    let stats = db.stats().await?;
    Ok(Json(json!(stats)))
}

#[derive(Deserialize)]
struct GcParams {
    #[serde(default)]
    tiered: bool,
    keep: Option<i64>,
}

async fn run_gc(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    body: Option<Json<GcParams>>,
) -> Result<Json<Value>, AppError> {
    let db = get_db(&state)?;
    let (tiered, keep) = match body {
        Some(Json(p)) => (p.tiered, p.keep),
        None => (true, None),
    };
    let result = db.gc(tiered, keep).await?;
    Ok(Json(json!({ "deleted": result.deleted, "remaining": result.remaining })))
}

async fn delete_handoff(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let db = get_db(&state)?;
    let deleted = db.delete(id).await?;
    Ok(Json(json!({ "ok": true, "deleted": deleted })))
}
