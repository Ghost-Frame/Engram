use axum::{
    extract::Query,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::{Auth, ResolvedDb};
use crate::state::AppState;
use kleos_lib::intelligence::{
    growth::{context_growth, list_observations, materialize, reflect},
    types::{GrowthObservation, GrowthReflectRequest},
};

mod types;
use types::{ContextQuery, MaterializeBody, ObservationsQuery};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/growth/reflect", post(reflect_handler))
        .route("/growth/observations", get(observations_handler))
        .route("/growth/context", get(context_handler))
        .route("/growth/materialize", post(materialize_handler))
}

async fn reflect_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<GrowthReflectRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let result = reflect(&db, &body, auth.effective_user_id()).await?;
    Ok((StatusCode::CREATED, Json(json!(result))))
}

// SECURITY: relies on ResolvedDb shard isolation (Phase 5+) to scope to the caller's tenant. Do not add state.db calls here without re-binding auth.
async fn observations_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<ObservationsQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(20).min(100);
    let observations: Vec<GrowthObservation> = list_observations(&db, auth.user_id, limit).await?;
    let count = observations.len();
    Ok(Json(
        json!({ "observations": observations, "count": count }),
    ))
}

async fn materialize_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<MaterializeBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let new_id = materialize(&db, body.observation_id, auth.effective_user_id()).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "ok": true, "memory_id": new_id })),
    ))
}

/// GET /growth/context?q=<keywords>&limit=<n>
///
/// Returns the top-N growth observations scored by relevance to `q` (keyword
/// overlap + recency). Intended for injection into agent system prompts at
/// session start — analogous to the SIS active-lessons pattern in ArgentOS.
// SECURITY: relies on ResolvedDb shard isolation (Phase 5+) to scope to the caller's tenant.
async fn context_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<ContextQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(5).min(20);
    let query = params.q.unwrap_or_default();
    let observations = context_growth(&db, auth.user_id, &query, limit).await?;
    let count = observations.len();
    Ok(Json(
        json!({ "observations": observations, "count": count, "query": query }),
    ))
}
