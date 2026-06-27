use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use kleos_lib::intelligence::{
    causal::{add_link, backward_chain, create_chain, get_chain, list_chains},
    consolidation::{consolidate, find_consolidation_candidates, list_consolidations, sweep},
    contradiction::{detect_contradictions, scan_all_contradictions},
    correction::correct_memory,
    decomposition::decompose,
    digests::{generate_digest, list_digests},
    duplicates::{deduplicate, find_duplicates},
    extraction::fast_extract_facts,
    feedback,
    health::memory_health,
    predictive::{detect_sequence_patterns, predictive_recall},
    reconsolidation::{reconsolidate_memory, run_reconsolidation_sweep},
    reflections::{
        create_reflection, generate_reflections_with_llm, list_reflections, LlmReflector,
    },
    scheduler::default_pipeline,
    sentiment,
    temporal::{detect_patterns, list_patterns, store_pattern, time_travel},
    types::FeedbackRequest,
    valence::{analyze_valence, get_emotional_profile, store_valence},
};
use kleos_lib::memory;
use serde_json::{json, Value};

use rusqlite::params;

use crate::{
    dreamer::active_user_ids,
    error::AppError,
    extractors::{Auth, ResolvedDb},
    state::AppState,
};

mod types;
use types::{
    AddLinkBody, BackwardBody, CandidatesBody, ConsolidateBody, CorrectBody, CreateChainBody,
    CreateReflectionBody, DeduplicateBody, DigestBody, DuplicatesQuery, ExtractBody, LimitQuery,
    ReconsolidationCandidatesQuery, SentimentAnalyzeBody, SentimentHistoryQuery, SequencesBody,
    SweepBody, TimeTravelBody, ValenceScoreBody,
};

// --- Router ---

pub fn router() -> Router<AppState> {
    Router::new()
        // Consolidation (with root-level alias for parity with original kleos)
        .route("/consolidate", post(consolidate_handler))
        .route("/intelligence/consolidate", post(consolidate_handler))
        .route(
            "/intelligence/consolidation-candidates",
            post(candidates_handler),
        )
        .route(
            "/intelligence/consolidations",
            get(list_consolidations_handler),
        )
        // Contradiction (with root-level alias for parity)
        .route("/contradictions/{memory_id}", get(contradictions_handler))
        .route("/contradictions", post(scan_contradictions_handler))
        .route(
            "/intelligence/contradictions/{memory_id}",
            get(contradictions_handler),
        )
        .route(
            "/intelligence/contradictions",
            post(scan_contradictions_handler),
        )
        // Decomposition
        .route(
            "/intelligence/decompose/{memory_id}",
            post(decompose_handler),
        )
        // Temporal
        .route(
            "/intelligence/temporal/detect",
            post(detect_temporal_handler),
        )
        .route(
            "/intelligence/temporal/patterns",
            get(list_temporal_handler),
        )
        // Digests (with root-level alias for parity)
        .route("/digests/generate", post(generate_digest_handler))
        .route("/digests", get(list_digests_handler))
        .route(
            "/intelligence/digests/generate",
            post(generate_digest_handler),
        )
        .route("/intelligence/digests", get(list_digests_handler))
        // Reflections (with root-level alias for parity)
        .route(
            "/reflections",
            post(create_reflection_handler).get(list_reflections_handler),
        )
        .route("/reflect", post(create_reflection_handler))
        .route(
            "/intelligence/reflections",
            post(create_reflection_handler).get(list_reflections_handler),
        )
        .route("/reflections/generate", post(generate_reflections_handler))
        .route(
            "/intelligence/reflections/generate",
            post(generate_reflections_handler),
        )
        // Causal
        .route(
            "/intelligence/causal/chains",
            post(create_chain_handler).get(list_chains_handler),
        )
        .route("/intelligence/causal/chains/{id}", get(get_chain_handler))
        .route("/intelligence/causal/links", post(add_link_handler))
        .route(
            "/intelligence/causal/backward/{memory_id}",
            post(causal_backward_handler),
        )
        // -- NEW: Sentiment
        .route(
            "/intelligence/sentiment/analyze",
            post(sentiment_analyze_handler),
        )
        .route(
            "/intelligence/sentiment/history",
            get(sentiment_history_handler),
        )
        // -- NEW: Valence
        .route("/intelligence/valence/score", post(valence_score_handler))
        .route(
            "/intelligence/valence/{memory_id}",
            get(valence_get_handler),
        )
        .route(
            "/intelligence/valence/profile",
            get(valence_profile_handler),
        )
        // -- NEW: Predictive
        .route(
            "/intelligence/predictive/recall",
            post(predictive_recall_handler),
        )
        .route(
            "/intelligence/predictive/patterns",
            get(predictive_patterns_handler),
        )
        .route(
            "/intelligence/predictive/sequences",
            post(predictive_sequences_handler),
        )
        // -- NEW: Reconsolidation
        .route(
            "/intelligence/reconsolidate/{memory_id}",
            post(reconsolidate_handler),
        )
        .route(
            "/intelligence/reconsolidation/candidates",
            get(reconsolidation_candidates_handler),
        )
        // -- NEW: Extraction
        .route("/intelligence/extract", post(extract_handler))
        // -- NEW: Time travel
        .route("/timetravel", post(time_travel_handler))
        // -- NEW: Sweep
        .route("/sweep", post(sweep_handler))
        // -- NEW: Correct
        .route("/correct", post(correct_handler))
        // -- NEW: Memory health
        .route("/memory-health", get(memory_health_handler))
        // -- NEW: Feedback
        .route("/feedback", post(feedback_handler))
        .route("/feedback/stats", get(feedback_stats_handler))
        // -- NEW: Duplicates
        .route("/duplicates", get(duplicates_handler))
        .route("/deduplicate", post(deduplicate_handler))
        // -- NEW: Dream
        .route("/intelligence/dream", post(dream_handler))
        // -- NEW: Pipeline DAG runner
        .route("/intelligence/run", post(run_pipeline_handler))
        // -- Dreamer stats from the background task
        .route("/intelligence/dreamer", get(dreamer_stats_handler))
}

