use axum::{
    extract::{Query, State},
    routing::{get, post},
    Json, Router,
};
use engram_lib::fsrs::decay;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{error::AppError, extractors::Auth, state::AppState};

/// Search-adjacent routes: decay refresh and decay scores.
/// Core search and recall are already in memory.rs.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/decay/refresh", post(refresh_decay))
        .route("/decay/scores", get(get_decay_scores))
}

async fn refresh_decay(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    // Recalculate decay scores for all non-static memories
    let mut rows = state
        .db
        .conn
        .query(
            "SELECT id, importance, created_at, access_count, last_accessed_at, is_static, source_count, fsrs_stability FROM memories WHERE user_id = ?1 AND is_static = 0 AND is_forgotten = 0",
            libsql::params![auth.user_id],
        )
        .await
        .map_err(engram_lib::EngError::Database)?;

    let mut updates = Vec::new();
    while let Some(r) = rows.next().await.map_err(engram_lib::EngError::Database)? {
        let id: i64 = r.get(0).map_err(|e| engram_lib::EngError::Internal(e.to_string()))?;
        let importance: f64 = r.get::<f64>(1).unwrap_or(5.0);
        let created_at: String = r.get::<String>(2).unwrap_or_default();
        let access_count: i64 = r.get::<i64>(3).unwrap_or(0);
        let last_accessed_at: Option<String> = r.get(4).unwrap_or(None);
        let is_static: bool = r.get::<i64>(5).unwrap_or(0) != 0;
        let source_count: i64 = r.get::<i64>(6).unwrap_or(1);
        let stability: Option<f64> = r.get(7).unwrap_or(None);

        let score = decay::calculate_decay_score(
            importance as f32,
            &created_at,
            access_count as i32,
            last_accessed_at.as_deref(),
            is_static,
            source_count as i32,
            stability.map(|s| s as f32),
        );

        updates.push((id, score as f64));
    }

    let count = updates.len();
    for (id, score) in &updates {
        state
            .db
            .conn
            .execute(
                "UPDATE memories SET decay_score = ?1 WHERE id = ?2",
                libsql::params![*score, *id],
            )
            .await
            .map_err(engram_lib::EngError::Database)?;
    }

    Ok(Json(json!({ "refreshed": count })))
}

#[derive(Debug, Deserialize)]
struct DecayScoresQuery {
    pub limit: Option<usize>,
    pub order: Option<String>,
}

async fn get_decay_scores(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<DecayScoresQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(20).min(100);
    let order_asc = params.order.as_deref() == Some("asc");

    let sql = if order_asc {
        "SELECT id, content, category, importance, decay_score, created_at FROM memories WHERE user_id = ?1 AND is_forgotten = 0 ORDER BY decay_score ASC LIMIT ?2"
    } else {
        "SELECT id, content, category, importance, decay_score, created_at FROM memories WHERE user_id = ?1 AND is_forgotten = 0 ORDER BY decay_score DESC LIMIT ?2"
    };

    let mut rows = state
        .db
        .conn
        .query(sql, libsql::params![auth.user_id, limit as i64])
        .await
        .map_err(engram_lib::EngError::Database)?;

    let mut memories = Vec::new();
    while let Some(r) = rows.next().await.map_err(engram_lib::EngError::Database)? {
        memories.push(json!({
            "id": r.get::<i64>(0).unwrap_or(0),
            "content": r.get::<String>(1).unwrap_or_default(),
            "category": r.get::<Option<String>>(2).unwrap_or(None),
            "importance": r.get::<f64>(3).unwrap_or(5.0),
            "decay_score": r.get::<Option<f64>>(4).unwrap_or(None),
            "created_at": r.get::<String>(5).unwrap_or_default(),
        }));
    }

    Ok(Json(json!({ "memories": memories })))
}
