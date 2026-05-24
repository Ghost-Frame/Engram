use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::{Auth, ResolvedDb};
use crate::state::AppState;
use kleos_lib::services::thymus::{
    create_rubric, delete_rubric, evaluate, get_agent_scores, get_drift_events, get_drift_summary,
    get_evaluation, get_metric_summary, get_metrics, get_rubric, get_session_quality, get_stats,
    list_evaluations, list_rubrics, record_drift_event, record_metric, record_session_quality,
    update_rubric, CreateRubricRequest, EvaluateRequest, RecordDriftEventRequest,
    RecordMetricRequest, RecordSessionQualityRequest, UpdateRubricRequest,
};

mod types;
use types::{
    AgentScoresParams, DriftEventsParams, DriftSummaryParams, GetMetricsParams,
    ListEvaluationsParams, MetricSummaryParams, SessionQualityParams,
};

/// Builds the Axum sub-router for all `/thymus/*` endpoints.
///
/// Registers rubric CRUD, evaluation, agent score aggregation, quality metrics,
/// session quality, drift events, and stats routes. The returned router is
/// expected to be merged into the top-level [`AppState`] router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/thymus/rubrics",
            get(list_rubrics_handler).post(create_rubric_handler),
        )
        .route(
            "/thymus/rubrics/{id}",
            get(get_rubric_handler)
                .patch(update_rubric_handler)
                .delete(delete_rubric_handler),
        )
        .route("/thymus/evaluate", post(evaluate_handler))
        .route("/thymus/evaluations", get(list_evaluations_handler))
        .route("/thymus/evaluations/{id}", get(get_evaluation_handler))
        .route(
            "/thymus/agents/{agent}/scores",
            get(get_agent_scores_handler),
        )
        .route(
            "/thymus/metrics",
            post(record_metric_handler).get(get_metrics_handler),
        )
        .route("/thymus/metrics/summary", get(get_metric_summary_handler))
        .route(
            "/thymus/session-quality",
            post(record_session_quality_handler).get(get_session_quality_handler),
        )
        .route(
            "/thymus/drift-events",
            post(record_drift_event_handler).get(get_drift_events_handler),
        )
        .route("/thymus/drift-summary", get(get_drift_summary_handler))
        .route("/thymus/stats", get(get_stats_handler))
}

// ---------------------------------------------------------------------------
// Rubric handlers
// ---------------------------------------------------------------------------

/// Handler for `GET /thymus/rubrics`. Returns all rubrics belonging to the
/// authenticated user as `{ "rubrics": [...] }`. Returns 200 on success.
async fn list_rubrics_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
) -> Result<Json<Value>, AppError> {
    let rubrics = list_rubrics(&db, auth.user_id).await?;
    Ok(Json(json!({ "rubrics": rubrics })))
}

/// Handler for `POST /thymus/rubrics`. Creates a new rubric and returns it
/// with `201 Created`. Injects the authenticated user's ID into the request.
async fn create_rubric_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<CreateRubricRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let req = CreateRubricRequest {
        user_id: Some(auth.user_id),
        ..body
    };
    let rubric = create_rubric(&db, req).await?;
    Ok((StatusCode::CREATED, Json(json!(rubric))))
}

/// Handler for `GET /thymus/rubrics/{id}`. Returns the rubric or 404 if not
/// found or not owned by the authenticated user.
async fn get_rubric_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let rubric = get_rubric(&db, id, auth.user_id).await?;
    Ok(Json(json!(rubric)))
}

/// Handler for `PATCH /thymus/rubrics/{id}`. Applies a partial update and
/// returns the updated rubric. Fails with 404 if the rubric does not exist
/// or is not owned by the authenticated user.
async fn update_rubric_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
    Json(body): Json<UpdateRubricRequest>,
) -> Result<Json<Value>, AppError> {
    let rubric = update_rubric(&db, id, auth.user_id, body).await?;
    Ok(Json(json!(rubric)))
}

/// Handler for `DELETE /thymus/rubrics/{id}`. Deletes the rubric owned by the
/// authenticated user and returns `{ "ok": true }`.
async fn delete_rubric_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    delete_rubric(&db, id, auth.user_id).await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Evaluation handlers
// ---------------------------------------------------------------------------

/// Handler for `POST /thymus/evaluate`. Runs an evaluation against a rubric and
/// records the result. Injects the authenticated user's ID into the request body.
/// Returns `201 Created` with the full evaluation record on success.
async fn evaluate_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<EvaluateRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let req = EvaluateRequest {
        user_id: Some(auth.user_id),
        ..body
    };
    let evaluation = evaluate(&db, req).await?;
    Ok((StatusCode::CREATED, Json(json!(evaluation))))
}

/// Handler for `GET /thymus/evaluations`. Returns a page of evaluations
/// belonging to the authenticated user, optionally filtered by agent and
/// rubric. Limit is capped at 1000.
async fn list_evaluations_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<ListEvaluationsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(100).min(1000);
    let evaluations = list_evaluations(
        &db,
        auth.user_id,
        params.agent.as_deref(),
        params.rubric_id,
        limit,
    )
    .await?;
    Ok(Json(json!({ "evaluations": evaluations })))
}

/// Handler for `GET /thymus/evaluations/{id}`. Returns a single evaluation
/// owned by the authenticated user by primary key, or 404 if not found.
async fn get_evaluation_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let evaluation = get_evaluation(&db, id, auth.user_id).await?;
    Ok(Json(json!(evaluation)))
}

