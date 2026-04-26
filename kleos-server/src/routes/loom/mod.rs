use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::{Auth, ResolvedDb};
use crate::state::AppState;
use kleos_lib::services::loom::{
    cancel_run, complete_step, create_run, create_workflow, delete_workflow, fail_step, get_logs,
    get_run, get_stats, get_steps, get_workflow, list_runs, list_workflows, update_workflow,
    CreateRunRequest, CreateWorkflowRequest, UpdateWorkflowRequest,
};

mod types;
use types::{CompleteStepBody, FailStepBody, GetLogsParams, ListRunsParams};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/loom/workflows",
            get(list_workflows_handler).post(create_workflow_handler),
        )
        .route(
            "/loom/workflows/{id}",
            get(get_workflow_handler)
                .patch(update_workflow_handler)
                .delete(delete_workflow_handler),
        )
        .route(
            "/loom/runs",
            post(create_run_handler).get(list_runs_handler),
        )
        .route("/loom/runs/{id}", get(get_run_handler))
        .route("/loom/runs/{id}/cancel", post(cancel_run_handler))
        .route("/loom/runs/{id}/steps", get(get_steps_handler))
        .route("/loom/runs/{id}/logs", get(get_logs_handler))
        .route("/loom/steps/{id}/complete", post(complete_step_handler))
        .route("/loom/steps/{id}/fail", post(fail_step_handler))
        .route("/loom/stats", get(get_stats_handler))
}

// ---------------------------------------------------------------------------
// Workflow handlers
// ---------------------------------------------------------------------------

async fn list_workflows_handler(
    Auth(_auth): Auth,
    ResolvedDb(db): ResolvedDb,
) -> Result<Json<Value>, AppError> {
    let workflows = list_workflows(&db).await?;
    Ok(Json(json!({ "workflows": workflows })))
}

async fn create_workflow_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<CreateWorkflowRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let req = CreateWorkflowRequest {
        user_id: Some(auth.user_id),
        ..body
    };
    let workflow = create_workflow(&db, req).await?;
    Ok((StatusCode::CREATED, Json(json!(workflow))))
}

async fn get_workflow_handler(
    Auth(_auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let workflow = get_workflow(&db, id).await?;
    Ok(Json(json!(workflow)))
}

async fn update_workflow_handler(
    Auth(_auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
    Json(body): Json<UpdateWorkflowRequest>,
) -> Result<Json<Value>, AppError> {
    let workflow = update_workflow(&db, id, body).await?;
    Ok(Json(json!(workflow)))
}

async fn delete_workflow_handler(
    Auth(_auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    delete_workflow(&db, id).await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Run handlers
// ---------------------------------------------------------------------------

async fn create_run_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<CreateRunRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let req = CreateRunRequest {
        user_id: Some(auth.user_id),
        ..body
    };
    let run = create_run(&db, req).await?;
    Ok((StatusCode::CREATED, Json(json!(run))))
}

async fn list_runs_handler(
    Auth(_auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<ListRunsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(100).min(1000);
    let runs = list_runs(&db, params.workflow_id, params.status.as_deref(), limit).await?;
    Ok(Json(json!({ "runs": runs })))
}

async fn get_run_handler(
    Auth(_auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let run = get_run(&db, id).await?;
    Ok(Json(json!(run)))
}

async fn cancel_run_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let cancelled = cancel_run(&db, id, auth.user_id).await?;
    Ok(Json(json!({ "ok": cancelled })))
}

async fn get_steps_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let steps = get_steps(&db, id, auth.user_id).await?;
    Ok(Json(json!({ "steps": steps })))
}

async fn get_logs_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
    Query(params): Query<GetLogsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(500).min(2000);
    let logs = get_logs(
        &db,
        id,
        params.step_id,
        params.level.as_deref(),
        limit,
        auth.user_id,
    )
    .await?;
    Ok(Json(json!({ "logs": logs })))
}

// ---------------------------------------------------------------------------
// Step handlers
// ---------------------------------------------------------------------------

async fn complete_step_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
    Json(body): Json<CompleteStepBody>,
) -> Result<Json<Value>, AppError> {
    complete_step(&db, id, body.output, auth.user_id).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn fail_step_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
    Json(body): Json<FailStepBody>,
) -> Result<Json<Value>, AppError> {
    fail_step(&db, id, &body.error, auth.user_id).await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

async fn get_stats_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
) -> Result<Json<Value>, AppError> {
    let stats = get_stats(&db, Some(auth.user_id)).await?;
    Ok(Json(json!(stats)))
}
