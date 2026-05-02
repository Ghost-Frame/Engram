use axum::{
    extract::Query,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use rusqlite::params;
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::{Auth, ResolvedDb};
use crate::state::AppState;

mod types;
use types::{InjectBody, InjectionRow, PendingQuery};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/supervisor/inject", post(inject_handler))
        .route("/supervisor/pending", get(pending_handler))
}

/// POST /supervisor/inject
/// Persists a violation reported by eidolon-supervisor. The row is keyed by
/// the calling user_id and the supplied session_id and stays unclaimed until
/// the agent drains it via GET /supervisor/pending.
async fn inject_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Json(body): Json<InjectBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    if body.session_id.trim().is_empty() {
        return Err(AppError::from(kleos_lib::EngError::InvalidInput(
            "session_id is required".into(),
        )));
    }
    if body.rule_id.trim().is_empty() {
        return Err(AppError::from(kleos_lib::EngError::InvalidInput(
            "rule_id is required".into(),
        )));
    }

    let user_id = auth.user_id;
    let id = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO supervisor_injections (user_id, session_id, rule_id, severity, message)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    user_id,
                    body.session_id,
                    body.rule_id,
                    body.severity,
                    body.message,
                ],
            )
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
            Ok(conn.last_insert_rowid())
        })
        .await?;

    Ok((StatusCode::CREATED, Json(json!({ "ok": true, "id": id }))))
}

/// GET /supervisor/pending?session_id=<id>
/// Atomically claims and returns all unclaimed injections for the calling
/// user_id and the supplied session_id. The atomicity is provided by a
/// single transaction: UPDATE ... WHERE claimed_at IS NULL RETURNING (so a
/// concurrent caller never sees the same row twice).
async fn pending_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Query(q): Query<PendingQuery>,
) -> Result<Json<Value>, AppError> {
    if q.session_id.trim().is_empty() {
        return Err(AppError::from(kleos_lib::EngError::InvalidInput(
            "session_id is required".into(),
        )));
    }

    let user_id = auth.user_id;
    let session_id = q.session_id.clone();
    let claimed: Vec<InjectionRow> = db
        .transaction(move |tx| {
            let mut stmt = tx
                .prepare(
                    "UPDATE supervisor_injections
                     SET claimed_at = datetime('now')
                     WHERE user_id = ?1 AND session_id = ?2 AND claimed_at IS NULL
                     RETURNING id, session_id, rule_id, severity, message, created_at",
                )
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;

            let rows = stmt
                .query_map(params![user_id, session_id], |row| {
                    Ok(InjectionRow {
                        id: row.get(0)?,
                        session_id: row.get(1)?,
                        rule_id: row.get(2)?,
                        severity: row.get(3)?,
                        message: row.get(4)?,
                        created_at: row.get(5)?,
                    })
                })
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;

            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?);
            }
            Ok(out)
        })
        .await?;

    let count = claimed.len();
    Ok(Json(json!({
        "injections": claimed,
        "claimed": count,
        "session_id": q.session_id,
    })))
}
