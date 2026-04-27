use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::{Auth, ResolvedDb};
use crate::state::AppState;
use kleos_lib::services::brain::{
    get_memory_for_absorb, verify_memory_ownership, AbsorbRequest, BrainQueryOptions, DecayRequest,
    FeedbackRequest,
};

#[allow(dead_code)]
mod types;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/brain/stats", get(stats_handler))
        .route("/brain/query", post(query_handler))
        .route("/brain/absorb", post(absorb_handler))
        .route("/brain/dream", post(dream_handler))
        .route("/brain/feedback", post(feedback_handler))
        .route("/brain/decay", post(decay_handler))
        .route(
            "/brain/evolution/feedback",
            post(evolution_feedback_handler),
        )
        .route("/brain/evolution/train", post(evolution_train_handler))
        .route("/brain/evolution/stats", get(evolution_stats_handler))
}

async fn require_brain(state: &AppState) -> Result<(), AppError> {
    if let Some(ref brain) = state.brain {
        if brain.is_ready() {
            return Ok(());
        }
    }
    Err(AppError(kleos_lib::EngError::Internal(
        "brain not ready".into(),
    )))
}

// Stats are global brain telemetry (no per-tenant data); auth required but
// no user_id needed.
async fn stats_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_brain(&state).await?;
    let brain = state
        .brain
        .as_ref()
        .ok_or_else(|| AppError(kleos_lib::EngError::Internal("brain not configured".into())))?;
    let stats = brain.stats().await?;
    Ok(Json(json!({ "ok": true, "stats": stats })))
}

// Query is read-only against the global brain index; the per-user filter
// inside the brain is the responsibility of the embedder + reranker pipeline.
async fn query_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Json(body): Json<BrainQueryOptions>,
) -> Result<Json<Value>, AppError> {
    require_brain(&state).await?;
    let brain = state
        .brain
        .as_ref()
        .ok_or_else(|| AppError(kleos_lib::EngError::Internal("brain not configured".into())))?;
    let embedder = state.current_embedder().await.ok_or_else(|| {
        AppError(kleos_lib::EngError::Internal(
            "embedder not ready (still loading)".into(),
        ))
    })?;
    let result = brain.query(embedder.as_ref(), &body.query, &body).await?;
    Ok(Json(json!({ "ok": true, "result": result })))
}

// C-R3-001: absorb fetches the memory from the caller's tenant DB and pipes
// auth.user_id into get_memory_for_absorb so monolith fetches still enforce
// ownership.
async fn absorb_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<AbsorbRequest>,
) -> Result<Json<Value>, AppError> {
    require_brain(&state).await?;
    let brain = state
        .brain
        .as_ref()
        .ok_or_else(|| AppError(kleos_lib::EngError::Internal("brain not configured".into())))?;
    let embedder = state.current_embedder().await.ok_or_else(|| {
        AppError(kleos_lib::EngError::Internal(
            "embedder not ready (still loading)".into(),
        ))
    })?;
    let memory = get_memory_for_absorb(&db, body.id, auth.user_id).await?;
    brain.absorb(embedder.as_ref(), memory).await?;
    Ok(Json(json!({ "ok": true, "id": body.id })))
}

// dream / decay / evolution_train mutate shared brain state. H-R3-001 tracks
// gating these behind admin scope; for C-R3-001 we keep the existing surface
// but switch to Auth(auth) so future logging/audit can attribute the call.
async fn dream_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_brain(&state).await?;
    let brain = state
        .brain
        .as_ref()
        .ok_or_else(|| AppError(kleos_lib::EngError::Internal("brain not configured".into())))?;
    let result = brain.dream_cycle().await?;
    Ok(Json(json!({ "ok": true, "result": result })))
}

// C-R3-001: feedback verifies that every memory_id in the body is owned by
// the calling user before it influences the brain. Previously the helper
// only checked existence -- the name lied.
async fn feedback_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<FeedbackRequest>,
) -> Result<Json<Value>, AppError> {
    require_brain(&state).await?;

    let owned = verify_memory_ownership(&db, &body.memory_ids, auth.user_id).await?;
    if !owned {
        return Err(AppError(kleos_lib::EngError::Auth(
            "One or more memory_ids not found or not owned by you".into(),
        )));
    }

    let brain = state
        .brain
        .as_ref()
        .ok_or_else(|| AppError(kleos_lib::EngError::Internal("brain not configured".into())))?;
    let result = brain
        .feedback_signal(body.memory_ids, body.edge_pairs, body.useful)
        .await?;
    Ok(Json(json!({ "ok": true, "result": result })))
}

async fn decay_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Json(body): Json<DecayRequest>,
) -> Result<Json<Value>, AppError> {
    require_brain(&state).await?;
    let brain = state
        .brain
        .as_ref()
        .ok_or_else(|| AppError(kleos_lib::EngError::Internal("brain not configured".into())))?;
    brain.decay_tick(body.ticks).await?;
    Ok(Json(json!({ "ok": true })))
}

// C-R3-001: same ownership gate as feedback_handler.
async fn evolution_feedback_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<FeedbackRequest>,
) -> Result<Json<Value>, AppError> {
    require_brain(&state).await?;

    let owned = verify_memory_ownership(&db, &body.memory_ids, auth.user_id).await?;
    if !owned {
        return Err(AppError(kleos_lib::EngError::Auth(
            "One or more memory_ids not found or not owned by you".into(),
        )));
    }

    let brain = state
        .brain
        .as_ref()
        .ok_or_else(|| AppError(kleos_lib::EngError::Internal("brain not configured".into())))?;
    let result = brain
        .feedback_signal(body.memory_ids, body.edge_pairs, body.useful)
        .await?;
    Ok(Json(json!({ "ok": true, "result": result })))
}

async fn evolution_train_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_brain(&state).await?;
    let brain = state
        .brain
        .as_ref()
        .ok_or_else(|| AppError(kleos_lib::EngError::Internal("brain not configured".into())))?;
    let result = brain.evolution_train().await?;
    Ok(Json(json!({ "ok": true, "result": result })))
}

async fn evolution_stats_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_brain(&state).await?;
    let brain = state
        .brain
        .as_ref()
        .ok_or_else(|| AppError(kleos_lib::EngError::Internal("brain not configured".into())))?;
    let result = brain.evolution_stats().await?;
    Ok(Json(json!({ "ok": true, "result": result })))
}
