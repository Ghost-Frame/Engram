//! Broca routes: action logging, feed, stats, LLM narration, and unauthenticated
//! Axon ingest.
//!
//! The authenticated handlers (`/broca/actions`, `/broca/feed`, `/broca/stats`,
//! `/broca/actions/{id}/narrate`, `/broca/narrate`) are mounted inside the auth
//! middleware stack via [`router`]. The webhook receiver (`/broca/ingest`) is
//! mounted outside auth via [`ingest_router`] and uses the system tenant shard
//! (user_id=1) for storage.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::{resolve_db_for_user, Auth, ResolvedDb};
use crate::state::AppState;
use kleos_lib::services::broca::{
    ask as broca_ask, get_action, get_or_narrate_action, get_stats as get_broca_stats, log_action,
    narrate_from_template, query_actions, LogActionRequest,
};

mod types;
use types::{AskBody, IngestBody, LogActionBody, NarrateBatchBody, QueryActionsParams};

/// Authenticated router: mounts broca routes inside the auth middleware stack.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/broca/actions",
            post(log_action_handler).get(list_actions_handler),
        )
        .route("/broca/actions/{id}", get(get_action_handler))
        .route("/broca/actions/{id}/narrate", get(narrate_action_handler))
        .route("/broca/narrate", post(narrate_batch_handler))
        .route("/broca/feed", get(get_feed_handler))
        .route("/broca/stats", get(get_stats))
        .route("/broca/ask", post(ask_handler))
}

/// Unauthenticated router: mounts `/broca/ingest` outside the auth middleware.
///
/// Mount this via `public_routes` in `server.rs` so the route does not pass
/// through `auth_middleware`. Network-layer controls (firewall, reverse-proxy
/// allowlist) are the recommended protection surface.
pub fn ingest_router() -> Router<AppState> {
    Router::new().route("/broca/ingest", post(ingest_handler))
}

/// Handler for `POST /broca/actions`. Logs an agent action for the
/// authenticated user. The `narrative` field (or `summary`/`detail` aliases)
/// is stored verbatim; when absent, `log_action` auto-generates one via the
/// template narrator.
async fn log_action_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<LogActionBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let action = body.action.clone().unwrap_or_else(|| "unknown".to_string());

    let narrative = body.narrative.or(body.summary).or(body.detail);

    let mut payload = body.payload.or(body.metadata);
    if let Some(project) = body.project {
        let obj = payload.get_or_insert_with(|| serde_json::Value::Object(Default::default()));
        if let Some(map) = obj.as_object_mut() {
            map.entry("project")
                .or_insert(serde_json::Value::String(project));
        }
    }

    let req = LogActionRequest {
        agent: body.agent,
        service: body.service,
        action,
        narrative,
        payload,
        axon_event_id: body.axon_event_id,
        user_id: Some(auth.user_id),
    };

    let entry = log_action(&db, req).await?;
    Ok((StatusCode::CREATED, Json(json!(entry))))
}

/// Handler for `GET /broca/actions`. Lists broca actions for the authenticated
/// user with optional filtering by agent, service, and action type.
async fn list_actions_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<QueryActionsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(100).min(1000);
    let offset = params.offset.unwrap_or(0);

    let agent = params.agent.as_deref();
    let service = params.service.as_deref();
    let action = params.action.as_deref();
    let since = params.since.as_deref();

    let entries = query_actions(
        &db,
        agent,
        service,
        action,
        since,
        limit,
        offset,
        auth.user_id,
    )
    .await?;

    Ok(Json(json!({ "actions": entries, "count": entries.len() })))
}

/// Handler for `GET /broca/actions/{id}`. Fetches a single broca action by id.
async fn get_action_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let entry = get_action(&db, id, auth.user_id).await?;
    Ok(Json(json!(entry)))
}

/// Handler for `GET /broca/feed`. Returns a chronological action feed for the
/// authenticated user, optionally filtered by agent. Intended for dashboard
/// consumption where service/action filtering is not needed.
async fn get_feed_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<QueryActionsParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(100).min(1000);
    let offset = params.offset.unwrap_or(0);
    let agent = params.agent.as_deref();
    let since = params.since.as_deref();

    let entries = query_actions(&db, agent, None, None, since, limit, offset, auth.user_id).await?;

    Ok(Json(json!({ "items": entries, "count": entries.len() })))
}

/// Handler for `GET /broca/stats`. Returns aggregate broca statistics
/// (total actions, distinct agents, distinct services) for the authenticated
/// user's tenant shard.
async fn get_stats(Auth(auth): Auth, ResolvedDb(db): ResolvedDb) -> Result<Json<Value>, AppError> {
    let stats = get_broca_stats(&db, auth.user_id).await?;
    Ok(Json(json!(stats)))
}

