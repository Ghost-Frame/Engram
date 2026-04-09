use axum::{
    extract::{Query, State},
    routing::{get, post},
    Json, Router,
};
use engram_lib::fsrs;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{error::AppError, extractors::Auth, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/fsrs/review", post(review))
        .route("/fsrs/state", get(get_state))
        .route("/fsrs/init", post(init_backfill))
}

#[derive(Debug, Deserialize)]
struct ReviewBody {
    pub id: Option<i64>,
    pub memory_id: Option<i64>,
    pub grade: Option<u8>,
}

async fn review(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<ReviewBody>,
) -> Result<Json<Value>, AppError> {
    let id = body
        .id
        .or(body.memory_id)
        .ok_or_else(|| {
            AppError(engram_lib::EngError::InvalidInput(
                "id (or memory_id) required, grade 1-4".into(),
            ))
        })?;

    let grade_num = body.grade.unwrap_or(3);
    if !(1..=4).contains(&grade_num) {
        return Err(AppError(engram_lib::EngError::InvalidInput(
            "grade must be 1-4".into(),
        )));
    }
    let grade = match grade_num {
        1 => fsrs::Rating::Again,
        2 => fsrs::Rating::Hard,
        3 => fsrs::Rating::Good,
        4 => fsrs::Rating::Easy,
        _ => unreachable!(),
    };

    // Verify memory belongs to user
    let mut rows = state
        .db
        .conn
        .query(
            "SELECT user_id, fsrs_stability, fsrs_difficulty, fsrs_storage_strength, fsrs_retrieval_strength, fsrs_learning_state, fsrs_reps, fsrs_lapses, fsrs_last_review_at, created_at FROM memories WHERE id = ?1",
            libsql::params![id],
        )
        .await
        .map_err(engram_lib::EngError::Database)?;

    let row = rows
        .next()
        .await
        .map_err(engram_lib::EngError::Database)?
        .ok_or_else(|| AppError(engram_lib::EngError::NotFound("not found".into())))?;

    let owner: i64 = row
        .get(0)
        .map_err(|e| engram_lib::EngError::Internal(e.to_string()))?;
    if owner != auth.user_id {
        return Err(AppError(engram_lib::EngError::NotFound("not found".into())));
    }

    // Build current FSRS state if it exists
    let stability: Option<f64> = row.get(1).unwrap_or(None);
    let current_state = if let Some(s) = stability {
        let difficulty: f64 = row.get::<f64>(2).unwrap_or(5.0);
        let storage: f64 = row.get::<f64>(3).unwrap_or(1.0);
        let retrieval: f64 = row.get::<f64>(4).unwrap_or(1.0);
        let learning_state_int: i64 = row.get::<i64>(5).unwrap_or(0);
        let reps: i64 = row.get::<i64>(6).unwrap_or(0);
        let lapses: i64 = row.get::<i64>(7).unwrap_or(0);
        let last_review: String = row.get::<String>(8).unwrap_or_default();

        let learning_state = match learning_state_int {
            1 => fsrs::LearningState::Learning,
            2 => fsrs::LearningState::Review,
            3 => fsrs::LearningState::Relearning,
            _ => fsrs::LearningState::New,
        };

        Some(fsrs::FsrsState {
            stability: s as f32,
            difficulty: difficulty as f32,
            storage_strength: storage as f32,
            retrieval_strength: retrieval as f32,
            learning_state,
            reps: reps as i32,
            lapses: lapses as i32,
            last_review_at: last_review,
        })
    } else {
        None
    };

    // Calculate elapsed days
    let created_at: String = row.get::<String>(9).unwrap_or_default();
    let last_review_str = current_state
        .as_ref()
        .map(|s| s.last_review_at.as_str())
        .unwrap_or(&created_at);
    let elapsed_days = calculate_elapsed_days(last_review_str);

    // Process review
    let new_state = fsrs::process_review(current_state.as_ref(), grade, elapsed_days);

    // Update the memory's FSRS columns
    let learning_state_int = new_state.learning_state as i64;
    state
        .db
        .conn
        .execute(
            "UPDATE memories SET fsrs_stability = ?1, fsrs_difficulty = ?2, fsrs_storage_strength = ?3, fsrs_retrieval_strength = ?4, fsrs_learning_state = ?5, fsrs_reps = ?6, fsrs_lapses = ?7, fsrs_last_review_at = ?8, access_count = access_count + 1, last_accessed_at = datetime('now') WHERE id = ?9",
            libsql::params![
                new_state.stability as f64,
                new_state.difficulty as f64,
                new_state.storage_strength as f64,
                new_state.retrieval_strength as f64,
                learning_state_int,
                new_state.reps as i64,
                new_state.lapses as i64,
                new_state.last_review_at.clone(),
                id
            ],
        )
        .await
        .map_err(engram_lib::EngError::Database)?;

    Ok(Json(json!({ "id": id, "fsrs": new_state })))
}

