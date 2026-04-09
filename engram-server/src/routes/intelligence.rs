use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use engram_lib::intelligence::{
    causal::{add_link, create_chain, get_chain, list_chains},
    consolidation::{consolidate, find_consolidation_candidates, list_consolidations},
    contradiction::{detect_contradictions, scan_all_contradictions},
    decomposition::decompose,
    digests::{generate_digest, list_digests},
    reflections::{create_reflection, list_reflections},
    temporal::{detect_patterns, list_patterns, store_pattern},
};
use engram_lib::memory;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{error::AppError, extractors::Auth, state::AppState};

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        // Consolidation
        .route("/intelligence/consolidate", post(consolidate_handler))
        .route(
            "/intelligence/consolidation-candidates",
            post(candidates_handler),
        )
        .route(
            "/intelligence/consolidations",
            get(list_consolidations_handler),
        )
        // Contradiction
        .route(
            "/intelligence/contradictions/{memory_id}",
            get(contradictions_handler),
        )
        .route(
            "/intelligence/contradictions",
            post(scan_contradictions_handler),
        )
        // Decomposition
        .route(
            "/intelligence/decompose/{memory_id}",
            post(decompose_handler),
        )
        // Temporal
        .route(
            "/intelligence/temporal/detect",
            post(detect_temporal_handler),
        )
        .route("/intelligence/temporal/patterns", get(list_temporal_handler))
        // Digests
        .route(
            "/intelligence/digests/generate",
            post(generate_digest_handler),
        )
        .route("/intelligence/digests", get(list_digests_handler))
        // Reflections
        .route(
            "/intelligence/reflections",
            post(create_reflection_handler).get(list_reflections_handler),
        )
        // Causal
        .route(
            "/intelligence/causal/chains",
            post(create_chain_handler).get(list_chains_handler),
        )
        .route("/intelligence/causal/chains/{id}", get(get_chain_handler))
        .route("/intelligence/causal/links", post(add_link_handler))
}

// ---------------------------------------------------------------------------
// Consolidation
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ConsolidateBody {
    pub memory_ids: Vec<i64>,
}

async fn consolidate_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Json(body): Json<ConsolidateBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let ids: Vec<String> = body.memory_ids.into_iter().map(|id| id.to_string()).collect();
    let result = consolidate(&state.db, &ids).await?;
    Ok((StatusCode::CREATED, Json(json!(result))))
}

#[derive(Debug, Deserialize)]
struct CandidatesBody {
    pub threshold: Option<f32>,
}

async fn candidates_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Json(body): Json<CandidatesBody>,
) -> Result<Json<Value>, AppError> {
    let threshold = body.threshold.unwrap_or(0.7);
    let groups = find_consolidation_candidates(&state.db, threshold).await?;
    Ok(Json(json!({ "groups": groups })))
}

#[derive(Debug, Deserialize)]
struct LimitQuery {
    pub limit: Option<usize>,
}

async fn list_consolidations_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<LimitQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(20);
    let items = list_consolidations(&state.db, auth.user_id, limit).await?;
    Ok(Json(json!({ "consolidations": items })))
}

// ---------------------------------------------------------------------------
// Contradiction
// ---------------------------------------------------------------------------

async fn contradictions_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(memory_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let mem = memory::get(&state.db, memory_id).await?;
    let contradictions = detect_contradictions(&state.db, &mem).await?;
    Ok(Json(json!({ "contradictions": contradictions })))
}

async fn scan_contradictions_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
) -> Result<Json<Value>, AppError> {
    let contradictions = scan_all_contradictions(&state.db).await?;
    Ok(Json(json!({ "contradictions": contradictions })))
}

// ---------------------------------------------------------------------------
// Decomposition
// ---------------------------------------------------------------------------

async fn decompose_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(memory_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let child_ids = decompose(&state.db, memory_id).await?;
    Ok(Json(json!({ "child_ids": child_ids })))
}

// ---------------------------------------------------------------------------
// Temporal
// ---------------------------------------------------------------------------

async fn detect_temporal_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let patterns = detect_patterns(&state.db, auth.user_id).await?;
    for pattern in &patterns {
        if let Err(e) = store_pattern(&state.db, pattern).await {
            tracing::warn!("failed to store temporal pattern: {}", e);
        }
    }
    Ok(Json(json!({ "patterns": patterns })))
}

async fn list_temporal_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<LimitQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(20);
    let patterns = list_patterns(&state.db, auth.user_id, limit).await?;
    Ok(Json(json!({ "patterns": patterns })))
}

// ---------------------------------------------------------------------------
// Digests
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct DigestBody {
    pub period: Option<String>,
}

async fn generate_digest_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<DigestBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let period = body.period.unwrap_or_else(|| "daily".into());
    let digest = generate_digest(&state.db, auth.user_id, &period).await?;
    Ok((StatusCode::CREATED, Json(json!(digest))))
}

async fn list_digests_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<LimitQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(20);
    let items = list_digests(&state.db, auth.user_id, limit).await?;
    Ok(Json(json!({ "digests": items })))
}

// ---------------------------------------------------------------------------
// Reflections
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateReflectionBody {
    pub content: String,
    pub reflection_type: Option<String>,
    pub source_memory_ids: Vec<i64>,
    pub confidence: Option<f64>,
}

async fn create_reflection_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<CreateReflectionBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let reflection_type = body
        .reflection_type
        .as_deref()
        .unwrap_or("general");
    let confidence = body.confidence.unwrap_or(1.0);
    let reflection = create_reflection(
        &state.db,
        &body.content,
        reflection_type,
        &body.source_memory_ids,
        confidence,
        auth.user_id,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(json!(reflection))))
}

async fn list_reflections_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<LimitQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(20);
    let items = list_reflections(&state.db, auth.user_id, limit).await?;
    Ok(Json(json!({ "reflections": items })))
}

// ---------------------------------------------------------------------------
// Causal
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateChainBody {
    pub root_memory_id: Option<i64>,
    pub description: Option<String>,
}

async fn create_chain_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<CreateChainBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let chain = create_chain(
        &state.db,
        body.root_memory_id,
        body.description.as_deref(),
        auth.user_id,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(json!(chain))))
}

async fn list_chains_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<LimitQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(20);
    let items = list_chains(&state.db, auth.user_id, limit).await?;
    Ok(Json(json!({ "chains": items })))
}

async fn get_chain_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let chain = get_chain(&state.db, id).await?;
    Ok(Json(json!(chain)))
}

#[derive(Debug, Deserialize)]
struct AddLinkBody {
    pub chain_id: i64,
    pub cause_memory_id: i64,
    pub effect_memory_id: i64,
    pub strength: Option<f64>,
    pub order_index: Option<i32>,
}

async fn add_link_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Json(body): Json<AddLinkBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let strength = body.strength.unwrap_or(1.0);
    let order_index = body.order_index.unwrap_or(0);
    let link = add_link(
        &state.db,
        body.chain_id,
        body.cause_memory_id,
        body.effect_memory_id,
        strength,
        order_index,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(json!(link))))
}
