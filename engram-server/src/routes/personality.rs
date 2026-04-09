use axum::{
    extract::{Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use engram_lib::personality::{
    detect_signals, get_profile, list_signals, store_signal, update_profile,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{error::AppError, extractors::Auth, state::AppState};

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/personality/detect", post(detect_handler))
        .route(
            "/personality/signals",
            post(store_signal_handler).get(list_signals_handler),
        )
        .route("/personality/profile", get(get_profile_handler))
        .route("/personality/profile/update", post(update_profile_handler))
}

// ---------------------------------------------------------------------------
// Body / query structs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct DetectBody {
    content: String,
}

#[derive(Debug, Deserialize)]
struct StoreSignalBody {
    signal_type: String,
    value: f64,
    evidence: Option<String>,
    agent: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListSignalsParams {
    limit: Option<usize>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /personality/detect
/// Detect personality signals from text content. No DB write.
async fn detect_handler(
    Auth(_auth): Auth,
    Json(body): Json<DetectBody>,
) -> Result<Json<Value>, AppError> {
    let signals = detect_signals(&body.content);
    let pairs: Vec<Value> = signals
        .into_iter()
        .map(|(t, v)| json!({ "signal_type": t, "value": v }))
        .collect();
    Ok(Json(json!({ "signals": pairs })))
}

/// POST /personality/signals
/// Store a single personality signal for the authenticated user.
async fn store_signal_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<StoreSignalBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let signal = store_signal(
        &state.db,
        &body.signal_type,
        body.value,
        body.evidence.as_deref(),
        auth.user_id,
        body.agent.as_deref(),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(json!(signal))))
}

/// GET /personality/signals
/// List recent personality signals for the authenticated user.
async fn list_signals_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ListSignalsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(50);
    let signals = list_signals(&state.db, auth.user_id, limit).await?;
    Ok(Json(json!({ "signals": signals, "count": signals.len() })))
}

/// GET /personality/profile
/// Get (or create) the personality profile for the authenticated user.
async fn get_profile_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let profile = get_profile(&state.db, auth.user_id).await?;
    Ok(Json(json!(profile)))
}

/// POST /personality/profile/update
/// Aggregate stored signals into the profile and return the updated profile.
async fn update_profile_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let profile = update_profile(&state.db, auth.user_id).await?;
    Ok(Json(json!(profile)))
}
