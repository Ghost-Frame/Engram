use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use engram_lib::services::thymus::{
    create_rubric, delete_rubric, evaluate, get_drift_events, get_evaluation, get_metric_summary,
    get_metrics, get_rubric, get_session_quality, get_stats, list_evaluations, list_rubrics,
    record_drift_event, record_metric, record_session_quality, update_rubric, CreateRubricRequest,
    EvaluateRequest, RecordDriftEventRequest, RecordMetricRequest, RecordSessionQualityRequest,
    UpdateRubricRequest,
};

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
        .route("/thymus/stats", get(get_stats_handler))
}

// ---------------------------------------------------------------------------
// Rubric handlers
// ---------------------------------------------------------------------------

async fn list_rubrics_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let rubrics = list_rubrics(&state.db, auth.user_id).await?;
    Ok(Json(json!({ "rubrics": rubrics })))
}

async fn create_rubric_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<CreateRubricRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let req = CreateRubricRequest {
        user_id: Some(auth.user_id),
        ..body
    };
    let rubric = create_rubric(&state.db, req).await?;
    Ok((StatusCode::CREATED, Json(json!(rubric))))
}

async fn get_rubric_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let rubric = get_rubric(&state.db, id, auth.user_id).await?;
    Ok(Json(json!(rubric)))
}

async fn update_rubric_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<UpdateRubricRequest>,
) -> Result<Json<Value>, AppError> {
    let rubric = update_rubric(&state.db, id, body, auth.user_id).await?;
    Ok(Json(json!(rubric)))
}

async fn delete_rubric_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    delete_rubric(&state.db, id, auth.user_id).await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Evaluation handlers
// ---------------------------------------------------------------------------

async fn evaluate_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<EvaluateRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let req = EvaluateRequest {
        user_id: Some(auth.user_id),
        ..body
    };
    let evaluation = evaluate(&state.db, req).await?;
    Ok((StatusCode::CREATED, Json(json!(evaluation))))
}

#[derive(Debug, Deserialize)]
struct ListEvaluationsParams {
    agent: Option<String>,
    rubric_id: Option<i64>,
    limit: Option<usize>,
}

async fn list_evaluations_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ListEvaluationsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(100);
    let evaluations = list_evaluations(
        &state.db,
        auth.user_id,
        params.agent.as_deref(),
        params.rubric_id,
        limit,
    )
    .await?;
    Ok(Json(json!({ "evaluations": evaluations })))
}

async fn get_evaluation_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let evaluation = get_evaluation(&state.db, id, auth.user_id).await?;
    Ok(Json(json!(evaluation)))
}

// ---------------------------------------------------------------------------
// Metric handlers
// ---------------------------------------------------------------------------

async fn record_metric_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<RecordMetricRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let req = RecordMetricRequest {
        user_id: Some(auth.user_id),
        ..body
    };
    let metric = record_metric(&state.db, req).await?;
    Ok((StatusCode::CREATED, Json(json!(metric))))
}

#[derive(Debug, Deserialize)]
struct GetMetricsParams {
    agent: Option<String>,
    metric: Option<String>,
    since: Option<String>,
    limit: Option<usize>,
}

async fn get_metrics_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<GetMetricsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(100);
    let metrics = get_metrics(
        &state.db,
        auth.user_id,
        params.agent.as_deref(),
        params.metric.as_deref(),
        params.since.as_deref(),
        limit,
    )
    .await?;
    Ok(Json(json!({ "metrics": metrics })))
}

#[derive(Debug, Deserialize)]
struct MetricSummaryParams {
    agent: Option<String>,
    metric: Option<String>,
    since: Option<String>,
}

async fn get_metric_summary_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<MetricSummaryParams>,
) -> Result<Json<Value>, AppError> {
    let agent = params.agent.as_deref().unwrap_or("*");
    let metric = params.metric.as_deref().unwrap_or("*");

    let summary = get_metric_summary(
        &state.db,
        auth.user_id,
        agent,
        metric,
        params.since.as_deref(),
    )
    .await?;
    Ok(Json(summary))
}

// ---------------------------------------------------------------------------
// Session quality handlers
// ---------------------------------------------------------------------------

async fn record_session_quality_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Json(body): Json<RecordSessionQualityRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let sq = record_session_quality(&state.db, body).await?;
    Ok((StatusCode::CREATED, Json(json!(sq))))
}

#[derive(Debug, Deserialize)]
struct SessionQualityParams {
    agent: Option<String>,
    since: Option<String>,
    limit: Option<usize>,
}

async fn get_session_quality_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Query(params): Query<SessionQualityParams>,
) -> Result<Json<Value>, AppError> {
    let agent = params.agent.as_deref().unwrap_or("*");
    let limit = params.limit.unwrap_or(100);
    let records = get_session_quality(&state.db, agent, params.since.as_deref(), limit).await?;
    Ok(Json(json!({ "session_quality": records })))
}

// ---------------------------------------------------------------------------
// Drift event handlers
// ---------------------------------------------------------------------------

async fn record_drift_event_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Json(body): Json<RecordDriftEventRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let event = record_drift_event(&state.db, body).await?;
    Ok((StatusCode::CREATED, Json(json!(event))))
}

#[derive(Debug, Deserialize)]
struct DriftEventsParams {
    agent: Option<String>,
    limit: Option<usize>,
}

async fn get_drift_events_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Query(params): Query<DriftEventsParams>,
) -> Result<Json<Value>, AppError> {
    let agent = params.agent.as_deref().unwrap_or("*");
    let limit = params.limit.unwrap_or(100);
    let events = get_drift_events(&state.db, agent, limit).await?;
    Ok(Json(json!({ "drift_events": events })))
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

async fn get_stats_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
) -> Result<Json<Value>, AppError> {
    let stats = get_stats(&state.db).await?;
    Ok(Json(json!(stats)))
}