/// Handler for `GET /broca/actions/{id}/narrate`. Returns the narrative for a
/// single action, scoped to the authenticated user's tenant. If the action
/// already has a stored narrative it is returned directly; otherwise the LLM
/// is called, the result is persisted, and then returned.
///
/// Response shape: `{ id, narrative }`.
///
/// Returns 404 when no action with the given id exists or when the action
/// belongs to a different tenant.
async fn narrate_action_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    // get_or_narrate_action enforces tenant scope via user_id and handles both
    // the fetch and the LLM-persist slow path. Returns None for missing or
    // cross-tenant ids so we can emit a clean 404.
    let narrative = get_or_narrate_action(&db, id, auth.user_id)
        .await?
        .ok_or_else(|| AppError(kleos_lib::EngError::NotFound(format!("action {id}"))))?;

    Ok(Json(json!({ "id": id, "narrative": narrative })))
}

/// Handler for `POST /broca/narrate`. Bulk-narrates up to 50 actions in a
/// single call, scoped to the authenticated user's tenant.
///
/// Accepts `{ "ids": [i64] }`. Returns a raw JSON array
/// `[{ "id": i64, "narrative": "..." }]` -- NOT wrapped in an envelope -- to
/// match the standalone broca server response shape.
///
/// Validation errors:
/// - 400 if `ids` is empty.
/// - 400 if `ids` contains more than 50 elements.
///
/// Actions whose id does not exist in the database, or that belong to a
/// different tenant, are silently skipped (the batch result simply omits them)
/// rather than causing a 404 for the whole request.
async fn narrate_batch_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<NarrateBatchBody>,
) -> Result<Json<Value>, AppError> {
    if body.ids.is_empty() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "ids array required and must not be empty".into(),
        )));
    }
    if body.ids.len() > 50 {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "max 50 ids per batch".into(),
        )));
    }

    let mut results: Vec<Value> = Vec::with_capacity(body.ids.len());
    for action_id in body.ids {
        // get_or_narrate_action returns Ok(None) for missing ids or ids owned by
        // other tenants -- silently skip both so cross-tenant ids don't error.
        if let Some(narrative) = get_or_narrate_action(&db, action_id, auth.user_id).await? {
            results.push(json!({ "id": action_id, "narrative": narrative }));
        }
    }

    Ok(Json(Value::Array(results)))
}

/// Handler for `POST /broca/ask`. Accepts a natural-language question and
/// returns a synthesized plain-English answer together with the query plan
/// and the matched raw action rows.
///
/// The pipeline is:
/// 1. LLM plan call -- translates the question into query parameters.
/// 2. `query_actions` -- fetches matching action rows for the authenticated tenant.
/// 3. LLM summarize call -- produces a 1-3 sentence answer with action-id citations.
///
/// Both LLM calls fall back gracefully (keyword heuristic / narrative
/// concatenation) so this endpoint never returns 500 for LLM-related failures.
///
/// Validation:
/// - `question` must be non-empty.
/// - `question` must not exceed 2 000 Unicode characters.
///
/// Response shape: `{ "answer": "...", "plan": {...}, "raw": [...] }`.
async fn ask_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<AskBody>,
) -> Result<Json<Value>, AppError> {
    // Validate question length before any LLM calls.
    if body.question.is_empty() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "question must not be empty".into(),
        )));
    }
    if body.question.chars().count() > 2000 {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "question must not exceed 2000 characters".into(),
        )));
    }

    let result = broca_ask(&db, auth.user_id, &body.question).await?;
    Ok(Json(json!(result)))
}

/// Handler for `POST /broca/ingest`. Intentionally unauthenticated; receives
/// webhook events from Axon and persists them as broca actions in the system
/// tenant shard (user_id=1) with the matching template-rendered narrative.
///
/// `source` and `type` are required. The upstream Axon event id is stored in
/// `axon_event_id` for correlation. Protect this endpoint at the network layer
/// (firewall, reverse-proxy allowlist) rather than with bearer tokens.
async fn ingest_handler(
    State(state): State<AppState>,
    Json(body): Json<IngestBody>,
) -> Result<Json<Value>, AppError> {
    if body.source.is_empty() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "source is required".into(),
        )));
    }
    if body.event_type.is_empty() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "type is required".into(),
        )));
    }

    // System user shard: webhook ingestion is not tied to a specific tenant.
    // user_id=1 is the operator/system tenant.
    let db = resolve_db_for_user(&state, 1).await?;

    let payload = body
        .payload
        .unwrap_or_else(|| serde_json::Value::Object(Default::default()));

    // Build the narrative before moving payload into the log request so we
    // can borrow it for template rendering without cloning the entire value.
    let narrative = narrate_from_template(&body.event_type, &payload);

    // Use the channel as the service label when present; fall back to source.
    let service = body.channel.unwrap_or_else(|| body.source.clone());

    let req = LogActionRequest {
        agent: body.source.clone(),
        service: Some(service),
        action: body.event_type,
        narrative,
        payload: Some(payload),
        axon_event_id: body.id,
        user_id: Some(1),
    };

    let entry = log_action(&db, req).await?;
    Ok(Json(json!({
        "ok": true,
        "id": entry.id,
        "axon_event_id": entry.axon_event_id,
    })))
}
