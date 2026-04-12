use axum::extract::Path;
use axum::{
    extract::{Query, State},
    routing::{get, post},
    Json, Router,
};
use engram_lib::auth::Scope;
use engram_lib::jobs::{
    self, cleanup_jobs, count_failed_jobs, list_failed_jobs, list_pending_jobs, list_running_jobs,
    purge_failed_jobs, retry_failed_job,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{error::AppError, extractors::Auth, state::AppState};

#[derive(Debug, Deserialize)]
struct ListJobsQuery {
    status: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn list_jobs(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ListJobsQuery>,
) -> Result<Json<Value>, AppError> {
    // Admin only
    if !auth.has_scope(&Scope::Admin) {
        return Err(AppError::from(engram_lib::EngError::Auth(
            "Admin required".into(),
        )));
    }

    let status = params.status.as_deref().unwrap_or("failed");
    let limit = params.limit.unwrap_or(50).min(200);
    let offset = params.offset.unwrap_or(0);

    let (jobs, total) = match status {
        "failed" => {
            let jobs = list_failed_jobs(&state.db.conn, limit, offset).await?;
            let total = count_failed_jobs(&state.db.conn).await?;
            (jobs, total)
        }
        "pending" => {
            let jobs = list_pending_jobs(&state.db.conn, limit, offset).await?;
            let total = jobs.len() as i64;
            (jobs, total)
        }
        "running" => {
            let jobs = list_running_jobs(&state.db.conn).await?;
            let total = jobs.len() as i64;
            (jobs, total)
        }
        _ => {
            return Err(AppError::from(engram_lib::EngError::InvalidInput(
                "Invalid status. Use: failed, pending, running".into(),
            )));
        }
    };

    // Parse payloads to JSON
    let jobs_json: Vec<Value> = jobs
        .into_iter()
        .map(|j| {
            let payload: Value = serde_json::from_str(&j.payload).unwrap_or(json!({}));
            json!({
                "id": j.id,
                "type": j.job_type,
                "payload": payload,
                "status": j.status.as_str(),
                "attempts": j.attempts,
                "max_attempts": j.max_attempts,
                "error": j.error,
                "created_at": j.created_at,
                "claimed_at": j.claimed_at,
                "completed_at": j.completed_at,
                "next_retry_at": j.next_retry_at,
            })
        })
        .collect();

    Ok(Json(json!({
        "jobs": jobs_json,
        "total": total,
        "limit": limit,
        "offset": offset,
        "status": status,
    })))
}

#[derive(Debug, Deserialize)]
struct RetryBody {
    id: i64,
}

async fn retry_job_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<RetryBody>,
) -> Result<Json<Value>, AppError> {
    // Admin only
    if !auth.has_scope(&Scope::Admin) {
        return Err(AppError::from(engram_lib::EngError::Auth(
            "Admin required".into(),
        )));
    }

    let retried = retry_failed_job(&state.db.conn, body.id).await?;
    if !retried {
        return Err(AppError::from(engram_lib::EngError::NotFound(
            "Job not found or not in failed state".into(),
        )));
    }

    Ok(Json(json!({ "retried": true, "id": body.id })))
}

#[derive(Debug, Deserialize)]
struct PurgeBody {
    older_than_days: Option<i64>,
}

async fn purge_jobs_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<PurgeBody>,
) -> Result<Json<Value>, AppError> {
    // Admin only
    if !auth.has_scope(&Scope::Admin) {
        return Err(AppError::from(engram_lib::EngError::Auth(
            "Admin required".into(),
        )));
    }

    let days = body.older_than_days.unwrap_or(7);
    let purged = purge_failed_jobs(&state.db.conn, days).await?;

    Ok(Json(json!({
        "purged": purged,
        "older_than_days": days,
    })))
}

async fn job_stats_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    // Admin only
    if !auth.has_scope(&Scope::Admin) {
        return Err(AppError::from(engram_lib::EngError::Auth(
            "Admin required".into(),
        )));
    }

    let stats = jobs::get_job_stats(&state.db.conn).await?;
    Ok(Json(json!(stats)))
}

// --- New handlers for P0-0 Phase 27c ---

