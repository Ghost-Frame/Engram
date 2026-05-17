//! Route handlers for the `/soma/*` namespace.
//!
//! Exposes the Soma agent registry over HTTP: agent CRUD, heartbeats, group
//! management, per-agent logs, stats, stale-agent queries, and capability
//! search. All routes require authentication via the [`Auth`] extractor and
//! tenant-scoped database access via [`ResolvedDb`].

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::{Auth, ResolvedDb};
use crate::state::AppState;
use kleos_lib::services::soma::{
    add_agent_to_group, create_group, delete_agent, delete_group, find_by_capability, get_agent,
    get_group, get_group_members, get_stale_agents, get_stats as get_soma_stats, heartbeat,
    list_agent_logs, list_agents, list_groups, log_event, register_agent, remove_agent_from_group,
    set_status, update_agent_quality, RegisterAgentRequest,
};

mod types;
use types::{
    AddMemberBody, CreateAgentBody, CreateGroupBody, HeartbeatBody, ListAgentsParams,
    ListLogsParams, LogEventBody, StaleAgentsParams, UpdateAgentBody, UpdateQualityBody,
};

/// Build and return the soma sub-router. Mount this under the server's root
/// router; the paths are absolute (e.g. `/soma/agents`).
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/soma/agents",
            post(create_agent_handler).get(list_agents_handler),
        )
        // NOTE: /stale and /capability/{name} must appear BEFORE /agents/{id}
        // because axum resolves routes in declaration order and would otherwise
        // match the literal segments "stale" and "capability" as id values.
        .route("/soma/agents/stale", get(get_stale_agents_handler))
        .route(
            "/soma/agents/capability/{name}",
            get(find_by_capability_handler),
        )
        .route(
            "/soma/agents/{id}",
            get(get_agent_handler)
                .patch(update_agent_handler)
                .delete(delete_agent_handler),
        )
        .route("/soma/agents/{id}/heartbeat", post(heartbeat_handler))
        .route(
            "/soma/agents/{id}/quality",
            axum::routing::patch(update_quality_handler),
        )
        .route(
            "/soma/agents/{id}/log",
            post(log_event_handler).get(list_logs_handler),
        )
        .route(
            "/soma/agents/{id}/logs",
            post(log_event_handler).get(list_logs_handler),
        )
        .route(
            "/soma/groups",
            post(create_group_handler).get(list_groups_handler),
        )
        .route(
            "/soma/groups/{id}",
            get(get_group_handler).delete(delete_group_handler),
        )
        .route(
            "/soma/groups/{id}/members",
            get(list_group_members_handler).post(add_member_handler),
        )
        .route(
            "/soma/groups/{id}/members/{agent_id}",
            axum::routing::delete(remove_member_handler),
        )
        .route("/soma/stats", get(get_stats))
}

/// Handler for `POST /soma/agents`.
///
/// Registers a new agent or upserts an existing registration by name. Returns
/// HTTP 201 with the full agent row on success.
async fn create_agent_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Json(body): Json<CreateAgentBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let type_ = body
        .r#type
        .ok_or_else(|| kleos_lib::EngError::InvalidInput("type is required".into()))?;

    let req = RegisterAgentRequest {
        user_id: Some(auth.user_id),
        name: body.name,
        type_,
        description: body.description,
        capabilities: body.capabilities,
        config: body.config,
    };

    let agent = register_agent(&db, req).await?;
    Ok((StatusCode::CREATED, Json(json!(agent))))
}

/// Handler for `GET /soma/agents`.
///
/// Lists agents scoped to the authenticated tenant. Supports optional `type`,
/// `status`, and `limit` query parameters. Returns `{ agents: [...], count: N }`.
async fn list_agents_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Query(params): Query<ListAgentsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(100).min(1000);
    let agents = list_agents(
        &db,
        auth.user_id,
        params.agent_type.as_deref(),
        params.status.as_deref(),
        limit,
    )
    .await?;

    let agents = if let Some(ref cap) = params.capability {
        agents
            .into_iter()
            .filter(|a| {
                a.capabilities
                    .as_array()
                    .map(|arr| arr.iter().any(|v| v.as_str() == Some(cap)))
                    .unwrap_or(false)
            })
            .collect()
    } else {
        agents
    };

    Ok(Json(json!({ "agents": agents, "count": agents.len() })))
}

