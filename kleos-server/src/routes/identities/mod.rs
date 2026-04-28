use axum::extract::{Path, Query};
use axum::routing::get;
use axum::{Json, Router};
use rusqlite::params;
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::{Auth, ResolvedDb};
use crate::state::AppState;
use kleos_lib::auth::Scope;

mod types;
use types::{AuditParams, ListIdentitiesParams};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/identities", get(list_handler))
        .route("/identities/{id}/audit", get(audit_handler))
}

async fn list_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<ListIdentitiesParams>,
) -> Result<Json<Value>, AppError> {
    if !auth.has_scope(&Scope::Admin) {
        return Err(AppError(kleos_lib::EngError::Auth(
            "admin scope required".into(),
        )));
    }

    let limit = params.limit.unwrap_or(50).min(500);
    let offset = params.offset.unwrap_or(0);
    let host = params.host;
    let agent = params.agent;
    let model = params.model;
    let key_id = params.key_id;

    let rows = db
        .read(move |conn| {
            let mut conditions = vec!["1=1".to_string()];
            let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

            if let Some(ref h) = host {
                bind_values.push(Box::new(h.clone()));
                conditions.push(format!("i.host_label = ?{}", bind_values.len()));
            }
            if let Some(ref a) = agent {
                bind_values.push(Box::new(a.clone()));
                conditions.push(format!("i.agent_label = ?{}", bind_values.len()));
            }
            if let Some(ref m) = model {
                bind_values.push(Box::new(m.clone()));
                conditions.push(format!("i.model_label = ?{}", bind_values.len()));
            }
            if let Some(kid) = key_id {
                bind_values.push(Box::new(kid));
                conditions.push(format!("i.identity_key_id = ?{}", bind_values.len()));
            }

            bind_values.push(Box::new(limit as i64));
            let limit_idx = bind_values.len();
            bind_values.push(Box::new(offset as i64));
            let offset_idx = bind_values.len();

            let sql = format!(
                "SELECT i.id, i.identity_hash, i.host_label, i.agent_label, i.model_label,
                        i.first_seen_at, i.last_seen_at, i.request_count, i.is_active,
                        ik.tier, ik.algo, ik.pubkey_fingerprint
                 FROM identities i
                 JOIN identity_keys ik ON ik.id = i.identity_key_id
                 WHERE {}
                 ORDER BY i.last_seen_at DESC
                 LIMIT ?{} OFFSET ?{}",
                conditions.join(" AND "),
                limit_idx,
                offset_idx
            );

            let params_ref: Vec<&dyn rusqlite::types::ToSql> =
                bind_values.iter().map(|b| b.as_ref()).collect();

            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;

            let rows = stmt
                .query_map(params_ref.as_slice(), |row| {
                    Ok(json!({
                        "id": row.get::<_, i64>(0)?,
                        "identity_hash": row.get::<_, String>(1)?,
                        "host_label": row.get::<_, String>(2)?,
                        "agent_label": row.get::<_, String>(3)?,
                        "model_label": row.get::<_, String>(4)?,
                        "first_seen_at": row.get::<_, String>(5)?,
                        "last_seen_at": row.get::<_, String>(6)?,
                        "request_count": row.get::<_, i64>(7)?,
                        "is_active": row.get::<_, bool>(8)?,
                        "tier": row.get::<_, String>(9)?,
                        "algo": row.get::<_, String>(10)?,
                        "pubkey_fingerprint": row.get::<_, String>(11)?,
                    }))
                })
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;

            Ok(rows)
        })
        .await?;

    Ok(Json(json!({ "identities": rows, "count": rows.len() })))
}

async fn audit_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(identity_id): Path<i64>,
    Query(params): Query<AuditParams>,
) -> Result<Json<Value>, AppError> {
    if !auth.has_scope(&Scope::Admin) {
        return Err(AppError(kleos_lib::EngError::Auth(
            "admin scope required".into(),
        )));
    }

    let limit = params.limit.unwrap_or(50).min(500);
    let since = params.since.unwrap_or_else(|| "1970-01-01".to_string());

    let rows = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, action, target_type, target_id, tier, created_at
                     FROM audit_log
                     WHERE identity_id = ?1 AND created_at >= ?2
                     ORDER BY created_at DESC
                     LIMIT ?3",
                )
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;

            let rows = stmt
                .query_map(params![identity_id, since, limit as i64], |row| {
                    Ok(json!({
                        "id": row.get::<_, i64>(0)?,
                        "action": row.get::<_, String>(1)?,
                        "target_type": row.get::<_, Option<String>>(2)?,
                        "target_id": row.get::<_, Option<i64>>(3)?,
                        "tier": row.get::<_, Option<String>>(4)?,
                        "created_at": row.get::<_, String>(5)?,
                    }))
                })
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;

            Ok(rows)
        })
        .await?;

    Ok(Json(json!({ "audit": rows, "count": rows.len() })))
}