#[derive(Debug, Deserialize)]
struct StateQuery {
    pub id: Option<i64>,
}

async fn get_state(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<StateQuery>,
) -> Result<Json<Value>, AppError> {
    let id = params.id.ok_or_else(|| {
        AppError(engram_lib::EngError::InvalidInput("id required".into()))
    })?;

    let mut rows = state
        .db
        .conn
        .query(
            "SELECT user_id, fsrs_stability, fsrs_difficulty, fsrs_storage_strength, fsrs_retrieval_strength, fsrs_learning_state, fsrs_reps, fsrs_lapses, fsrs_last_review_at, created_at FROM memories WHERE id = ?1",
            libsql::params![id],
        )
        .await
        .map_err(engram_lib::EngError::Database)?;

    let row = rows
        .next()
        .await
        .map_err(engram_lib::EngError::Database)?
        .ok_or_else(|| AppError(engram_lib::EngError::NotFound("not found".into())))?;

    let owner: i64 = row
        .get(0)
        .map_err(|e| engram_lib::EngError::Internal(e.to_string()))?;
    if owner != auth.user_id {
        return Err(AppError(engram_lib::EngError::NotFound("not found".into())));
    }

    let stability: Option<f64> = row.get(1).unwrap_or(None);
    let difficulty: Option<f64> = row.get(2).unwrap_or(None);
    let storage_strength: Option<f64> = row.get(3).unwrap_or(None);
    let retrieval_strength: Option<f64> = row.get(4).unwrap_or(None);
    let learning_state: i64 = row.get::<i64>(5).unwrap_or(0);
    let reps: i64 = row.get::<i64>(6).unwrap_or(0);
    let lapses: i64 = row.get::<i64>(7).unwrap_or(0);
    let last_review_at: Option<String> = row.get(8).unwrap_or(None);
    let created_at: String = row.get::<String>(9).unwrap_or_default();

    // Calculate retrievability
    let ref_str = last_review_at.as_deref().unwrap_or(&created_at);
    let elapsed = calculate_elapsed_days(ref_str);
    let retrievability = stability
        .map(|s| fsrs::retrievability(s as f32, elapsed));
    let next_review = stability
        .map(|s| fsrs::next_interval(s as f32, 0.9));

    Ok(Json(json!({
        "id": id,
        "retrievability": retrievability,
        "next_review_days": next_review,
        "fsrs_stability": stability,
        "fsrs_difficulty": difficulty,
        "fsrs_storage_strength": storage_strength,
        "fsrs_retrieval_strength": retrieval_strength,
        "fsrs_learning_state": learning_state,
        "fsrs_reps": reps,
        "fsrs_lapses": lapses,
        "fsrs_last_review_at": last_review_at,
        "created_at": created_at,
    })))
}

async fn init_backfill(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    // Find memories without FSRS state
    let mut rows = state
        .db
        .conn
        .query(
            "SELECT id FROM memories WHERE user_id = ?1 AND fsrs_stability IS NULL",
            libsql::params![auth.user_id],
        )
        .await
        .map_err(engram_lib::EngError::Database)?;

    let mut ids = Vec::new();
    while let Some(r) = rows.next().await.map_err(engram_lib::EngError::Database)? {
        let id: i64 = r
            .get(0)
            .map_err(|e| engram_lib::EngError::Internal(e.to_string()))?;
        ids.push(id);
    }

    let mut count = 0i64;
    for id in &ids {
        let init = fsrs::process_review(None, fsrs::Rating::Good, 0.0);
        let learning_state_int = init.learning_state as i64;
        state
            .db
            .conn
            .execute(
                "UPDATE memories SET fsrs_stability = ?1, fsrs_difficulty = ?2, fsrs_storage_strength = ?3, fsrs_retrieval_strength = ?4, fsrs_learning_state = ?5, fsrs_reps = ?6, fsrs_lapses = ?7, fsrs_last_review_at = ?8 WHERE id = ?9",
                libsql::params![
                    init.stability as f64,
                    init.difficulty as f64,
                    init.storage_strength as f64,
                    init.retrieval_strength as f64,
                    learning_state_int,
                    init.reps as i64,
                    init.lapses as i64,
                    init.last_review_at.clone(),
                    *id
                ],
            )
            .await
            .map_err(engram_lib::EngError::Database)?;
        count += 1;
    }

    Ok(Json(json!({ "initialized": count })))
}

fn calculate_elapsed_days(date_str: &str) -> f32 {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let normalized = if date_str.contains('Z') {
        date_str.to_string()
    } else {
        format!("{}Z", date_str.replace(' ', "T"))
    };
    let ref_ms = normalized
        .parse::<chrono::DateTime<chrono::Utc>>()
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(now_ms);
    ((now_ms - ref_ms) as f32) / (1000.0 * 60.0 * 60.0 * 24.0)
}