#[derive(Debug, Deserialize)]
struct PaginationQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn list_pending_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<PaginationQuery>,
) -> Result<Json<Value>, AppError> {
    if !auth.has_scope(&Scope::Admin) {
        return Err(AppError::from(engram_lib::EngError::Auth(
            "Admin required".into(),
        )));
    }
    let limit = params.limit.unwrap_or(50).min(200);
    let offset = params.offset.unwrap_or(0);
    let jobs = list_pending_jobs(&state.db.conn, limit, offset).await?;
    let jobs_json: Vec<Value> = jobs
        .into_iter()
        .map(|j| {
            let payload: Value = serde_json::from_str(&j.payload).unwrap_or(json!({}));
            json!({
                "id": j.id,
                "type": j.job_type,
                "payload": payload,
                "status": j.status.as_str(),
                "attempts": j.attempts,
                "max_attempts": j.max_attempts,
                "created_at": j.created_at,
            })
        })
        .collect();
    Ok(Json(json!({ "jobs": jobs_json, "count": jobs_json.len() })))
}

async fn list_running_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    if !auth.has_scope(&Scope::Admin) {
        return Err(AppError::from(engram_lib::EngError::Auth(
            "Admin required".into(),
        )));
    }
    let jobs = list_running_jobs(&state.db.conn).await?;
    let jobs_json: Vec<Value> = jobs
        .into_iter()
        .map(|j| {
            let payload: Value = serde_json::from_str(&j.payload).unwrap_or(json!({}));
            json!({
                "id": j.id,
                "type": j.job_type,
                "payload": payload,
                "status": j.status.as_str(),
                "attempts": j.attempts,
                "max_attempts": j.max_attempts,
                "created_at": j.created_at,
                "claimed_at": j.claimed_at,
            })
        })
        .collect();
    Ok(Json(json!({ "jobs": jobs_json, "count": jobs_json.len() })))
}

async fn list_failed_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<PaginationQuery>,
) -> Result<Json<Value>, AppError> {
    if !auth.has_scope(&Scope::Admin) {
        return Err(AppError::from(engram_lib::EngError::Auth(
            "Admin required".into(),
        )));
    }
    let limit = params.limit.unwrap_or(50).min(200);
    let offset = params.offset.unwrap_or(0);
    let jobs = list_failed_jobs(&state.db.conn, limit, offset).await?;
    let total = count_failed_jobs(&state.db.conn).await?;
    let jobs_json: Vec<Value> = jobs
        .into_iter()
        .map(|j| {
            let payload: Value = serde_json::from_str(&j.payload).unwrap_or(json!({}));
            json!({
                "id": j.id,
                "type": j.job_type,
                "payload": payload,
                "status": j.status.as_str(),
                "attempts": j.attempts,
                "max_attempts": j.max_attempts,
                "error": j.error,
                "created_at": j.created_at,
                "completed_at": j.completed_at,
            })
        })
        .collect();
    Ok(Json(
        json!({ "jobs": jobs_json, "total": total, "count": jobs_json.len() }),
    ))
}

async fn retry_job_by_id_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    if !auth.has_scope(&Scope::Admin) {
        return Err(AppError::from(engram_lib::EngError::Auth(
            "Admin required".into(),
        )));
    }
    let retried = retry_failed_job(&state.db.conn, id).await?;
    if !retried {
        return Err(AppError::from(engram_lib::EngError::NotFound(
            "Job not found or not in failed state".into(),
        )));
    }
    Ok(Json(json!({ "retried": true, "id": id })))
}

#[derive(Debug, Deserialize)]
struct CleanupBody {
    older_than_days: Option<i64>,
}

async fn cleanup_jobs_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<CleanupBody>,
) -> Result<Json<Value>, AppError> {
    if !auth.has_scope(&Scope::Admin) {
        return Err(AppError::from(engram_lib::EngError::Auth(
            "Admin required".into(),
        )));
    }
    let days = body.older_than_days.unwrap_or(30);
    let deleted = cleanup_jobs(&state.db.conn, days).await?;
    Ok(Json(json!({
        "deleted": deleted,
        "older_than_days": days,
    })))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/jobs", get(list_jobs))
        .route("/jobs/pending", get(list_pending_handler))
        .route("/jobs/running", get(list_running_handler))
        .route("/jobs/failed", get(list_failed_handler))
        .route("/jobs/retry", post(retry_job_handler))
        .route("/jobs/{id}/retry", post(retry_job_by_id_handler))
        .route("/jobs/purge", post(purge_jobs_handler))
        .route("/jobs/cleanup", post(cleanup_jobs_handler))
        .route("/jobs/stats", get(job_stats_handler))
}
