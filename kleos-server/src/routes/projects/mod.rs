use axum::{
    extract::{Path, Query},
    routing::{get, post, put},
    Json, Router,
};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::{Auth, ResolvedDb};
use crate::state::AppState;

mod types;
use types::StatusQuery;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects", post(create_project).get(list_projects))
        .route(
            "/projects/{id}",
            get(get_project)
                .put(update_project)
                .delete(delete_project_handler),
        )
        .route(
            "/projects/{id}/memories/{mid}",
            put(link_memory).delete(unlink_memory),
        )
}

async fn create_project(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<kleos_lib::projects::CreateProjectBody>,
) -> Result<Json<Value>, AppError> {
    let name = body.name.as_deref().unwrap_or("").trim();
    if name.is_empty() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "name is required".into(),
        )));
    }
    let status = body.status.as_deref().unwrap_or("active");
    let status = if kleos_lib::projects::VALID_PROJECT_STATUSES.contains(&status) {
        status
    } else {
        "active"
    };
    let metadata = body
        .metadata
        .as_ref()
        .map(|m| serde_json::to_string(m).unwrap_or_default());
    let (id, created_at) = kleos_lib::projects::create_project(
        &db,
        name,
        body.description.as_deref(),
        status,
        metadata.as_deref(),
        auth.user_id,
    )
    .await?;
    Ok(Json(
        json!({ "created": true, "id": id, "name": name, "status": status, "created_at": created_at }),
    ))
}

async fn list_projects(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(q): Query<StatusQuery>,
) -> Result<Json<Value>, AppError> {
    let projects =
        kleos_lib::projects::list_projects(&db, auth.user_id, q.status.as_deref()).await?;
    let count = projects.len();
    Ok(Json(json!({ "projects": projects, "count": count })))
}

async fn get_project(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let project = kleos_lib::projects::get_project(&db, id, auth.user_id)
        .await?
        .ok_or_else(|| AppError(kleos_lib::EngError::NotFound("Project not found".into())))?;
    let memory_ids = kleos_lib::projects::get_project_memory_ids(&db, id, auth.user_id).await?;
    Ok(Json(
        json!({ "id": project.id, "name": project.name, "description": project.description, "status": project.status, "metadata": project.metadata, "memory_ids": memory_ids, "created_at": project.created_at }),
    ))
}

async fn update_project(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
    Json(body): Json<kleos_lib::projects::UpdateProjectBody>,
) -> Result<Json<Value>, AppError> {
    let metadata = body
        .metadata
        .as_ref()
        .map(|m| serde_json::to_string(m).unwrap_or_default());
    kleos_lib::projects::update_project(
        &db,
        id,
        auth.user_id,
        body.name.as_deref(),
        body.description.as_deref(),
        body.status.as_deref(),
        metadata.as_deref(),
    )
    .await?;
    Ok(Json(json!({ "updated": true, "id": id })))
}

async fn delete_project_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    kleos_lib::projects::delete_project(&db, id, auth.user_id).await?;
    Ok(Json(json!({ "deleted": true, "id": id })))
}

async fn link_memory(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path((id, mid)): Path<(i64, i64)>,
) -> Result<Json<Value>, AppError> {
    kleos_lib::projects::link_memory(&db, mid, id, auth.user_id).await?;
    Ok(Json(
        json!({ "linked": true, "project_id": id, "memory_id": mid }),
    ))
}

async fn unlink_memory(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path((id, mid)): Path<(i64, i64)>,
) -> Result<Json<Value>, AppError> {
    kleos_lib::projects::unlink_memory(&db, mid, id, auth.user_id).await?;
    Ok(Json(
        json!({ "unlinked": true, "project_id": id, "memory_id": mid }),
    ))
}
