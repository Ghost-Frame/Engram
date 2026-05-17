use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::{Auth, ResolvedDb};
use crate::state::AppState;
use kleos_lib::auth::{AuthContext, Scope};
use kleos_lib::services::chiasm::{
    create_task, delete_task, generate_plan, get_feed as get_task_feed,
    get_stats as get_task_stats, get_task, keys as agent_keys, list_task_history, list_tasks,
    submit_feedback, submit_output, update_task, CreateTaskRequest, UpdateTaskRequest,
};

#[derive(serde::Deserialize)]
struct CreateAgentKeyBody {
    agent: String,
}

fn require_admin(auth: &AuthContext) -> Result<(), AppError> {
    if !auth.has_scope(&Scope::Admin) {
        return Err(AppError(kleos_lib::EngError::Auth(
            "admin scope required".into(),
        )));
    }
    Ok(())
}

mod types;
use types::{
    AddDepsBody, CheckConflictsBody, ClaimBody, ClaimsProjectParams, CreateClaimsBody,
    CreateTaskBody, EnqueueBody, HistoryParams, ListTasksParams, SubmitFeedbackBody,
    SubmitOutputBody, UpdateTaskBody,
};

/// Builds the Chiasm tasks router.
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
        // Chiasm aliases so agents using Syntheos naming can find tasks
        .route(
            "/chiasm/tasks",
            get(list_tasks_handler).post(create_task_handler),
        )
        .route("/chiasm/tasks/stats", get(get_stats))
        .route(
            "/chiasm/tasks/{id}",
            get(get_task_handler)
                .patch(update_task_handler)
                .delete(delete_task_handler),
        )
        .route("/chiasm/tasks/{id}/history", get(get_task_history_handler))
        .route("/chiasm/feed", get(get_feed))
        .route("/tasks/{id}/output", post(submit_output_handler))
        .route("/tasks/{id}/feedback", post(submit_feedback_handler))
        .route("/tasks/{id}/plan", post(generate_plan_handler))
        .route(
            "/tasks/{id}/dependencies",
            get(list_deps_handler).post(add_deps_handler),
        )
        .route(
            "/tasks/{id}/dependencies/{dep_id}",
            delete(remove_dep_handler),
        )
        .route("/chiasm/tasks/{id}/output", post(submit_output_handler))
        .route("/chiasm/tasks/{id}/feedback", post(submit_feedback_handler))
        .route("/chiasm/tasks/{id}/plan", post(generate_plan_handler))
        .route(
            "/chiasm/tasks/{id}/dependencies",
            get(list_deps_handler).post(add_deps_handler),
        )
        .route(
            "/chiasm/tasks/{id}/dependencies/{dep_id}",
            delete(remove_dep_handler),
        )
        .route(
            "/tasks/{id}/claims",
            get(list_task_claims_handler)
                .post(create_claims_handler)
                .delete(release_claims_handler),
        )
        .route("/claims/check", post(check_conflicts_handler))
        .route("/claims", get(list_project_claims_handler))
        .route(
            "/chiasm/tasks/{id}/claims",
            get(list_task_claims_handler)
                .post(create_claims_handler)
                .delete(release_claims_handler),
        )
        .route("/chiasm/claims/check", post(check_conflicts_handler))
        .route("/chiasm/claims", get(list_project_claims_handler))
        // Heartbeat
        .route("/tasks/{id}/heartbeat", post(heartbeat_handler))
        .route("/chiasm/tasks/{id}/heartbeat", post(heartbeat_handler))
        // Queue
        .route("/queue", post(enqueue_handler))
        .route("/queue/claim", post(claim_handler))
        .route("/chiasm/queue", post(enqueue_handler))
        .route("/chiasm/queue/claim", post(claim_handler))
        // Admin: per-agent bearer keys (mirrors standalone chiasm /admin/keys)
        .route(
            "/chiasm/admin/keys",
            post(create_agent_key_handler).get(list_agent_keys_handler),
        )
        .route("/chiasm/admin/keys/{id}", delete(revoke_agent_key_handler))
}

