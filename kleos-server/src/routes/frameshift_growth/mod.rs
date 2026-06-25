//! Frameshift cross-machine growth-log endpoints (Component 4).
//!
//! Mirrors the handoffs routes: all traffic lands on the reserved
//! `frameshift-growth` tenant shard (monolith fallback when sharding is off),
//! row-scoped by the authenticated `user_id`. `POST` requires `Write` scope and
//! is idempotent on `content_hash`; `GET` pulls incrementally past a cursor.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use kleos_lib::auth::Scope;
use kleos_lib::frameshift_growth::{FrameshiftGrowthDb, GrowthFilters, StoreParams};
use kleos_lib::tenant::FRAMESHIFT_GROWTH_TENANT_ID;
use kleos_lib::EngError;

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;

/// Wire the growth endpoints.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/frameshift-growth", post(store).get(list))
        .route("/frameshift-growth/search", get(search))
        .route("/frameshift-growth/cursor", get(cursor))
}

/// Resolve the growth database: the reserved `frameshift-growth` tenant shard
/// when sharding is enabled, else the monolith DB (which carries the table via
/// migration 90). Mirrors the handoffs resolver.
async fn get_db(state: &AppState) -> Result<FrameshiftGrowthDb, AppError> {
    match state.tenant_registry.as_ref() {
        Some(registry) => {
            let handle = registry
                .get_or_create(FRAMESHIFT_GROWTH_TENANT_ID)
                .await
                .map_err(|e| {
                    AppError(EngError::Internal(format!(
                        "frameshift-growth tenant load: {e}"
                    )))
                })?;
            Ok(FrameshiftGrowthDb::new(handle.database()))
        }
        None => Ok(FrameshiftGrowthDb::new(state.db.clone())),
    }
}

#[tracing::instrument(skip_all)]
async fn store(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(params): Json<StoreParams>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    if !auth.has_scope(&Scope::Write) {
        return Err(AppError(EngError::Auth("Write scope required".into())));
    }
    let db = get_db(&state).await?;
    let result = db.store(params, auth.effective_user_id()).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "ok": true, "id": result.id, "skipped": result.skipped })),
    ))
}

#[tracing::instrument(skip_all)]
async fn list(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(filters): Query<GrowthFilters>,
) -> Result<Json<Value>, AppError> {
    let db = get_db(&state).await?;
    let entries = db.list(filters, auth.effective_user_id()).await?;
    // Surface the highest id returned so the client can advance its cursor.
    let next_cursor = entries.iter().map(|e| e.id).max();
    Ok(Json(
        json!({ "entries": entries, "next_cursor": next_cursor }),
    ))
}

/// Query params for the search endpoint.
#[derive(Deserialize)]
struct SearchQuery {
    q: String,
    limit: Option<i64>,
}

#[tracing::instrument(skip_all)]
async fn search(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Value>, AppError> {
    let db = get_db(&state).await?;
    let entries = db
        .search(
            &query.q,
            auth.effective_user_id(),
            query.limit.unwrap_or(50),
        )
        .await?;
    Ok(Json(json!({ "entries": entries })))
}

#[tracing::instrument(skip_all)]
async fn cursor(State(state): State<AppState>, Auth(auth): Auth) -> Result<Json<Value>, AppError> {
    let db = get_db(&state).await?;
    let cursor = db.max_cursor(auth.effective_user_id()).await?;
    Ok(Json(json!({ "cursor": cursor })))
}
