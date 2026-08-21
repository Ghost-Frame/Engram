use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use kleos_lib::handoffs::{ExtractedAtom, HandoffFilters, HandoffsDb, StoreParams};
use kleos_lib::tenant::HANDOFFS_TENANT_ID;
use kleos_lib::EngError;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;

/// Builds the handoffs router, wiring all handoff and atom endpoints.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/handoffs", post(store_handoff).get(list_handoffs))
        .route("/handoffs/latest", get(get_latest))
        .route("/handoffs/candidates", get(list_candidates))
        .route("/handoffs/search", get(search_handoffs))
        .route("/handoffs/stats", get(get_stats))
        .route("/handoffs/gc", post(run_gc))
        .route("/handoffs/{id}", get(get_handoff).delete(delete_handoff))
        .route("/handoffs/atoms", get(list_atoms))
        .route("/handoffs/atoms/packed", get(get_packed_context))
        .route("/handoffs/atoms/supersede", post(supersede_atom))
        .route("/handoffs/atoms/decay", post(apply_decay))
}

/// Resolve the handoffs database: uses the reserved "handoffs" tenant shard
/// when tenant sharding is enabled, falls back to the global database
/// (which gets the handoffs table via migration v55) otherwise.
async fn get_db(state: &AppState) -> Result<HandoffsDb, AppError> {
    match state.tenant_registry.as_ref() {
        Some(registry) => {
            let handle = registry
                .get_or_create(HANDOFFS_TENANT_ID)
                .await
                .map_err(|e| AppError(EngError::Internal(format!("handoffs tenant load: {e}"))))?;
            Ok(HandoffsDb::new(
                handle.database(),
                state.handoffs_gc_sem.clone(),
            ))
        }
        None => Ok(HandoffsDb::new(
            state.db.clone(),
            state.handoffs_gc_sem.clone(),
        )),
    }
}

/// Request body for the store handoff endpoint.
#[derive(Deserialize)]
struct StoreHandoffRequest {
    #[serde(flatten)]
    params: StoreParams,
    /// Optional pre-extracted atoms to index alongside the handoff.
    atoms: Option<Vec<ExtractedAtom>>,
}

#[tracing::instrument(skip_all)]
/// Stores a validated repository or standalone handoff for the caller.
async fn store_handoff(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<StoreHandoffRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let db = get_db(&state).await?;
    let result = db
        .store_with_atoms(body.params, auth.effective_user_id(), body.atoms, None)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "ok": true, "id": result.id, "skipped": result.skipped })),
    ))
}

#[tracing::instrument(skip_all)]
/// Lists handoffs matching the caller's exact filters.
async fn list_handoffs(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(filters): Query<HandoffFilters>,
) -> Result<Json<Value>, AppError> {
    let db = get_db(&state).await?;
    let handoffs = db.list(filters, auth.effective_user_id()).await?;
    let count = handoffs.len();
    Ok(Json(json!({ "handoffs": handoffs, "count": count })))
}

#[tracing::instrument(skip_all)]
/// Returns the newest exact filter match without cross-project fallback.
async fn get_latest(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(filters): Query<HandoffFilters>,
) -> Result<Json<Value>, AppError> {
    let db = get_db(&state).await?;
    match db.get_latest(filters, auth.effective_user_id()).await? {
        Some(handoff) => Ok(Json(json!(handoff))),
        None => Err(AppError(EngError::NotFound("no handoff found".to_string()))),
    }
}

#[tracing::instrument(skip_all)]
/// Returns the newest checkpoint for every stable session identity.
async fn list_candidates(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(filters): Query<HandoffFilters>,
) -> Result<Json<Value>, AppError> {
    let db = get_db(&state).await?;
    let handoffs = db.candidates(filters, auth.effective_user_id()).await?;
    let count = handoffs.len();
    Ok(Json(json!({ "handoffs": handoffs, "count": count })))
}

#[tracing::instrument(skip_all)]
/// Returns one exact handoff id when it belongs to the caller.
async fn get_handoff(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let db = get_db(&state).await?;
    match db.get(id, auth.effective_user_id()).await? {
        Some(handoff) => Ok(Json(json!(handoff))),
        None => Err(AppError(EngError::NotFound(format!(
            "handoff {id} not found"
        )))),
    }
}

/// Query parameters accepted by full-text handoff search.
#[derive(Deserialize)]
struct SearchQuery {
    q: String,
    project: Option<String>,
    #[serde(default = "default_limit")]
    limit: usize,
}

/// Supplies the default FTS result limit.
fn default_limit() -> usize {
    10
}

#[tracing::instrument(skip_all)]
/// Searches handoff content within the caller's tenant.
async fn search_handoffs(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<SearchQuery>,
) -> Result<Json<Value>, AppError> {
    let db = get_db(&state).await?;
    let results = db
        .search(
            &params.q,
            params.project.as_deref(),
            params.limit as i64,
            auth.effective_user_id(),
        )
        .await?;
    let count = results.len();
    Ok(Json(json!({ "results": results, "count": count })))
}