/// Lists tasks with optional filters.
async fn list_tasks_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Query(params): Query<ListTasksParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(500).min(1000);
    let offset = params.offset.unwrap_or(0);

    let tasks = list_tasks(
        &db,
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

/// Creates a new task.
async fn create_task_handler(
    ResolvedDb(db): ResolvedDb,
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
        expected_output: body.expected_output,
        output_format: body.output_format,
        condition: body.condition,
        guardrail_url: body.guardrail_url,
        heartbeat_interval: body.heartbeat_interval,
    };

    let task = create_task(&db, req).await?;
    Ok((StatusCode::CREATED, Json(json!(task))))
}

/// Returns task statistics.
async fn get_stats(ResolvedDb(db): ResolvedDb, Auth(_auth): Auth) -> Result<Json<Value>, AppError> {
    let stats = get_task_stats(&db).await?;
    Ok(Json(json!(stats)))
}

/// Retrieves a single task by ID.
async fn get_task_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let task = get_task(&db, id, auth.user_id).await?;
    Ok(Json(json!(task)))
}

/// Partially updates a task.
async fn update_task_handler(
    ResolvedDb(db): ResolvedDb,
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

    let task = update_task(&db, id, req, auth.user_id).await?;
    Ok(Json(json!(task)))
}

/// Lists history entries for a task.
async fn get_task_history_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Query(params): Query<HistoryParams>,
) -> Result<Json<Value>, AppError> {
    let _ = get_task(&db, id, auth.user_id).await?;
    let limit = params.limit.unwrap_or(100).min(1000);
    let history = list_task_history(&db, id, auth.user_id, limit).await?;
    Ok(Json(json!({ "history": history, "count": history.len() })))
}

/// Deletes a task.
async fn delete_task_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    delete_task(&db, id).await?;
    Ok(Json(json!({ "ok": true })))
}

/// Returns the task activity feed.
async fn get_feed(
    ResolvedDb(db): ResolvedDb,
    Auth(_auth): Auth,
    Query(params): Query<ListTasksParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(100).min(1000);
    let offset = params.offset.unwrap_or(0);
    let items = get_task_feed(&db, limit, offset).await?;
    let total = get_task_stats(&db).await?.total;
    Ok(Json(json!({ "items": items, "total": total })))
}

/// Submit output for a task.
async fn submit_output_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<SubmitOutputBody>,
) -> Result<Json<Value>, AppError> {
    let task = submit_output(&db, id, &body.output, auth.user_id).await?;
    Ok(Json(json!(task)))
}

/// Submit feedback for a task, resetting it to active.
async fn submit_feedback_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<SubmitFeedbackBody>,
) -> Result<Json<Value>, AppError> {
    let task = submit_feedback(&db, id, &body.feedback, auth.user_id).await?;
    Ok(Json(json!(task)))
}

/// Generate an LLM execution plan for a task. POST body is ignored (the
/// standalone accepts `{}`); we keep the same contract so existing chiasm
/// clients drop in unchanged.
async fn generate_plan_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let task = generate_plan(&db, id, auth.user_id).await?;
    Ok(Json(json!(task)))
}

/// List all dependencies for a task.
async fn list_deps_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let deps = kleos_lib::services::chiasm::dependencies::get_dependencies(&db, id).await?;
    Ok(Json(json!({ "dependencies": deps, "count": deps.len() })))
}

/// Add dependencies to a task with circular dependency detection.
async fn add_deps_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<AddDepsBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    kleos_lib::services::chiasm::dependencies::add_dependencies(&db, id, &body.depends_on).await?;
    let deps = kleos_lib::services::chiasm::dependencies::get_dependencies(&db, id).await?;
    Ok((StatusCode::CREATED, Json(json!({ "dependencies": deps }))))
}

/// Remove a single dependency edge.
async fn remove_dep_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(_auth): Auth,
    Path((id, dep_id)): Path<(i64, i64)>,
) -> Result<Json<Value>, AppError> {
    let removed =
        kleos_lib::services::chiasm::dependencies::remove_dependency(&db, id, dep_id).await?;
    Ok(Json(json!({ "removed": removed })))
}

