use axum::{
    extract::{Query, State},
    routing::{get, post},
    Json, Router,
};
use engram_lib::fsrs::decay;
use rusqlite::params;
use serde_json::{json, Value};

use crate::{error::AppError, extractors::Auth, state::AppState};

mod types;
use types::DecayScoresQuery;

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
    let user_id = auth.user_id;
    let updates: Vec<(i64, f64)> = state
        .db
        .read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, importance, created_at, access_count, last_accessed_at, is_static, source_count, fsrs_stability \
                 FROM memories WHERE user_id = ?1 AND is_static = 0 AND is_forgotten = 0",
            )?;
            let rows = stmt.query_map(params![user_id], |r| {
                let id: i64 = r.get(0)?;
                let importance: f64 = r.get::<_, Option<f64>>(1)?.unwrap_or(5.0);
                let created_at: String = r.get::<_, Option<String>>(2)?.unwrap_or_default();
                let access_count: i64 = r.get::<_, Option<i64>>(3)?.unwrap_or(0);
                let last_accessed_at: Option<String> = r.get(4)?;
                let is_static: bool = r.get::<_, Option<i64>>(5)?.unwrap_or(0) != 0;
                let source_count: i64 = r.get::<_, Option<i64>>(6)?.unwrap_or(1);
                let stability: Option<f64> = r.get(7)?;
                Ok((id, importance, created_at, access_count, last_accessed_at, is_static, source_count, stability))
            })?;
            let mut result = Vec::new();
            for row in rows {
                let (id, importance, created_at, access_count, last_accessed_at, is_static, source_count, stability) = row?;
                let score = decay::calculate_decay_score(
                    importance as f32,
                    &created_at,
                    access_count as i32,
                    last_accessed_at.as_deref(),
                    is_static,
                    source_count as i32,
                    stability.map(|s| s as f32),
                );
                result.push((id, score as f64));
            }
            Ok(result)
        })
        .await?;

    let count = updates.len();
    state
        .db
        .write(move |conn| {
            for (id, score) in &updates {
                conn.execute(
                    "UPDATE memories SET decay_score = ?1 WHERE id = ?2",
                    params![*score, *id],
                )?;
            }
            Ok(())
        })
        .await?;

    Ok(Json(json!({ "refreshed": count })))
}

async fn get_decay_scores(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(query_params): Query<DecayScoresQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = query_params.limit.unwrap_or(20).min(100) as i64;
    let order_asc = query_params.order.as_deref() == Some("asc");
    let user_id = auth.user_id;

    let memories: Vec<Value> = state
        .db
        .read(move |conn| {
            let sql = if order_asc {
                "SELECT id, content, category, importance, decay_score, created_at \
                 FROM memories WHERE user_id = ?1 AND is_forgotten = 0 \
                 ORDER BY decay_score ASC LIMIT ?2"
            } else {
                "SELECT id, content, category, importance, decay_score, created_at \
                 FROM memories WHERE user_id = ?1 AND is_forgotten = 0 \
                 ORDER BY decay_score DESC LIMIT ?2"
            };
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map(params![user_id, limit], |r| {
                let id: i64 = r.get(0)?;
                let content: String = r.get::<_, Option<String>>(1)?.unwrap_or_default();
                let category: Option<String> = r.get(2)?;
                let importance: f64 = r.get::<_, Option<f64>>(3)?.unwrap_or(5.0);
                let decay_score: Option<f64> = r.get(4)?;
                let created_at: String = r.get::<_, Option<String>>(5)?.unwrap_or_default();
                Ok((id, content, category, importance, decay_score, created_at))
            })?;
            let mut memories = Vec::new();
            for row in rows {
                let (id, content, category, importance, decay_score, created_at) = row?;
                memories.push(json!({
                    "id": id,
                    "content": content,
                    "category": category,
                    "importance": importance,
                    "decay_score": decay_score,
                    "created_at": created_at,
                }));
            }
            Ok(memories)
        })
        .await?;

    Ok(Json(json!({ "memories": memories })))
}