// H-R3-002: dreamer stats are operator-wide telemetry (one shared dreamer
// task across all tenants); no tenant data touched, so Auth(_auth) is
// intentional.
#[tracing::instrument(skip_all)]
async fn dreamer_stats_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
) -> Result<Json<Value>, AppError> {
    let stats = state.dreamer_stats.read().await;
    Ok(Json(json!(*stats)))
}

// --- Consolidation ---

#[tracing::instrument(skip_all)]
async fn consolidate_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<ConsolidateBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    if !state.config.consolidation_enabled {
        return Err(AppError(kleos_lib::EngError::Forbidden(
            "consolidation is disabled; set KLEOS_CONSOLIDATION_ENABLED=1 to re-enable".into(),
        )));
    }
    let ids: Vec<String> = body
        .memory_ids
        .into_iter()
        .map(|id| id.to_string())
        .collect();
    let result = consolidate(&db, &ids, auth.effective_user_id()).await?;
    Ok((StatusCode::CREATED, Json(json!(result))))
}

/// GET /intelligence/candidates -- list memory candidates for further intelligence processing.
#[tracing::instrument(skip_all)]
async fn candidates_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<CandidatesBody>,
) -> Result<Json<Value>, AppError> {
    if !state.config.consolidation_enabled {
        return Err(AppError(kleos_lib::EngError::Forbidden(
            "consolidation is disabled; set KLEOS_CONSOLIDATION_ENABLED=1 to re-enable".into(),
        )));
    }
    let threshold = body.threshold.unwrap_or(0.7);
    let groups = find_consolidation_candidates(&db, threshold, auth.effective_user_id()).await?;
    Ok(Json(json!({ "groups": groups })))
}

/// GET /intelligence/consolidations -- list completed consolidation records.
#[tracing::instrument(skip_all)]
async fn list_consolidations_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<LimitQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(20).min(500);
    let items = list_consolidations(&db, auth.effective_user_id(), limit).await?;
    Ok(Json(json!({ "consolidations": items })))
}

// --- Contradiction ---

#[tracing::instrument(skip_all)]
async fn contradictions_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(memory_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let mem = memory::get(&db, memory_id, auth.effective_user_id()).await?;
    let contradictions = detect_contradictions(&db, &mem).await?;
    Ok(Json(json!({ "contradictions": contradictions })))
}

/// POST /intelligence/contradictions -- scan the corpus for fact-level contradictions and return all detected pairs.
#[tracing::instrument(skip_all)]
async fn scan_contradictions_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
) -> Result<Json<Value>, AppError> {
    let contradictions = scan_all_contradictions(&db, auth.effective_user_id()).await?;
    Ok(Json(json!({ "contradictions": contradictions })))
}

