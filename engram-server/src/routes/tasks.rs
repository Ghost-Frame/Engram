use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use engram_lib::services::chiasm::{
    create_task, delete_task, get_feed as get_task_feed, get_stats as get_task_stats, get_task,
    list_tasks, update_task, CreateTaskRequest, UpdateTaskRequest,
};

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
        .route("/feed", get(get_feed))
}

#[derive(Debug, Deserialize)]
struct ListTasksParams {
    agent: Option<String>,
    project: Option<String>,
    status: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct CreateTaskBody {
    title: String,
    description: Option<String>,
    status: Option<String>,
    priority: Option<i32>,
    agent: Option<String>,
    project: Option<String>,
    tags: Option<Vec<String>>,
    metadata: Option<serde_json::Value>,
    due_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpdateTaskBody {
    title: Option<String>,
    description: Option<String>,
    summary: Option<String>,
    status: Option<String>,
    priority: Option<i32>,
    agent: Option<String>,
    project: Option<String>,
    tags: Option<Vec<String>>,
    metadata: Option<serde_json::Value>,
    due_at: Option<String>,
}

async fn list_tasks_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ListTasksParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(500);
    let offset = params.offset.unwrap_or(0);

    // list_tasks signature: (db, user_id, status, limit, offset)
    // No agent/project filter in the lib -- filter client-side or just use user_id
    let mut tasks = list_tasks(
        &state.db,
        Some(auth.user_id),
        params.status.as_deref(),
        limit,
        offset,
    )
    .await?;

    // Apply agent/project filters in-memory since lib doesn't support them
    if let Some(ref agent) = params.agent {
        tasks.retain(|t| t.agent.as_deref() == Some(agent.as_str()));
    }
    if let Some(ref project) = params.project {
        tasks.retain(|t| t.project.as_deref() == Some(project.as_str()));
    }

    Ok(Json(json!({ "tasks": tasks, "count": tasks.len() })))
}

async fn create_task_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<CreateTaskBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let req = CreateTaskRequest {
        title: body.title,
        description: body.description,
        status: body.status,
        priority: body.priority,
        agent: body.agent,
        project: body.project,
        tags: body.tags,
        metadata: body.metadata,
        user_id: Some(auth.user_id),
        due_at: body.due_at,
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
    Auth(_auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let task = get_task(&state.db, id).await?;
    Ok(Json(json!(task)))
}

async fn update_task_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<UpdateTaskBody>,
) -> Result<Json<Value>, AppError> {
    let req = UpdateTaskRequest {
        title: body.title,
        description: body.summary.or(body.description),
        status: body.status,
        priority: body.priority,
        agent: body.agent,
        project: body.project,
        tags: body.tags,
        metadata: body.metadata,
        due_at: body.due_at,
    };

    let task = update_task(&state.db, id, req).await?;
    Ok(Json(json!(task)))
}

async fn delete_task_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    delete_task(&state.db, id).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn get_feed(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ListTasksParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(100);
    let offset = params.offset.unwrap_or(0);
    let items = get_task_feed(&state.db, auth.user_id, limit, offset).await?;
    let total = get_task_stats(&state.db, Some(auth.user_id)).await?.total;
    Ok(Json(json!({ "items": items, "total": total })))
}
