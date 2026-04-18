use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use kleos_lib::services::chiasm::{
    create_task, delete_task, get_feed as get_task_feed, get_stats as get_task_stats, get_task,
    list_task_history, list_tasks, update_task, CreateTaskRequest, UpdateTaskRequest,
};

mod types;
use types::{CreateTaskBody, HistoryParams, ListTasksParams, UpdateTaskBody};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/tasks", get(list_tasks_handler).post(create_task_handler))
        .route("/tasks/stats", get(get_stats))
        .route(
            "/tasks/{id}",
            get(get_task_handler)
                .patch(update_task_handler)
                .delete(delete_task_handler),
        )
        .route("/tasks/{id}/history", get(get_task_history_handler))
        .route("/feed", get(get_feed))
}

async fn list_tasks_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ListTasksParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(500).min(1000);
    let offset = params.offset.unwrap_or(0);

    let tasks = list_tasks(
        &state.db,
        auth.user_id,
        params.status.as_deref(),
        params.agent.as_deref(),
        params.project.as_deref(),
        limit,
        offset,
    )
    .await?;

    Ok(Json(json!({ "tasks": tasks, "count": tasks.len() })))
}

async fn create_task_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<CreateTaskBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let req = CreateTaskRequest {
        agent: body.agent,
        project: body.project,
        title: body.title,
        status: body.status,
        summary: body.summary,
        user_id: Some(auth.user_id),
    };

    let task = create_task(&state.db, req).await?;
    Ok((StatusCode::CREATED, Json(json!(task))))
}

async fn get_stats(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let stats = get_task_stats(&state.db, Some(auth.user_id)).await?;
    Ok(Json(json!(stats)))
}

async fn get_task_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let task = get_task(&state.db, id, auth.user_id).await?;
    Ok(Json(json!(task)))
}

async fn update_task_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<UpdateTaskBody>,
) -> Result<Json<Value>, AppError> {
    let req = UpdateTaskRequest {
        title: body.title,
        status: body.status,
        summary: body.summary,
        agent: body.agent,
    };

    let task = update_task(&state.db, id, req, auth.user_id).await?;
    Ok(Json(json!(task)))
}

async fn get_task_history_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Query(params): Query<HistoryParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(100).min(1000);
    let history = list_task_history(&state.db, id, auth.user_id, limit).await?;
    Ok(Json(json!({ "history": history, "count": history.len() })))
}

async fn delete_task_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    delete_task(&state.db, id, auth.user_id).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn get_feed(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ListTasksParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(100).min(1000);
    let offset = params.offset.unwrap_or(0);
    let items = get_task_feed(&state.db, auth.user_id, limit, offset).await?;
    let total = get_task_stats(&state.db, Some(auth.user_id)).await?.total;
    Ok(Json(json!({ "items": items, "total": total })))
}
