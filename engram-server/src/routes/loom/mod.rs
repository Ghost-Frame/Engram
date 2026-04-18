use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
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
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let workflows = list_workflows(&state.db, auth.user_id).await?;
    Ok(Json(json!({ "workflows": workflows })))
}

async fn create_workflow_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<CreateWorkflowRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let req = CreateWorkflowRequest {
        user_id: Some(auth.user_id),
        ..body
    };
    let workflow = create_workflow(&state.db, req).await?;
    Ok((StatusCode::CREATED, Json(json!(workflow))))
}

async fn get_workflow_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let workflow = get_workflow(&state.db, id, auth.user_id).await?;
    Ok(Json(json!(workflow)))
}

async fn update_workflow_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<UpdateWorkflowRequest>,
) -> Result<Json<Value>, AppError> {
    let workflow = update_workflow(&state.db, id, auth.user_id, body).await?;
    Ok(Json(json!(workflow)))
}

async fn delete_workflow_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    delete_workflow(&state.db, id, auth.user_id).await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Run handlers
// ---------------------------------------------------------------------------

async fn create_run_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<CreateRunRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let req = CreateRunRequest {
        user_id: Some(auth.user_id),
        ..body
    };
    let run = create_run(&state.db, req).await?;
    Ok((StatusCode::CREATED, Json(json!(run))))
}

async fn list_runs_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ListRunsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(100).min(1000);
    let runs = list_runs(
        &state.db,
        auth.user_id,
        params.workflow_id,
        params.status.as_deref(),
        limit,
    )
    .await?;
    Ok(Json(json!({ "runs": runs })))
}

async fn get_run_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let run = get_run(&state.db, id, auth.user_id).await?;
    Ok(Json(json!(run)))
}

async fn cancel_run_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let cancelled = cancel_run(&state.db, id, auth.user_id).await?;
    Ok(Json(json!({ "ok": cancelled })))
}

async fn get_steps_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let steps = get_steps(&state.db, id, auth.user_id).await?;
    Ok(Json(json!({ "steps": steps })))
}

async fn get_logs_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Query(params): Query<GetLogsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(500).min(2000);
    let logs = get_logs(
        &state.db,
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
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<CompleteStepBody>,
) -> Result<Json<Value>, AppError> {
    complete_step(&state.db, id, body.output, auth.user_id).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn fail_step_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<FailStepBody>,
) -> Result<Json<Value>, AppError> {
    fail_step(&state.db, id, &body.error, auth.user_id).await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

async fn get_stats_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let stats = get_stats(&state.db, Some(auth.user_id)).await?;
    Ok(Json(json!(stats)))
}
