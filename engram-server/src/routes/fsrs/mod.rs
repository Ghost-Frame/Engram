use axum::{
    extract::{Query, State},
    routing::{get, post},
    Json, Router,
};
use kleos_lib::fsrs;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

use crate::{error::AppError, extractors::Auth, state::AppState};

mod types;
use types::{ReviewBody, StateQuery};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/fsrs/review", post(review))
        .route("/fsrs/state", get(get_state))
        .route("/fsrs/init", post(init_backfill))
}

async fn review(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<ReviewBody>,
) -> Result<Json<Value>, AppError> {
    let id = body.id.or(body.memory_id).ok_or_else(|| {
        AppError(kleos_lib::EngError::InvalidInput(
            "id (or memory_id) required, grade 1-4".into(),
        ))
    })?;

    let grade_num = body.grade.unwrap_or(3);
    if !(1..=4).contains(&grade_num) {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
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

    // Verify memory belongs to user and fetch FSRS state
    let row_data = state
        .db
        .read(move |conn| {
            conn.query_row(
                "SELECT user_id, fsrs_stability, fsrs_difficulty, fsrs_storage_strength, fsrs_retrieval_strength, fsrs_learning_state, fsrs_reps, fsrs_lapses, fsrs_last_review_at, created_at FROM memories WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<f64>>(1)?,
                        row.get::<_, Option<f64>>(2)?,
                        row.get::<_, Option<f64>>(3)?,
                        row.get::<_, Option<f64>>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                        row.get::<_, Option<i64>>(6)?,
                        row.get::<_, Option<i64>>(7)?,
                        row.get::<_, Option<String>>(8)?,
                        row.get::<_, String>(9)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await?;

    let (
        owner,
        stability,
        difficulty,
        storage,
        retrieval,
        learning_state_int,
        reps,
        lapses,
        last_review,
        created_at,
    ) = row_data.ok_or_else(|| AppError(kleos_lib::EngError::NotFound("not found".into())))?;

    if owner != auth.user_id {
        return Err(AppError(kleos_lib::EngError::NotFound("not found".into())));
    }

    // Build current FSRS state if it exists
    let current_state = if let Some(s) = stability {
        let difficulty_val = difficulty.unwrap_or(5.0);
        let storage_val = storage.unwrap_or(1.0);
        let retrieval_val = retrieval.unwrap_or(1.0);
        let ls_int = learning_state_int.unwrap_or(0);
        let reps_val = reps.unwrap_or(0);
        let lapses_val = lapses.unwrap_or(0);
        let last_review_str = last_review.unwrap_or_default();

        let learning_state = match ls_int {
            1 => fsrs::LearningState::Learning,
            2 => fsrs::LearningState::Review,
            3 => fsrs::LearningState::Relearning,
            _ => fsrs::LearningState::New,
        };

        Some(fsrs::FsrsState {
            stability: s as f32,
            difficulty: difficulty_val as f32,
            storage_strength: storage_val as f32,
            retrieval_strength: retrieval_val as f32,
            learning_state,
            reps: reps_val as i32,
            lapses: lapses_val as i32,
            last_review_at: last_review_str,
        })
    } else {
        None
    };

    // Calculate elapsed days
    let last_review_str_for_elapsed = current_state
        .as_ref()
        .map(|s| s.last_review_at.clone())
        .unwrap_or_else(|| created_at.clone());
    let elapsed_days = calculate_elapsed_days(&last_review_str_for_elapsed);

    // Process review
    let new_state = fsrs::process_review(current_state.as_ref(), grade, elapsed_days);

    // Update the memory's FSRS columns
    let learning_state_int_new = new_state.learning_state as i64;
    let stability_new = new_state.stability as f64;
    let difficulty_new = new_state.difficulty as f64;
    let storage_strength_new = new_state.storage_strength as f64;
    let retrieval_strength_new = new_state.retrieval_strength as f64;
    let reps_new = new_state.reps as i64;
    let lapses_new = new_state.lapses as i64;
    let last_review_at_new = new_state.last_review_at.clone();

    state
        .db
        .write(move |conn| {
            conn.execute(
                "UPDATE memories SET fsrs_stability = ?1, fsrs_difficulty = ?2, fsrs_storage_strength = ?3, fsrs_retrieval_strength = ?4, fsrs_learning_state = ?5, fsrs_reps = ?6, fsrs_lapses = ?7, fsrs_last_review_at = ?8, access_count = access_count + 1, last_accessed_at = datetime('now') WHERE id = ?9",
                params![
                    stability_new,
                    difficulty_new,
                    storage_strength_new,
                    retrieval_strength_new,
                    learning_state_int_new,
                    reps_new,
                    lapses_new,
                    last_review_at_new,
                    id
                ],
            )
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
            Ok(())
        })
        .await?;

    Ok(Json(json!({ "id": id, "fsrs": new_state })))
}

async fn get_state(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<StateQuery>,
) -> Result<Json<Value>, AppError> {
    let id = params
        .id
        .ok_or_else(|| AppError(kleos_lib::EngError::InvalidInput("id required".into())))?;

    let row_data = state
        .db
        .read(move |conn| {
            conn.query_row(
                "SELECT user_id, fsrs_stability, fsrs_difficulty, fsrs_storage_strength, fsrs_retrieval_strength, fsrs_learning_state, fsrs_reps, fsrs_lapses, fsrs_last_review_at, created_at FROM memories WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<f64>>(1)?,
                        row.get::<_, Option<f64>>(2)?,
                        row.get::<_, Option<f64>>(3)?,
                        row.get::<_, Option<f64>>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, Option<String>>(8)?,
                        row.get::<_, String>(9)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await?;

    let (
        owner,
        stability,
        difficulty,
        storage_strength,
        retrieval_strength,
        learning_state,
        reps,
        lapses,
        last_review_at,
        created_at,
    ) = row_data.ok_or_else(|| AppError(kleos_lib::EngError::NotFound("not found".into())))?;

    if owner != auth.user_id {
        return Err(AppError(kleos_lib::EngError::NotFound("not found".into())));
    }

    // Calculate retrievability
    let ref_str = last_review_at.as_deref().unwrap_or(&created_at);
    let elapsed = calculate_elapsed_days(ref_str);
    let retrievability = stability.map(|s| fsrs::retrievability(s as f32, elapsed));
    let next_review = stability.map(|s| fsrs::next_interval(s as f32, 0.9));

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
    let ids: Vec<i64> = state
        .db
        .read(move |conn| {
            let mut stmt = conn
                .prepare("SELECT id FROM memories WHERE user_id = ?1 AND fsrs_stability IS NULL")
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
            let mut rows = stmt
                .query(params![auth.user_id])
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
            let mut results = Vec::new();
            while let Some(row) = rows
                .next()
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?
            {
                let id: i64 = row
                    .get(0)
                    .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
                results.push(id);
            }
            Ok(results)
        })
        .await?;

    let count = ids.len() as i64;

    state
        .db
        .write(move |conn| {
            for id in &ids {
                let init = fsrs::process_review(None, fsrs::Rating::Good, 0.0);
                let learning_state_int = init.learning_state as i64;
                conn.execute(
                    "UPDATE memories SET fsrs_stability = ?1, fsrs_difficulty = ?2, fsrs_storage_strength = ?3, fsrs_retrieval_strength = ?4, fsrs_learning_state = ?5, fsrs_reps = ?6, fsrs_lapses = ?7, fsrs_last_review_at = ?8 WHERE id = ?9",
                    params![
                        init.stability as f64,
                        init.difficulty as f64,
                        init.storage_strength as f64,
                        init.retrieval_strength as f64,
                        learning_state_int,
                        init.reps as i64,
                        init.lapses as i64,
                        init.last_review_at,
                        *id
                    ],
                )
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
            }
            Ok(())
        })
        .await?;

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