/// Handler for `GET /thymus/agents/{agent}/scores`.
///
/// Returns aggregate evaluation scores for one agent across evaluations
/// belonging to the authenticated user, optionally filtered to a specific
/// rubric (`rubric_id`) or a time window (`since`). Delegates to
/// [`kleos_lib::services::thymus::get_agent_scores`].
///
/// Response shape: `{ agent, overall_avg, evaluation_count, by_criterion }`.
async fn get_agent_scores_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(agent): Path<String>,
    Query(params): Query<AgentScoresParams>,
) -> Result<Json<Value>, AppError> {
    let scores = get_agent_scores(
        &db,
        auth.user_id,
        &agent,
        params.rubric_id,
        params.since.as_deref(),
    )
    .await?;
    Ok(Json(json!(scores)))
}

// ---------------------------------------------------------------------------
// Metric handlers
// ---------------------------------------------------------------------------

/// Handler for `POST /thymus/metrics`. Records a single quality metric
/// observation. Injects the authenticated user's ID into the request body
/// before persisting. Returns `201 Created` with the stored metric record.
async fn record_metric_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<RecordMetricRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let req = RecordMetricRequest {
        user_id: Some(auth.user_id),
        ..body
    };
    let metric = record_metric(&db, req).await?;
    Ok((StatusCode::CREATED, Json(json!(metric))))
}

/// Handler for `GET /thymus/metrics`. Returns quality metric records belonging
/// to the authenticated user, optionally filtered by agent, metric name, and
/// time window. Limit capped at 1000.
async fn get_metrics_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<GetMetricsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(100).min(1000);
    let metrics = get_metrics(
        &db,
        auth.user_id,
        params.agent.as_deref(),
        params.metric.as_deref(),
        params.since.as_deref(),
        limit,
    )
    .await?;
    Ok(Json(json!({ "metrics": metrics })))
}

/// Handler for `GET /thymus/metrics/summary`. Returns aggregate statistics
/// (avg, min, max, count) for a given agent+metric combination belonging to
/// the authenticated user.
async fn get_metric_summary_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<MetricSummaryParams>,
) -> Result<Json<Value>, AppError> {
    let agent = params.agent.as_deref().ok_or_else(|| {
        kleos_lib::EngError::InvalidInput("agent query parameter is required".into())
    })?;
    let metric = params.metric.as_deref().ok_or_else(|| {
        kleos_lib::EngError::InvalidInput("metric query parameter is required".into())
    })?;

    let summary =
        get_metric_summary(&db, auth.user_id, agent, metric, params.since.as_deref()).await?;
    Ok(Json(summary))
}

// ---------------------------------------------------------------------------
// Session quality handlers
// ---------------------------------------------------------------------------

/// Handler for `POST /thymus/session-quality`. Records a session quality
/// snapshot (e.g. coherence, goal completion, drift indicators). Stamps the
/// authenticated user's ID onto the mutable body before persisting.
/// Returns `201 Created` with the stored session quality record on success.
async fn record_session_quality_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(mut body): Json<RecordSessionQualityRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    body.user_id = Some(auth.user_id);
    let sq = record_session_quality(&db, body).await?;
    Ok((StatusCode::CREATED, Json(json!(sq))))
}

/// Handler for `GET /thymus/session-quality`. Returns session quality records
/// for a given agent belonging to the authenticated user, optionally windowed
/// by `since`. Limit capped at 1000.
async fn get_session_quality_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<SessionQualityParams>,
) -> Result<Json<Value>, AppError> {
    let agent = params.agent.as_deref().unwrap_or("*");
    let limit = params.limit.unwrap_or(100).min(1000);
    let records =
        get_session_quality(&db, auth.user_id, agent, params.since.as_deref(), limit).await?;
    Ok(Json(json!({ "session_quality": records })))
}

// ---------------------------------------------------------------------------
// Drift event handlers
// ---------------------------------------------------------------------------

/// Handler for `POST /thymus/drift-events`. Records a behavioral drift event
/// for an agent (e.g. persona divergence, instruction violation). Stamps the
/// authenticated user's ID onto the mutable body before persisting.
/// Returns `201 Created` with the stored drift event record on success.
async fn record_drift_event_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(mut body): Json<RecordDriftEventRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    body.user_id = Some(auth.user_id);
    let event = record_drift_event(&db, body).await?;
    Ok((StatusCode::CREATED, Json(json!(event))))
}

/// Handler for `GET /thymus/drift-events`. Returns behavioral drift event
/// records for a given agent belonging to the authenticated user. Limit capped
/// at 1000.
async fn get_drift_events_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<DriftEventsParams>,
) -> Result<Json<Value>, AppError> {
    let agent = params.agent.as_deref().unwrap_or("*");
    let limit = params.limit.unwrap_or(100).min(1000);
    let events = get_drift_events(&db, auth.user_id, agent, limit).await?;
    Ok(Json(json!({ "drift_events": events })))
}

/// Handler for `GET /thymus/drift-summary?agent=<name>`. Returns aggregated
/// drift event counts grouped by drift_type and severity for the named agent
/// belonging to the authenticated user. Requires the `agent` query parameter;
/// returns 400 when absent.
async fn get_drift_summary_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<DriftSummaryParams>,
) -> Result<Json<Value>, AppError> {
    let agent = params
        .agent
        .ok_or_else(|| kleos_lib::EngError::InvalidInput("agent is required".into()))?;
    let summary = get_drift_summary(&db, auth.user_id, &agent).await?;
    Ok(Json(json!({ "drift_summary": summary })))
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

/// Handler for `GET /thymus/stats`. Returns aggregate thymus statistics for
/// the authenticated user (rubric count, evaluation count, metric count, etc.).
/// No query parameters. Returns 200 on success.
async fn get_stats_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
) -> Result<Json<Value>, AppError> {
    let stats = get_stats(&db, auth.user_id).await?;
    Ok(Json(json!(stats)))
}