/// Handler for `GET /soma/agents/{id}`.
///
/// Returns the full agent row for the given numeric `id`. Returns 404 when no
/// agent with that id exists in the tenant's shard.
async fn get_agent_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let agent = get_agent(&db, id, auth.user_id).await?;
    Ok(Json(json!(agent)))
}

/// Handler for `PATCH /soma/agents/{id}`.
///
/// Partially updates an agent. Structural fields (type, description,
/// capabilities, config) are merged via the register_agent upsert. The
/// `status` field is applied separately via `set_status`. Returns the updated
/// agent row.
async fn update_agent_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<UpdateAgentBody>,
) -> Result<Json<Value>, AppError> {
    let existing = get_agent(&db, id, auth.user_id).await?;

    if body.r#type.is_some()
        || body.description.is_some()
        || body.capabilities.is_some()
        || body.config.is_some()
    {
        let type_ = body.r#type.unwrap_or(existing.type_.clone());
        let description = body.description.or(existing.description.clone());
        let capabilities = body.capabilities.or(Some(existing.capabilities.clone()));
        let config = body.config.or(Some(existing.config.clone()));
        register_agent(
            &db,
            RegisterAgentRequest {
                user_id: Some(auth.user_id),
                name: existing.name.clone(),
                type_,
                description,
                capabilities,
                config,
            },
        )
        .await?;
    }

    if let Some(status) = body.status.as_deref() {
        set_status(&db, id, status).await?;
    }

    let agent = get_agent(&db, id, auth.user_id).await?;
    Ok(Json(json!(agent)))
}

/// Handler for `DELETE /soma/agents/{id}`.
///
/// Permanently removes the agent row for the given `id`. Returns
/// `{ "ok": true }` on success (idempotent -- does not 404 on missing id).
async fn delete_agent_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    delete_agent(&db, id).await?;
    Ok(Json(json!({ "ok": true })))
}

/// Handler for `POST /soma/agents/{id}/heartbeat`.
///
/// Records a heartbeat for the agent. When the body carries an optional
/// `status` field, that value overrides the default `offline -> online`
/// transition (validated against the allowed status set in the service layer).
/// An absent body or missing `status` keeps the legacy behavior. Returns
/// `{ "ok": true }`.
async fn heartbeat_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
    body: Option<Json<HeartbeatBody>>,
) -> Result<Json<Value>, AppError> {
    let status = body.and_then(|Json(b)| b.status);
    heartbeat(&db, id, status.as_deref()).await?;
    let agent = get_agent(&db, id, 0).await?;
    Ok(Json(json!(agent)))
}

/// Handler for `PATCH /soma/agents/{id}/quality`.
///
/// Updates the `quality_score` and/or `drift_flags` columns on an agent.
/// At least one field must be supplied; `drift_flags` must be a JSON array.
/// Returns the updated agent row.
async fn update_quality_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<UpdateQualityBody>,
) -> Result<Json<Value>, AppError> {
    let agent = update_agent_quality(&db, id, body.quality_score, body.drift_flags).await?;
    Ok(Json(json!(agent)))
}

/// Handler for `DELETE /soma/groups/{id}`.
///
/// Removes a group and cascades its membership rows. Returns
/// `{ "removed": bool }`; `removed` is `false` when no such group existed
/// for the caller's tenant.
async fn delete_group_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let removed = delete_group(&db, id, auth.user_id).await?;
    Ok(Json(json!({ "removed": removed })))
}

/// Handler for `GET /soma/groups/{id}`.
///
/// Returns the group row for the given numeric `id` scoped to the authenticated
/// tenant. Returns 404 when no group with that id exists for the tenant.
async fn get_group_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let group = get_group(&db, id, auth.user_id).await?;
    Ok(Json(json!(group)))
}

/// Handler for `GET /soma/groups/{id}/members`.
///
/// Lists all agents that are members of the given group. Returns
/// `{ members: [...], count: N }`.
async fn list_group_members_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(_auth): Auth,
    Path(group_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let members = get_group_members(&db, group_id).await?;
    let count = members.len();
    Ok(Json(json!({ "members": members, "count": count })))
}

/// Handler for `GET /soma/stats`.
///
/// Returns aggregate counts: total agents, online agents, and distinct type
/// count. Scoped to the caller's tenant shard.
async fn get_stats(ResolvedDb(db): ResolvedDb, Auth(_auth): Auth) -> Result<Json<Value>, AppError> {
    let stats = get_soma_stats(&db).await?;
    Ok(Json(json!(stats)))
}

