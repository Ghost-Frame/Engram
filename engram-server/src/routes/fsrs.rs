use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::str::FromStr;

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use engram_lib::fsrs::{self, FsrsState, Rating};
use engram_lib::EngError;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/fsrs/review", post(review_handler))
        .route("/fsrs/retrievability", get(retrievability_handler))
        .route("/fsrs/next-interval", get(next_interval_handler))
        .route("/fsrs/stats", get(stats_handler))
}

#[derive(Debug, Deserialize)]
struct ReviewBody {
    state: Option<FsrsState>,
    rating: String,
    elapsed_days: Option<f32>,
}

#[derive(Debug, Deserialize)]
struct RetrievabilityQuery {
    stability: f32,
    elapsed_days: f32,
}

#[derive(Debug, Deserialize)]
struct NextIntervalQuery {
    stability: f32,
    desired_retention: Option<f32>,
}

async fn review_handler(
    Auth(_auth): Auth,
    Json(body): Json<ReviewBody>,
) -> Result<Json<Value>, AppError> {
    let rating = Rating::from_str(&body.rating)
        .map_err(engram_lib::EngError::InvalidInput)
        .map_err(AppError)?;
    let elapsed_days = body.elapsed_days.unwrap_or(0.0);
    let next = fsrs::process_review(body.state.as_ref(), rating, elapsed_days);
    let interval_days = fsrs::next_interval(next.stability, 0.9);
    Ok(Json(json!({
        "state": next,
        "interval_days": interval_days,
    })))
}

async fn retrievability_handler(
    Auth(_auth): Auth,
    Query(query): Query<RetrievabilityQuery>,
) -> Result<Json<Value>, AppError> {
    let value = fsrs::retrievability(query.stability, query.elapsed_days);
    Ok(Json(json!({ "retrievability": value })))
}

async fn next_interval_handler(
    Auth(_auth): Auth,
    Query(query): Query<NextIntervalQuery>,
) -> Result<Json<Value>, AppError> {
    let retention = query.desired_retention.unwrap_or(0.9);
    let interval = fsrs::next_interval(query.stability, retention);
    Ok(Json(json!({ "interval_days": interval })))
}

async fn stats_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let mut rows = state
        .db
        .conn
        .query(
            "SELECT
                COUNT(*) as total,
                COALESCE(SUM(CASE WHEN fsrs_stability IS NOT NULL THEN 1 ELSE 0 END), 0) as with_fsrs,
                COALESCE(AVG(fsrs_stability), 0),
                COALESCE(AVG(fsrs_difficulty), 0)
             FROM memories
             WHERE user_id = ?1",
            libsql::params![auth.user_id],
        )
        .await
        .map_err(EngError::Database)?;
    let row = rows
        .next()
        .await
        .map_err(EngError::Database)?
        .ok_or_else(|| EngError::Internal("no fsrs stats row".to_string()))?;
    Ok(Json(json!({
        "total_memories": row.get::<i64>(0).map_err(EngError::Database)?,
        "with_fsrs": row.get::<i64>(1).map_err(EngError::Database)?,
        "avg_stability": row.get::<f64>(2).map_err(EngError::Database)?,
        "avg_difficulty": row.get::<f64>(3).map_err(EngError::Database)?,
    })))
}