// --- Decomposition ---

/// POST /intelligence/decompose/:memory_id -- split an owned memory into
/// atomic child facts.
#[tracing::instrument(skip_all)]
async fn decompose_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(memory_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let child_ids = decompose(&db, memory_id, auth.user_id).await?;
    Ok(Json(json!({ "child_ids": child_ids })))
}

// --- Temporal ---

#[tracing::instrument(skip_all)]
async fn detect_temporal_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
) -> Result<Json<Value>, AppError> {
    let patterns = detect_patterns(&db, auth.effective_user_id()).await?;
    // detect_patterns already stores each pattern; these store calls are
    // now redundant but kept for backwards-compat with existing routes that
    // used to call store_pattern separately. They are scoped to auth.effective_user_id().
    for pattern in &patterns {
        if let Err(e) = store_pattern(&db, pattern, auth.effective_user_id()).await {
            tracing::warn!("failed to store temporal pattern: {}", e);
        }
    }
    Ok(Json(json!({ "patterns": patterns })))
}

/// GET /intelligence/temporal/patterns -- list previously detected temporal patterns, newest first.
#[tracing::instrument(skip_all)]
async fn list_temporal_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<LimitQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(20).clamp(1, 500) as i64;
    let patterns = list_patterns(&db, auth.effective_user_id(), limit).await?;
    Ok(Json(json!({ "patterns": patterns })))
}

// --- Digests ---

#[tracing::instrument(skip_all)]
async fn generate_digest_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<DigestBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let period = body.period.unwrap_or_else(|| "daily".into());
    let digest = generate_digest(&db, auth.effective_user_id(), &period).await?;
    Ok((StatusCode::CREATED, Json(json!(digest))))
}

/// GET /intelligence/digests -- list generated periodic digests, newest first.
#[tracing::instrument(skip_all)]
async fn list_digests_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<LimitQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(20).min(500);
    let items = list_digests(&db, auth.effective_user_id(), limit).await?;
    Ok(Json(json!({ "digests": items })))
}

// --- Reflections ---

#[tracing::instrument(skip_all)]
async fn create_reflection_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<CreateReflectionBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let reflection_type = body.reflection_type.as_deref().unwrap_or("general");
    let confidence = body.confidence.unwrap_or(1.0);
    let reflection = create_reflection(
        &db,
        &body.content,
        reflection_type,
        &body.source_memory_ids,
        confidence,
        auth.effective_user_id(),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(json!(reflection))))
}

/// GET /intelligence/reflections -- list generated reflection records, newest first.
#[tracing::instrument(skip_all)]
async fn list_reflections_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<LimitQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(20).min(500);
    let items = list_reflections(&db, auth.effective_user_id(), limit).await?;
    Ok(Json(json!({ "reflections": items })))
}

// Hybrid: needs state.llm for LLM-driven generation.
#[tracing::instrument(skip_all)]
async fn generate_reflections_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<LimitQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(10).min(100);
    let llm_ref: Option<&dyn LlmReflector> = state.llm.as_deref().map(|c| c as &dyn LlmReflector);
    let items =
        generate_reflections_with_llm(&db, llm_ref, auth.effective_user_id(), limit).await?;
    Ok(Json(json!({ "reflections": items, "count": items.len() })))
}

// --- Causal ---

#[tracing::instrument(skip_all)]
async fn create_chain_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<CreateChainBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let chain = create_chain(
        &db,
        body.root_memory_id,
        body.description.as_deref(),
        auth.effective_user_id(),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(json!(chain))))
}

/// GET /intelligence/chains -- list causal chains, newest first.
#[tracing::instrument(skip_all)]
async fn list_chains_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<LimitQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(20).min(500);
    let items = list_chains(&db, auth.effective_user_id(), limit).await?;
    Ok(Json(json!({ "chains": items })))
}

/// GET /intelligence/chains/{id} -- fetch one causal chain with its full link set.
#[tracing::instrument(skip_all)]
async fn get_chain_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let chain = get_chain(&db, id, auth.effective_user_id()).await?;
    Ok(Json(json!(chain)))
}

