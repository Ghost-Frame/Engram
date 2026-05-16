//! Structural analysis routes: EN-syntax topology, node roles, bridges,
//! betweenness, distance, and trace. Mirrors the legacy Engram MCP
//! `structural_*` surface as native Kleos endpoints.

use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use kleos_lib::services::structural;

/// Build the structural router. All endpoints are read-only against the
/// posted `source` so no DB extractor is needed.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/structural/analyze", post(analyze_handler))
        .route("/structural/detail", post(detail_handler))
        .route("/structural/between", post(between_handler))
        .route("/structural/distance", post(distance_handler))
        .route("/structural/trace", post(trace_handler))
}

#[derive(Debug, Deserialize)]
struct SourceBody {
    source: String,
}

#[derive(Debug, Deserialize)]
struct BetweenBody {
    source: String,
    node: String,
}

#[derive(Debug, Deserialize)]
struct PathBody {
    source: String,
    from: String,
    to: String,
}

fn require_source(s: &str) -> Result<(), AppError> {
    if s.trim().is_empty() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "source must not be empty".into(),
        )));
    }
    Ok(())
}

async fn analyze_handler(
    Auth(_auth): Auth,
    Json(body): Json<SourceBody>,
) -> Result<Json<Value>, AppError> {
    require_source(&body.source)?;
    let report = structural::analyze_source(&body.source);
    Ok(Json(json!(report)))
}

async fn detail_handler(
    Auth(_auth): Auth,
    Json(body): Json<SourceBody>,
) -> Result<Json<Value>, AppError> {
    require_source(&body.source)?;
    let report = structural::detail_source(&body.source);
    Ok(Json(json!(report)))
}

async fn between_handler(
    Auth(_auth): Auth,
    Json(body): Json<BetweenBody>,
) -> Result<Json<Value>, AppError> {
    require_source(&body.source)?;
    let score = structural::node_betweenness_in_source(&body.source, &body.node)?;
    Ok(Json(json!({ "node": body.node, "betweenness": score })))
}

async fn distance_handler(
    Auth(_auth): Auth,
    Json(body): Json<PathBody>,
) -> Result<Json<Value>, AppError> {
    require_source(&body.source)?;
    let report = structural::distance_in_source(&body.source, &body.from, &body.to)?;
    Ok(Json(json!(report)))
}

async fn trace_handler(
    Auth(_auth): Auth,
    Json(body): Json<PathBody>,
) -> Result<Json<Value>, AppError> {
    require_source(&body.source)?;
    let report = structural::trace_in_source(&body.source, &body.from, &body.to)?;
    Ok(Json(json!(report)))
}
