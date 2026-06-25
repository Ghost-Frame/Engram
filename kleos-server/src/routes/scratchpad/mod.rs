use axum::{
    extract::{Path, Query},
    routing::{delete, get, post},
    Json, Router,
};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::{Auth, ResolvedDb};
use crate::state::AppState;

mod types;
use types::{PromoteBody, ScratchGetQuery, ScratchQuery};

/// Register all scratchpad routes on the shared application state.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/scratch", get(list_scratch).put(put_scratch))
        .route("/scratch/{session}", delete(delete_session))
        .route("/scratch/{session}/{key}", delete(delete_key))
        .route("/scratch/{session}/promote", post(promote))
        // Ledger read used by the `ke` edit-gate.  Path must match
        // `GET /scratchpad/get?namespace=spec-task&key=<session>:<path>`.
        .route("/scratchpad/get", get(get_scratch))
}

/// Lists the caller's active scratchpad entries, scoped to the authenticated
/// user, optionally filtered by agent, model, and session.
async fn list_scratch(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(q): Query<ScratchQuery>,
) -> Result<Json<Value>, AppError> {
    let entries = kleos_lib::scratchpad::list_entries(
        &db,
        auth.effective_user_id(),
        q.agent.as_deref(),
        q.model.as_deref(),
        q.session.as_deref(),
    )
    .await?;
    let count = entries.len();
    Ok(Json(json!({ "entries": entries, "count": count })))
}

/// Upserts one or more scratchpad entries for the authenticated user.
async fn put_scratch(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<kleos_lib::scratchpad::ScratchPutBody>,
) -> Result<Json<Value>, AppError> {
    let user_id = auth.effective_user_id();
    let session = body.session.as_deref().unwrap_or("default");
    let agent = body.agent.as_deref().unwrap_or("unknown");
    let model = body.model.as_deref().unwrap_or("");
    let ttl = body.ttl.unwrap_or(30).clamp(1, 1440);
    let entries = body.entries.unwrap_or_default();
    let mut stored = 0;
    for e in &entries {
        let value = e.value.as_deref().unwrap_or("");
        kleos_lib::scratchpad::upsert_entry(
            &db, user_id, session, agent, model, &e.key, value, ttl,
        )
        .await?;
        stored += 1;
    }
    Ok(Json(
        json!({ "stored": stored, "session": session, "ttl_minutes": ttl }),
    ))
}

/// Deletes every scratchpad entry in one session owned by the authenticated user.
async fn delete_session(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(session): Path<String>,
) -> Result<Json<Value>, AppError> {
    kleos_lib::scratchpad::delete_session(&db, auth.effective_user_id(), &session).await?;
    Ok(Json(json!({ "deleted": true, "session": session })))
}

/// Deletes one key from one scratchpad session owned by the authenticated user.
async fn delete_key(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path((session, key)): Path<(String, String)>,
) -> Result<Json<Value>, AppError> {
    kleos_lib::scratchpad::delete_session_key(&db, auth.effective_user_id(), &session, &key)
        .await?;
    Ok(Json(
        json!({ "deleted": true, "session": session, "key": key }),
    ))
}

/// Promotes selected scratchpad entries in a session into permanent memories
/// owned by the authenticated user.
async fn promote(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(session): Path<String>,
    Json(body): Json<PromoteBody>,
) -> Result<Json<Value>, AppError> {
    let combine = body.combine.unwrap_or(false);
    let category = body.category.as_deref().unwrap_or("discovery");
    let ids = kleos_lib::scratchpad::promote_entries(
        &db,
        auth.effective_user_id(),
        &session,
        body.keys.as_deref(),
        combine,
        category,
    )
    .await?;
    Ok(Json(
        json!({ "promoted": true, "memory_ids": ids, "count": ids.len() }),
    ))
}

/// Read one ledger entry for the `ke` edit-gate.
///
/// `GET /scratchpad/get?namespace=<agent>&key=<entry_key>`
///
/// - Returns 200 `{"value": "<spec_id>", "key": "<key>"}` when a non-expired
///   row is found whose `agent = namespace` and `entry_key = key`.
/// - Returns 404 `{"value": null, "key": "<key>"}` otherwise so `ke` can
///   distinguish "not found" (HTTP 404) from a genuine server error.
///
/// The response shape is designed to satisfy ke's exact success conditions:
/// HTTP 200, body non-empty, no `"value":null`, no `not found`.
async fn get_scratch(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(q): Query<ScratchGetQuery>,
) -> Result<(axum::http::StatusCode, Json<Value>), AppError> {
    match kleos_lib::scratchpad::get_by_namespace_key(
        &db,
        auth.effective_user_id(),
        &q.namespace,
        &q.key,
    )
    .await?
    {
        Some(value) => Ok((
            axum::http::StatusCode::OK,
            Json(json!({ "value": value, "key": q.key })),
        )),
        None => Ok((
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({ "value": null, "key": q.key })),
        )),
    }
}

/// Unit tests for the scratchpad ledger read helper.
#[cfg(test)]
mod tests {
    use kleos_lib::scratchpad::{get_by_namespace_key, upsert_entry};

    /// Inserting an entry under namespace="spec-task" and querying by the same
    /// namespace+key must return the stored value.  A different key returns None.
    #[tokio::test]
    async fn scratchpad_get_by_namespace_key_hit_and_miss() {
        let db = kleos_lib::db::Database::connect_memory()
            .await
            .expect("in-memory db");

        // Write a ledger entry the way the forge spec-task handler does, as user 1.
        upsert_entry(
            &db,
            1,           // user_id
            "S",         // session
            "spec-task", // agent == namespace ke queries
            "",          // model
            "S:/x/a.rs", // entry_key == `format!("{session_id}:{path}")` ke builds
            "spec_1",    // value == spec id
            1440,
        )
        .await
        .expect("upsert spec-task ledger entry");

        // Hit: correct user + namespace + key returns the spec id.
        let found = get_by_namespace_key(&db, 1, "spec-task", "S:/x/a.rs")
            .await
            .expect("query hit");
        assert_eq!(found, Some("spec_1".to_string()));

        // Miss: different key under same namespace returns None.
        let miss = get_by_namespace_key(&db, 1, "spec-task", "S:/x/b.rs")
            .await
            .expect("query miss");
        assert_eq!(miss, None);

        // Miss: same key but wrong namespace returns None.
        let miss2 = get_by_namespace_key(&db, 1, "forge", "S:/x/a.rs")
            .await
            .expect("query wrong namespace");
        assert_eq!(miss2, None);

        // Isolation: user 2 cannot read user 1's ledger entry even with the
        // exact namespace + key. This is the cross-tenant fix (v75 readd).
        let other_tenant = get_by_namespace_key(&db, 2, "spec-task", "S:/x/a.rs")
            .await
            .expect("query as other tenant");
        assert_eq!(
            other_tenant, None,
            "user 2 must not see user 1's ledger entry"
        );
    }
}