/// POST /intelligence/chains/{id}/links -- append a causal link to an existing chain.
#[tracing::instrument(skip_all)]
async fn add_link_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<AddLinkBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let strength = body.strength.unwrap_or(1.0);
    let order_index = body.order_index.unwrap_or(0);
    let link = add_link(
        &db,
        body.chain_id,
        body.cause_memory_id,
        body.effect_memory_id,
        strength,
        order_index,
        auth.effective_user_id(),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(json!(link))))
}

/// GET /intelligence/causal/backward/{memory_id} -- traverse causal links backward from a memory.
#[tracing::instrument(skip_all)]
async fn causal_backward_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(memory_id): Path<i64>,
    body: Option<Json<BackwardBody>>,
) -> Result<Json<Value>, AppError> {
    let max_depth = body.and_then(|b| b.0.max_depth).unwrap_or(5).min(20);
    let ancestors = backward_chain(&db, memory_id, auth.effective_user_id(), max_depth).await?;
    Ok(Json(
        json!({ "ancestors": ancestors, "max_depth": max_depth, "count": ancestors.len() }),
    ))
}

// --- Sentiment ---

#[tracing::instrument(skip_all)]
async fn sentiment_analyze_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<SentimentAnalyzeBody>,
) -> Result<Json<Value>, AppError> {
    let text = if let Some(ref content) = body.content {
        content.clone()
    } else if let Some(memory_id) = body.memory_id {
        let mem = memory::get(&db, memory_id, auth.effective_user_id()).await?;
        mem.content
    } else {
        return Err(AppError::from(kleos_lib::EngError::InvalidInput(
            "provide either 'content' or 'memory_id'".to_string(),
        )));
    };

    let score = sentiment::score_text(&text);
    let (sum, count) = sentiment::score_text_sum(&text);
    let label = if score > 1.0 {
        "positive"
    } else if score < -1.0 {
        "negative"
    } else {
        "neutral"
    };

    Ok(Json(json!({
        "score": score,
        "label": label,
        "sum": sum,
        "word_count": count,
    })))
}

/// GET /intelligence/sentiment/history -- list historical sentiment records.
#[tracing::instrument(skip_all)]
async fn sentiment_history_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<SentimentHistoryQuery>,
) -> Result<Json<Value>, AppError> {
    let limit =
        kleos_lib::validation::clamp_signed_limit(params.limit.unwrap_or(20), 20, 100) as i64;
    let since = params.since.as_deref().unwrap_or("1970-01-01");

    // Scope to the caller: in monolith mode ResolvedDb is the shared DB, so
    // without the user_id predicate this leaked every tenant's memory ids,
    // timestamps, and derived sentiment scores.
    let user_id = auth.effective_user_id();
    let since_owned = since.to_string();
    let history = db
        .read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, content, created_at FROM memories \
                     WHERE is_forgotten = 0 AND created_at >= ?1 AND user_id = ?3 \
                     ORDER BY created_at DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![since_owned, limit, user_id], |row| {
                let id: i64 = row.get(0)?;
                let content: String = row.get(1)?;
                let created_at: String = row.get(2)?;
                Ok((id, content, created_at))
            })?;
            let mut history = Vec::new();
            for row in rows {
                let (id, content, created_at) = row?;
                let score = sentiment::score_text(&content);
                history.push(serde_json::json!({
                    "memory_id": id,
                    "score": score,
                    "created_at": created_at,
                }));
            }
            Ok(history)
        })
        .await?;

    Ok(Json(json!({ "history": history })))
}

// --- Valence ---

#[tracing::instrument(skip_all)]
async fn valence_score_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<ValenceScoreBody>,
) -> Result<Json<Value>, AppError> {
    if let Some(memory_id) = body.memory_id {
        let mem = memory::get(&db, memory_id, auth.effective_user_id()).await?;
        let result = store_valence(&db, memory_id, &mem.content).await?;
        Ok(Json(json!(result)))
    } else if let Some(ref content) = body.content {
        let result = analyze_valence(content);
        Ok(Json(json!(result)))
    } else {
        Err(AppError::from(kleos_lib::EngError::InvalidInput(
            "provide either 'memory_id' or 'content'".to_string(),
        )))
    }
}

/// GET /intelligence/valence/{memory_id} -- fetch the emotional valence record for a memory.
#[tracing::instrument(skip_all)]
async fn valence_get_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(memory_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let mem = memory::get(&db, memory_id, auth.effective_user_id()).await?;
    Ok(Json(json!({
        "memory_id": memory_id,
        "valence": mem.valence,
        "arousal": mem.arousal,
        "dominant_emotion": mem.dominant_emotion,
    })))
}