/// Create path claims for a task.
async fn create_claims_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<CreateClaimsBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let ttl = body.ttl_seconds.unwrap_or(1800);
    let path_refs: Vec<&str> = body.paths.iter().map(|s| s.as_str()).collect();
    let claims = kleos_lib::services::chiasm::claims::create_claims(
        &db,
        id,
        &body.agent,
        &body.project,
        &path_refs,
        ttl,
    )
    .await?;
    let count = claims.len();
    Ok((
        StatusCode::CREATED,
        Json(json!({ "claims": claims, "count": count })),
    ))
}

/// List active claims for a task.
async fn list_task_claims_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let claims = kleos_lib::services::chiasm::claims::get_claims_for_task(&db, id).await?;
    let count = claims.len();
    Ok(Json(json!({ "claims": claims, "count": count })))
}

/// Release all claims for a task.
async fn release_claims_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let released = kleos_lib::services::chiasm::claims::release_claims(&db, id).await?;
    Ok(Json(json!({ "released": released })))
}

/// Check for path conflicts before creating claims.
async fn check_conflicts_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(_auth): Auth,
    Json(body): Json<CheckConflictsBody>,
) -> Result<Json<Value>, AppError> {
    let path_refs: Vec<&str> = body.paths.iter().map(|s| s.as_str()).collect();
    let conflicts = kleos_lib::services::chiasm::claims::check_conflicts(
        &db,
        &body.project,
        &path_refs,
        body.exclude_task_id,
    )
    .await?;
    let has_conflicts = !conflicts.is_empty();
    Ok(Json(
        json!({ "conflicts": conflicts, "has_conflicts": has_conflicts }),
    ))
}

/// List all active claims in a project.
async fn list_project_claims_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(_auth): Auth,
    Query(params): Query<ClaimsProjectParams>,
) -> Result<Json<Value>, AppError> {
    let claims =
        kleos_lib::services::chiasm::claims::get_claims_for_project(&db, &params.project).await?;
    let count = claims.len();
    Ok(Json(json!({ "claims": claims, "count": count })))
}

/// Record a heartbeat for a task, refreshing its liveness timestamp.
async fn heartbeat_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    kleos_lib::services::chiasm::heartbeat::record_heartbeat(&db, id).await?;
    Ok(Json(json!({ "ok": true })))
}

/// Enqueue a new unassigned task into the work queue.
async fn enqueue_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Json(body): Json<EnqueueBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let task = kleos_lib::services::chiasm::queue::enqueue_task(
        &db,
        &body.project,
        &body.title,
        body.summary.as_deref(),
        auth.user_id,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(json!(task))))
}

/// Claim the next available queued task for an agent.
async fn claim_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Json(body): Json<ClaimBody>,
) -> Result<Json<Value>, AppError> {
    let task = kleos_lib::services::chiasm::queue::claim_next_task(
        &db,
        &body.agent,
        body.project.as_deref(),
        auth.user_id,
    )
    .await?;
    Ok(Json(json!({ "task": task })))
}

/// Create a per-agent bearer key. Admin only; the raw key is shown exactly
/// once in the response (`key` field) -- the server only persists its
/// SHA-256 hash.
async fn create_agent_key_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Json(body): Json<CreateAgentKeyBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    require_admin(&auth)?;
    let created = agent_keys::create_key(&db, &body.agent).await?;
    Ok((StatusCode::CREATED, Json(json!(created))))
}

/// List every stored key (active or revoked) with no secrets.
async fn list_agent_keys_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let keys = agent_keys::list_keys(&db).await?;
    Ok(Json(json!({ "keys": keys })))
}

/// Mark a key as revoked. Idempotent: revoking an already-revoked key
/// returns 404 so callers can distinguish "no such key" from "already done".
async fn revoke_agent_key_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    require_admin(&auth)?;
    let revoked = agent_keys::revoke_key(&db, id).await?;
    if !revoked {
        return Err(AppError(kleos_lib::EngError::NotFound(format!(
            "key {}",
            id
        ))));
    }
    Ok(Json(json!({ "ok": true, "revoked": true })))
}