#[tracing::instrument(skip_all)]
/// Returns aggregate handoff statistics for the caller.
async fn get_stats(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let db = get_db(&state).await?;
    let stats = db.stats(auth.effective_user_id()).await?;
    Ok(Json(json!(stats)))
}

/// Optional retention controls accepted by the handoff GC endpoint.
#[derive(Deserialize)]
struct GcParams {
    #[serde(default)]
    tiered: bool,
    keep: Option<i64>,
}

#[tracing::instrument(skip_all)]
/// Applies caller-scoped handoff retention.
async fn run_gc(
    State(state): State<AppState>,
    Auth(auth): Auth,
    body: Option<Json<GcParams>>,
) -> Result<Json<Value>, AppError> {
    let db = get_db(&state).await?;
    let (tiered, keep) = match body {
        Some(Json(p)) => (p.tiered, p.keep),
        None => (true, None),
    };
    let result = db.gc(tiered, keep, auth.effective_user_id()).await?;
    Ok(Json(
        json!({ "deleted": result.deleted, "remaining": result.remaining }),
    ))
}

#[tracing::instrument(skip_all)]
/// Deletes one exact caller-owned handoff.
async fn delete_handoff(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let db = get_db(&state).await?;
    let deleted = db.delete(id, auth.effective_user_id()).await?;
    Ok(Json(json!({ "ok": true, "deleted": deleted })))
}

/// Query parameters for the list atoms endpoint.
#[derive(Deserialize)]
struct ListAtomsQuery {
    project: String,
    atom_type: Option<String>,
    #[serde(default = "default_atom_status")]
    status: String,
    #[serde(default = "default_atom_limit")]
    limit: i64,
}

/// Supplies the default active atom status filter.
fn default_atom_status() -> String {
    "active".to_string()
}

/// Supplies the default atom listing limit.
fn default_atom_limit() -> i64 {
    100
}

/// Lists atoms for a project, optionally filtered by type and status.
#[tracing::instrument(skip_all)]
async fn list_atoms(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ListAtomsQuery>,
) -> Result<Json<Value>, AppError> {
    let db = get_db(&state).await?;
    let atoms = db
        .list_atoms(
            &params.project,
            params.atom_type.as_deref(),
            Some(&params.status),
            params.limit,
            auth.effective_user_id(),
        )
        .await?;
    let count = atoms.len();
    Ok(Json(json!({ "atoms": atoms, "count": count })))
}

/// Query parameters for the packed context endpoint.
#[derive(Deserialize)]
struct PackedContextQuery {
    project: String,
    #[serde(default = "default_max_tokens")]
    max_tokens: i64,
}

/// Supplies the default packed-context token budget.
fn default_max_tokens() -> i64 {
    4000
}

/// Returns a budget-packed context string for a project.
#[tracing::instrument(skip_all)]
async fn get_packed_context(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<PackedContextQuery>,
) -> Result<Json<Value>, AppError> {
    let db = get_db(&state).await?;
    let context = db
        .get_packed_context(
            &params.project,
            kleos_lib::validation::clamp_signed_limit(params.max_tokens, 4000, 128_000),
            auth.effective_user_id(),
        )
        .await?;
    Ok(Json(
        json!({ "context": context, "max_tokens": params.max_tokens }),
    ))
}

/// Request body for the supersede atom endpoint.
#[derive(Deserialize)]
struct SupersedeAtomRequest {
    old_atom_id: String,
    new_atom_id: String,
}

/// Marks an atom as superseded by a newer atom.
#[tracing::instrument(skip_all)]
async fn supersede_atom(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<SupersedeAtomRequest>,
) -> Result<Json<Value>, AppError> {
    let db = get_db(&state).await?;
    db.supersede_atom(
        &body.old_atom_id,
        &body.new_atom_id,
        auth.effective_user_id(),
    )
    .await?;
    Ok(Json(json!({ "ok": true })))
}

/// Request body for the apply decay endpoint.
#[derive(Deserialize)]
struct ApplyDecayRequest {
    project: String,
    #[serde(default = "default_sessions_elapsed")]
    sessions_elapsed: u32,
}

/// Supplies the default decay interval.
fn default_sessions_elapsed() -> u32 {
    1
}

/// Applies session decay to atoms in a project.
#[tracing::instrument(skip_all)]
async fn apply_decay(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<ApplyDecayRequest>,
) -> Result<Json<Value>, AppError> {
    let db = get_db(&state).await?;
    let affected = db
        .apply_session_decay(
            &body.project,
            body.sessions_elapsed,
            auth.effective_user_id(),
        )
        .await?;
    Ok(Json(
        json!({ "affected": affected, "sessions_elapsed": body.sessions_elapsed }),
    ))
}