/// GET /intelligence/valence/profile -- aggregate emotional profile across the corpus.
#[tracing::instrument(skip_all)]
async fn valence_profile_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
) -> Result<Json<Value>, AppError> {
    let profile = get_emotional_profile(&db, auth.effective_user_id()).await?;
    Ok(Json(json!(profile)))
}

// --- Predictive ---

#[tracing::instrument(skip_all)]
async fn predictive_recall_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
) -> Result<Json<Value>, AppError> {
    let context = predictive_recall(&db, auth.effective_user_id()).await?;
    Ok(Json(json!(context)))
}

/// GET /intelligence/predictive/patterns -- return persisted temporal patterns used to drive predictions.
#[tracing::instrument(skip_all)]
async fn predictive_patterns_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<LimitQuery>,
) -> Result<Json<Value>, AppError> {
    // Return temporal patterns that drive predictions, scoped to the caller.
    let limit = params.limit.unwrap_or(20).clamp(1, 500) as i64;
    let patterns = list_patterns(&db, auth.effective_user_id(), limit).await?;
    Ok(Json(json!({ "patterns": patterns })))
}

/// GET /intelligence/predictive/sequences -- mine recent memory sequences for repeated bigrams within a time window.
#[tracing::instrument(skip_all)]
async fn predictive_sequences_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<SequencesBody>,
) -> Result<Json<Value>, AppError> {
    let window_mins = body.window_mins.unwrap_or(60).clamp(1, 24 * 60);
    let patterns = detect_sequence_patterns(&db, auth.user_id, window_mins).await?;
    Ok(Json(
        json!({ "patterns": patterns, "window_mins": window_mins, "count": patterns.len() }),
    ))
}

// --- Reconsolidation ---

#[tracing::instrument(skip_all)]
async fn reconsolidate_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(memory_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let result = reconsolidate_memory(&db, memory_id, auth.effective_user_id()).await?;
    Ok(Json(json!(result)))
}

/// GET /intelligence/reconsolidation/candidates -- list memories eligible for the next reconsolidation sweep.
#[tracing::instrument(skip_all)]
async fn reconsolidation_candidates_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<ReconsolidationCandidatesQuery>,
) -> Result<Json<Value>, AppError> {
    let batch_size = params.limit.unwrap_or(20).min(100);
    let results = run_reconsolidation_sweep(&db, auth.effective_user_id(), batch_size).await?;
    Ok(Json(json!({ "results": results, "count": results.len() })))
}

// --- Extraction ---

#[tracing::instrument(skip_all)]
async fn extract_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<ExtractBody>,
) -> Result<Json<Value>, AppError> {
    let (content, memory_id) = if let Some(mid) = body.memory_id {
        let mem = memory::get(&db, mid, auth.effective_user_id()).await?;
        (mem.content, mid)
    } else if let Some(ref c) = body.content {
        // Store as temp memory so extraction has a memory_id to reference
        let result = memory::store(
            &db,
            kleos_lib::memory::types::StoreRequest {
                content: c.clone(),
                source: "extraction".to_string(),
                user_id: Some(auth.effective_user_id()),
                ..Default::default()
            },
            None,
            false,
        )
        .await?;
        (c.clone(), result.id)
    } else {
        return Err(AppError::from(kleos_lib::EngError::InvalidInput(
            "provide either 'content' or 'memory_id'".to_string(),
        )));
    };

    let stats =
        fast_extract_facts(&db, &content, memory_id, auth.effective_user_id(), None).await?;
    Ok(Json(json!({
        "memory_id": memory_id,
        "facts": stats.facts,
        "preferences": stats.preferences,
        "state_updates": stats.state_updates,
    })))
}

// --- Time Travel ---

#[tracing::instrument(skip_all)]
async fn time_travel_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<TimeTravelBody>,
) -> Result<Json<Value>, AppError> {
    let limit = kleos_lib::validation::clamp_signed_limit(body.limit.unwrap_or(20), 20, 100) as i64;
    let results = time_travel(
        &db,
        auth.effective_user_id(),
        body.query.as_deref(),
        &body.timestamp,
        limit,
    )
    .await?;
    Ok(Json(json!({
        "results": results,
        "timestamp": body.timestamp,
        "count": results.len(),
    })))
}

// --- Sweep ---

