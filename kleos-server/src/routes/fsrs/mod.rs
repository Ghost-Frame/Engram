use axum::{
    extract::{Query, State},
    routing::{get, post},
    Json, Router,
};
use kleos_lib::fsrs;
use kleos_lib::memory::search::hybrid_search;
use kleos_lib::memory::types::SearchRequest;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

use crate::{
    error::AppError,
    extractors::{Auth, ResolvedDb},
    state::AppState,
};
use kleos_lib::auth::Scope;

mod types;
use types::{RecallDueQuery, ReviewBody, StateQuery};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/fsrs/review", post(review))
        .route("/fsrs/state", get(get_state))
        .route("/fsrs/init", post(init_backfill))
        .route("/fsrs/recall-due", get(recall_due))
}

// SECURITY: relies on ResolvedDb shard isolation (Phase 5+) to scope to the caller's tenant. Do not add state.db calls here without re-binding auth.
async fn review(
    Auth(_auth): Auth,
    ResolvedDb(db): ResolvedDb,
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
    // R8-R-005: grade_num is clamped to 1..=4 above; map by value so a
    // future refactor of the guard cannot turn a bad grade into a panic.
    let grade = match grade_num {
        1 => fsrs::Rating::Again,
        2 => fsrs::Rating::Hard,
        4 => fsrs::Rating::Easy,
        _ => fsrs::Rating::Good,
    };

    // Fetch FSRS state for the memory. Tenant isolation comes from ResolvedDb
    // (Phase 5+: each shard contains only one tenant's memories). On the
    // legacy monolith path the same ResolvedDb fallback applies; user_id was
    // dropped from memories in Phase 5.1.
    let row_data = db
        .read(move |conn| {
            conn.query_row(
                "SELECT fsrs_stability, fsrs_difficulty, fsrs_storage_strength, fsrs_retrieval_strength, fsrs_learning_state, fsrs_reps, fsrs_lapses, fsrs_last_review_at, created_at FROM memories WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, Option<f64>>(0)?,
                        row.get::<_, Option<f64>>(1)?,
                        row.get::<_, Option<f64>>(2)?,
                        row.get::<_, Option<f64>>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                        row.get::<_, Option<i64>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, String>(8)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await?;

    let (
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

    db
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

// SECURITY: relies on ResolvedDb shard isolation (Phase 5+) to scope to the caller's tenant. Do not add state.db calls here without re-binding auth.
async fn get_state(
    Auth(_auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<StateQuery>,
) -> Result<Json<Value>, AppError> {
    let id = params
        .id
        .ok_or_else(|| AppError(kleos_lib::EngError::InvalidInput("id required".into())))?;

    let row_data = db
        .read(move |conn| {
            conn.query_row(
                "SELECT fsrs_stability, fsrs_difficulty, fsrs_storage_strength, fsrs_retrieval_strength, fsrs_learning_state, fsrs_reps, fsrs_lapses, fsrs_last_review_at, created_at FROM memories WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, Option<f64>>(0)?,
                        row.get::<_, Option<f64>>(1)?,
                        row.get::<_, Option<f64>>(2)?,
                        row.get::<_, Option<f64>>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, String>(8)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await?;

    let (
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
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
) -> Result<Json<Value>, AppError> {
    // SECURITY: init_backfill mass-mutates every memory's FSRS columns. Gate
    // behind Admin scope so a regular write-scoped key cannot wipe everyone's
    // spaced-repetition schedule with one POST.
    if !auth.has_scope(&Scope::Admin) {
        return Err(AppError(kleos_lib::EngError::Auth(
            "admin scope required for /fsrs/init".into(),
        )));
    }

    // Find memories without FSRS state. ResolvedDb scopes us to the caller's
    // tenant shard (Phase 5+), so the SELECT/UPDATE only touches that user's
    // rows.
    let ids: Vec<i64> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare("SELECT id FROM memories WHERE fsrs_stability IS NULL")
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
            let mut rows = stmt
                .query(params![])
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

    db
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

async fn recall_due(
    State(state): State<AppState>,
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<RecallDueQuery>,
) -> Result<Json<Value>, AppError> {
    let embedding = if let Some(embedder) = state.current_embedder().await {
        match embedder.embed(&params.topic).await {
            Ok(emb) => Some(emb),
            Err(e) => {
                tracing::warn!("embedding failed for recall-due: {}", e);
                None
            }
        }
    } else {
        None
    };

    let search_limit = params.limit.clamp(1, 100);
    let fetch_limit = (search_limit * 3).min(100);

    let req = SearchRequest {
        query: params.topic.clone(),
        embedding,
        limit: Some(fetch_limit),
        category: None,
        source: params.session.clone(),
        tags: None,
        threshold: None,
        user_id: Some(auth.user_id),
        space_id: None,
        include_forgotten: Some(false),
        mode: None,
        question_type: None,
        expand_relationships: false,
        include_links: false,
        latest_only: true,
        source_filter: None,
    };

    let arc_results = hybrid_search(&db, req).await?;
    let entries = fsrs::recall::rerank_by_retrievability(&arc_results, None);

    let items: Vec<Value> = entries
        .into_iter()
        .take(search_limit)
        .map(|e| {
            json!({
                "memory_id": e.memory_id,
                "content": e.content,
                "retrievability": e.retrievability,
                "original_score": e.original_score,
                "recall_due_score": e.recall_due_score,
            })
        })
        .collect();

    Ok(Json(json!({
        "topic": params.topic,
        "count": items.len(),
        "results": items,
    })))
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