// --- Handlers for P0-0 Phase 27c: groups and logs ---

/// Handler for `POST /soma/groups`.
///
/// Creates a new agent group scoped to the authenticated tenant. Returns
/// HTTP 201 with the full group row.
async fn create_group_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Json(body): Json<CreateGroupBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let group = create_group(&db, body.name, body.description, auth.user_id).await?;
    Ok((StatusCode::CREATED, Json(json!(group))))
}

/// Handler for `GET /soma/groups`.
///
/// Returns all groups belonging to the authenticated tenant, ordered
/// alphabetically by name. Returns `{ groups: [...], count: N }`.
async fn list_groups_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let groups = list_groups(&db, auth.user_id).await?;
    Ok(Json(json!({ "groups": groups, "count": groups.len() })))
}

/// Handler for `POST /soma/groups/{id}/members`.
///
/// Adds an agent to a group. The operation is idempotent (INSERT OR IGNORE).
/// Returns `{ ok: true, group_id, agent_id }`.
async fn add_member_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Path(group_id): Path<i64>,
    Json(body): Json<AddMemberBody>,
) -> Result<Json<Value>, AppError> {
    add_agent_to_group(&db, body.agent_id, group_id, auth.user_id).await?;
    Ok(Json(
        json!({ "ok": true, "group_id": group_id, "agent_id": body.agent_id }),
    ))
}

/// Handler for `DELETE /soma/groups/{id}/members/{agent_id}`.
///
/// Removes an agent from a group. Returns `{ removed: bool }` where
/// `removed` is `false` when the membership did not exist.
async fn remove_member_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Path((group_id, agent_id)): Path<(i64, i64)>,
) -> Result<Json<Value>, AppError> {
    let removed = remove_agent_from_group(&db, agent_id, group_id, auth.user_id).await?;
    Ok(Json(json!({ "removed": removed })))
}

/// Handler for `POST /soma/agents/{id}/log`.
///
/// Appends a structured log entry to the agent's log stream. Returns
/// HTTP 201 with `{ id: <new_log_id> }`.
///
/// SECURITY: relies on ResolvedDb shard isolation (Phase 5+) to scope to the
/// caller's tenant. Do not add `state.db` calls here without re-binding auth.
async fn log_event_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(_auth): Auth,
    Path(agent_id): Path<i64>,
    Json(body): Json<LogEventBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let id = log_event(&db, agent_id, &body.level, &body.message, body.data).await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

/// Handler for `GET /soma/agents/{id}/log` and `GET /soma/agents/{id}/logs`.
///
/// Returns the most recent log entries for an agent, newest first. Accepts an
/// optional `limit` query parameter (default 100, max 1000). Returns
/// `{ logs: [...], count: N }`.
async fn list_logs_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(_auth): Auth,
    Path(agent_id): Path<i64>,
    Query(params): Query<ListLogsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(100).min(1000);
    let logs = list_agent_logs(&db, agent_id, limit, params.level.as_deref()).await?;
    Ok(Json(json!({ "logs": logs, "count": logs.len() })))
}

// --- Agent stale-window and capability search ---

/// Handler for `GET /soma/agents/stale`.
///
/// Returns agents marked `online` whose last heartbeat is older than the
/// `minutes` window (default 5, clamped to [1, 1440]). The result feeds
/// external sweepers that decide when to mark an agent offline.
async fn get_stale_agents_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(_auth): Auth,
    Query(params): Query<StaleAgentsParams>,
) -> Result<Json<Value>, AppError> {
    let minutes = params.minutes.unwrap_or(5);
    let agents = get_stale_agents(&db, minutes).await?;
    Ok(Json(json!(agents)))
}

/// Handler for `GET /soma/agents/capability/{name}`.
///
/// Returns every agent whose `capabilities` array contains the exact `name`
/// string. Path-decoded; uses `LIKE` plus an exact post-filter to avoid
/// substring false positives (e.g. `"code"` does not match `"code-review"`).
async fn find_by_capability_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(_auth): Auth,
    Path(name): Path<String>,
) -> Result<Json<Value>, AppError> {
    let agents = find_by_capability(&db, &name).await?;
    Ok(Json(json!(agents)))
}