#[tracing::instrument(skip_all)]
async fn sweep_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<SweepBody>,
) -> Result<Json<Value>, AppError> {
    if !state.config.consolidation_enabled {
        return Err(AppError(kleos_lib::EngError::Forbidden(
            "consolidation is disabled; set KLEOS_CONSOLIDATION_ENABLED=1 to re-enable".into(),
        )));
    }
    let threshold = body.threshold.unwrap_or(0.85);
    let result = sweep(&db, auth.effective_user_id(), threshold).await?;
    Ok(Json(json!(result)))
}

// --- Correct ---

#[tracing::instrument(skip_all)]
async fn correct_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<CorrectBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let corrected = correct_memory(
        &db,
        auth.effective_user_id(),
        body.memory_id,
        &body.content,
        body.reason.as_deref(),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(json!(corrected))))
}

// --- Memory Health ---

#[tracing::instrument(skip_all)]
async fn memory_health_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
) -> Result<Json<Value>, AppError> {
    let report = memory_health(&db, auth.effective_user_id()).await?;
    Ok(Json(json!(report)))
}

// --- Feedback ---

#[tracing::instrument(skip_all)]
async fn feedback_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<FeedbackRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    feedback::record_feedback(&db, auth.effective_user_id(), &body).await?;
    Ok((StatusCode::CREATED, Json(json!({ "ok": true }))))
}

/// GET /intelligence/feedback/stats -- aggregate stats over recorded user feedback events.
#[tracing::instrument(skip_all)]
async fn feedback_stats_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
) -> Result<Json<Value>, AppError> {
    let stats = feedback::feedback_stats(&db, auth.effective_user_id()).await?;
    Ok(Json(json!(stats)))
}

// --- Duplicates ---

#[tracing::instrument(skip_all)]
async fn duplicates_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<DuplicatesQuery>,
) -> Result<Json<Value>, AppError> {
    let threshold = params.threshold.unwrap_or(0.9);
    let limit = params.limit.unwrap_or(50).min(200);
    let pairs = find_duplicates(&db, auth.effective_user_id(), threshold, limit).await?;
    Ok(Json(json!({ "duplicates": pairs, "count": pairs.len() })))
}

/// POST /intelligence/deduplicate -- scan for near-duplicate memory pairs and persist memory_links rows.
#[tracing::instrument(skip_all)]
async fn deduplicate_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<DeduplicateBody>,
) -> Result<Json<Value>, AppError> {
    let threshold = body.threshold.unwrap_or(0.9);
    let dry_run = body.dry_run.unwrap_or(true);
    let result = deduplicate(&db, auth.effective_user_id(), threshold, dry_run).await?;
    Ok(Json(json!(result)))
}

// --- Dream (Eidolon integration -- graceful degradation) ---

#[tracing::instrument(skip_all)]
async fn run_pipeline_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
) -> Result<Json<Value>, AppError> {
    let report = default_pipeline(
        state.config.consolidation_enabled,
        state
            .config
            .auto_link_enabled
            .then_some(state.config.auto_link_batch),
    )
    .run(&db, auth.effective_user_id())
    .await?;
    Ok(Json(json!(report)))
}

// H-R3-001: shadow path of /brain/dream. Both must require admin scope or
// the gate on the other endpoint is meaningless.
#[tracing::instrument(skip_all)]
async fn dream_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    if !auth.has_scope(&kleos_lib::auth::Scope::Admin) {
        return Err(AppError(kleos_lib::EngError::Auth(
            "admin scope required for global dream cycle".into(),
        )));
    }
    if let Some(ref brain) = state.brain {
        // Use the monolith DB — users table lives in the registry, not tenant shards.
        let users = active_user_ids(&state.db)
            .await
            .map_err(|e| AppError(kleos_lib::EngError::Internal(e.to_string())))?;
        let mut last_result = String::new();
        for user_id in users {
            match brain.dream_cycle(user_id).await {
                Ok(result) => {
                    last_result = format!("{:?}", result);
                }
                Err(e) => {
                    return Ok(Json(json!({
                        "status": "error",
                        "user_id": user_id,
                        "error": format!("{}", e),
                    })));
                }
            }
        }
        Ok(Json(json!({
            "status": "completed",
            "result": last_result,
        })))
    } else {
        Ok(Json(json!({
            "status": "unavailable",
            "message": "Neural backend (Brain/Eidolon) is not configured",
        })))
    }
}
