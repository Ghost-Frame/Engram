use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use engram_lib::services::brain::{
    get_memory_for_absorb, verify_memory_ownership, AbsorbRequest, BrainQueryOptions, DecayRequest,
    FeedbackRequest,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/brain/stats", get(stats_handler))
        .route("/brain/query", post(query_handler))
        .route("/brain/absorb", post(absorb_handler))
        .route("/brain/dream", post(dream_handler))
        .route("/brain/feedback", post(feedback_handler))
        .route("/brain/decay", post(decay_handler))
        .route("/brain/evolution/feedback", post(evolution_feedback_handler))
        .route("/brain/evolution/train", post(evolution_train_handler))
        .route("/brain/evolution/stats", get(evolution_stats_handler))
}

async fn require_brain(state: &AppState) -> Result<(), AppError> {
    if let Some(ref brain) = state.brain {
        if brain.is_ready() {
            return Ok(());
        }
    }
    Err(AppError(engram_lib::EngError::Internal(
        "brain not ready".into(),
    )))
}

async fn stats_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_brain(&state).await?;
    let brain = state.brain.as_ref().ok_or_else(|| {
        AppError(engram_lib::EngError::Internal(
            "brain not configured".into(),
        ))
    })?;
    let stats = brain.stats().await?;
    Ok(Json(json!({ "ok": true, "stats": stats })))
}

async fn query_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Json(body): Json<BrainQueryOptions>,
) -> Result<Json<Value>, AppError> {
    require_brain(&state).await?;
    let brain = state.brain.as_ref().ok_or_else(|| {
        AppError(engram_lib::EngError::Internal(
            "brain not configured".into(),
        ))
    })?;
    let embedder_guard = state.embedder.read().await;
    let embedder = embedder_guard.as_ref().ok_or_else(|| {
        AppError(engram_lib::EngError::Internal(
            "embedder not ready (still loading)".into(),
        ))
    })?;
    let result = brain.query(embedder.as_ref(), &body.query, &body).await?;
    Ok(Json(json!({ "ok": true, "result": result })))
}

async fn absorb_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<AbsorbRequest>,
) -> Result<Json<Value>, AppError> {
    require_brain(&state).await?;
    let brain = state.brain.as_ref().ok_or_else(|| {
        AppError(engram_lib::EngError::Internal(
            "brain not configured".into(),
        ))
    })?;
    let embedder_guard = state.embedder.read().await;
    let embedder = embedder_guard.as_ref().ok_or_else(|| {
        AppError(engram_lib::EngError::Internal(
            "embedder not ready (still loading)".into(),
        ))
    })?;
    let memory = get_memory_for_absorb(&state.db, body.id, auth.user_id).await?;
    brain.absorb(embedder.as_ref(), memory).await?;
    Ok(Json(json!({ "ok": true, "id": body.id })))
}

async fn dream_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_brain(&state).await?;
    let brain = state.brain.as_ref().ok_or_else(|| {
        AppError(engram_lib::EngError::Internal(
            "brain not configured".into(),
        ))
    })?;
    let result = brain.dream_cycle().await?;
    Ok(Json(json!({ "ok": true, "result": result })))
}

async fn feedback_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<FeedbackRequest>,
) -> Result<Json<Value>, AppError> {
    require_brain(&state).await?;

    // Verify memory ownership
    let owned = verify_memory_ownership(&state.db, &body.memory_ids, auth.user_id).await?;
    if !owned {
        return Err(AppError(engram_lib::EngError::Auth(
            "One or more memory_ids not found or not owned by you".into(),
        )));
    }

    let brain = state.brain.as_ref().ok_or_else(|| {
        AppError(engram_lib::EngError::Internal(
            "brain not configured".into(),
        ))
    })?;
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
    let brain = state.brain.as_ref().ok_or_else(|| {
        AppError(engram_lib::EngError::Internal(
            "brain not configured".into(),
        ))
    })?;
    brain.decay_tick(body.ticks).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn evolution_feedback_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<FeedbackRequest>,
) -> Result<Json<Value>, AppError> {
    require_brain(&state).await?;

    let owned = verify_memory_ownership(&state.db, &body.memory_ids, auth.user_id).await?;
    if !owned {
        return Err(AppError(engram_lib::EngError::Auth(
            "One or more memory_ids not found or not owned by you".into(),
        )));
    }

    let brain = state.brain.as_ref().ok_or_else(|| {
        AppError(engram_lib::EngError::Internal(
            "brain not configured".into(),
        ))
    })?;
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
    let brain = state.brain.as_ref().ok_or_else(|| {
        AppError(engram_lib::EngError::Internal(
            "brain not configured".into(),
        ))
    })?;
    let result = brain.evolution_train().await?;
    Ok(Json(json!({ "ok": true, "result": result })))
}

async fn evolution_stats_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_brain(&state).await?;
    let brain = state.brain.as_ref().ok_or_else(|| {
        AppError(engram_lib::EngError::Internal(
            "brain not configured".into(),
        ))
    })?;
    let result = brain.evolution_stats().await?;
    Ok(Json(json!({ "ok": true, "result": result })))
}
