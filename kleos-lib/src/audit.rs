use crate::db::Database;
use crate::{EngError, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: i64,
    pub user_id: Option<i64>,
    pub agent_id: Option<i64>,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<i64>,
    pub details: Option<String>,
    pub ip: Option<String>,
    pub request_id: Option<String>,
    pub created_at: String,
}

fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<AuditEntry> {
    Ok(AuditEntry {
        id: row.get(0)?,
        user_id: row.get(1)?,
        agent_id: row.get(2)?,
        action: row.get(3)?,
        target_type: row.get(4)?,
        target_id: row.get(5)?,
        details: row.get(6)?,
        ip: row.get(7)?,
        request_id: row.get(8)?,
        created_at: row.get(9)?,
    })
}

/// Log a mutation event (create/update/delete) to the audit trail.
///
/// Maps legacy "operation/resource" terminology onto the DB schema columns.
/// The `before`/`after` snapshots and `actor` are merged into the `details` JSON field.
#[tracing::instrument(skip(db, before, after), fields(operation = %operation, resource_type = %resource_type, resource_id = %resource_id))]
pub async fn log_mutation(
    db: &Database,
    operation: &str,
    resource_type: &str,
    resource_id: &str,
    actor: Option<&str>,
    before: Option<serde_json::Value>,
    after: Option<serde_json::Value>,
) -> Result<AuditEntry> {
    let target_id: Option<i64> = resource_id.parse().ok();
    let target_type: Option<String> = if resource_type.is_empty() {
        None
    } else {
        Some(resource_type.to_string())
    };

    let details: Option<String> = {
        let mut d = serde_json::Map::new();
        if let Some(a) = actor {
            d.insert("actor".into(), serde_json::Value::String(a.to_string()));
        }
        if let Some(b) = before {
            d.insert("before".into(), b);
        }
        if let Some(a) = after {
            d.insert("after".into(), a);
        }
        if d.is_empty() {
            None
        } else {
            Some(serde_json::Value::Object(d).to_string())
        }
    };

    let operation = operation.to_string();

    db.write(move |conn| {
        conn.query_row(
            "INSERT INTO audit_log (action, target_type, target_id, details)
             VALUES (?1, ?2, ?3, ?4)
             RETURNING id, user_id, agent_id, action, target_type, target_id, details, ip, request_id, created_at",
            params![operation, target_type, target_id, details],
            row_to_entry,
        )
        .map_err(rusqlite_to_eng_error)
    })
    .await
}

/// Log an HTTP request to the audit trail. Used by the audit middleware.
///
/// Fire-and-forget compatible -- errors are discarded by the caller.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip(db, details), fields(action = %action))]
pub async fn log_request(
    db: &Database,
    user_id: Option<i64>,
    agent_id: Option<i64>,
    action: &str,
    target_type: Option<&str>,
    target_id: Option<i64>,
    details: Option<&str>,
    ip: Option<&str>,
    request_id: Option<&str>,
    identity_id: Option<i64>,
    tier: Option<&str>,
) -> Result<()> {
    let action = action.to_string();
    let target_type = target_type.map(|s| s.to_string());
    let details = details.map(|s| s.to_string());
    let ip = ip.map(|s| s.to_string());
    let request_id = request_id.map(|s| s.to_string());
    let tier = tier.map(|s| s.to_string());

    db.write(move |conn| {
        conn.execute(
            "INSERT INTO audit_log (user_id, agent_id, action, target_type, target_id, details, ip, request_id, identity_id, tier)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![user_id, agent_id, action, target_type, target_id, details, ip, request_id, identity_id, tier],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

/// Query audit log entries for a specific tenant.
///
/// SECURITY: the `user_id` argument is ALWAYS applied as a WHERE clause so a
/// non-admin caller can only see their own entries. Admin-wide access lives in
/// `query_audit_log_admin`.
#[tracing::instrument(skip(db))]
pub async fn query_audit_log(
    db: &Database,
    resource_type: Option<&str>,
    resource_id: Option<&str>,
    limit: usize,
    user_id: i64,
) -> Result<Vec<AuditEntry>> {
    let target_id: Option<i64> = resource_id.and_then(|r| r.parse().ok());
    let limit_i64 = limit as i64;
    let resource_type = resource_type.map(|s| s.to_string());

    db.read(move |conn| {
        let sql;
        let mut stmt;
        let rows: Box<dyn Iterator<Item = rusqlite::Result<AuditEntry>>>;

        match (resource_type.as_deref(), target_id) {
            (Some(rt), Some(tid)) => {
                sql = "SELECT id, user_id, agent_id, action, target_type, target_id, details, ip, request_id, created_at
                       FROM audit_log WHERE user_id = ?1 AND target_type = ?2 AND target_id = ?3
                       ORDER BY id DESC LIMIT ?4".to_string();
                stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
                rows = Box::new(
                    stmt.query_map(params![user_id, rt, tid, limit_i64], row_to_entry)
                        .map_err(rusqlite_to_eng_error)?,
                );
            }
            (Some(rt), None) => {
                sql = "SELECT id, user_id, agent_id, action, target_type, target_id, details, ip, request_id, created_at
                       FROM audit_log WHERE user_id = ?1 AND target_type = ?2
                       ORDER BY id DESC LIMIT ?3".to_string();
                stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
                rows = Box::new(
                    stmt.query_map(params![user_id, rt, limit_i64], row_to_entry)
                        .map_err(rusqlite_to_eng_error)?,
                );
            }
            _ => {
                sql = "SELECT id, user_id, agent_id, action, target_type, target_id, details, ip, request_id, created_at
                       FROM audit_log WHERE user_id = ?1 ORDER BY id DESC LIMIT ?2".to_string();
                stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
                rows = Box::new(
                    stmt.query_map(params![user_id, limit_i64], row_to_entry)
                        .map_err(rusqlite_to_eng_error)?,
                );
            }
        }

        let mut entries = Vec::new();
        for row in rows {
            match row {
                Ok(entry) => entries.push(entry),
                Err(e) => tracing::warn!("audit row parse error: {}", e),
            }
        }
        Ok(entries)
    })
    .await
}

/// List audit log entries for a specific user with limit and offset for pagination.
///
/// SECURITY: the `user_id` argument is ALWAYS applied as a WHERE clause so a
/// non-admin caller can only see their own entries.
#[tracing::instrument(skip(db))]
pub async fn list_audit_entries(
    db: &Database,
    user_id: i64,
    limit: i64,
    offset: i64,
) -> Result<Vec<AuditEntry>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, user_id, agent_id, action, target_type, target_id, details, ip, request_id, created_at
                 FROM audit_log WHERE user_id = ?1 ORDER BY id DESC LIMIT ?2 OFFSET ?3",
            )
            .map_err(rusqlite_to_eng_error)?;
        let rows = stmt
            .query_map(params![user_id, limit, offset], row_to_entry)
            .map_err(rusqlite_to_eng_error)?;
        let mut entries = Vec::new();
        for row in rows {
            match row {
                Ok(entry) => entries.push(entry),
                Err(e) => tracing::warn!("audit row parse error: {}", e),
            }
        }
        Ok(entries)
    })
    .await
}

/// Count audit log entries for a specific user.
#[tracing::instrument(skip(db))]
pub async fn count_audit_entries(db: &Database, user_id: i64) -> Result<i64> {
    db.read(move |conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM audit_log WHERE user_id = ?1",
            params![user_id],
            |row| row.get(0),
        )
        .map_err(rusqlite_to_eng_error)
    })
    .await
}

/// Admin-wide audit query (no user_id filter). Callers MUST verify that the
/// requester carries `Scope::Admin` before invoking this function.
#[tracing::instrument(skip(db))]
pub async fn query_audit_log_admin(
    db: &Database,
    resource_type: Option<&str>,
    resource_id: Option<&str>,
    limit: usize,
) -> Result<Vec<AuditEntry>> {
    let target_id: Option<i64> = resource_id.and_then(|r| r.parse().ok());
    let limit_i64 = limit as i64;
    let resource_type = resource_type.map(|s| s.to_string());

    db.read(move |conn| {
        let sql;
        let mut stmt;
        let rows: Box<dyn Iterator<Item = rusqlite::Result<AuditEntry>>>;

        match (resource_type.as_deref(), target_id) {
            (Some(rt), Some(tid)) => {
                sql = "SELECT id, user_id, agent_id, action, target_type, target_id, details, ip, request_id, created_at
                       FROM audit_log WHERE target_type = ?1 AND target_id = ?2
                       ORDER BY id DESC LIMIT ?3".to_string();
                stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
                rows = Box::new(
                    stmt.query_map(params![rt, tid, limit_i64], row_to_entry)
                        .map_err(rusqlite_to_eng_error)?,
                );
            }
            (Some(rt), None) => {
                sql = "SELECT id, user_id, agent_id, action, target_type, target_id, details, ip, request_id, created_at
                       FROM audit_log WHERE target_type = ?1
                       ORDER BY id DESC LIMIT ?2".to_string();
                stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
                rows = Box::new(
                    stmt.query_map(params![rt, limit_i64], row_to_entry)
                        .map_err(rusqlite_to_eng_error)?,
                );
            }
            _ => {
                sql = "SELECT id, user_id, agent_id, action, target_type, target_id, details, ip, request_id, created_at
                       FROM audit_log ORDER BY id DESC LIMIT ?1".to_string();
                stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
                rows = Box::new(
                    stmt.query_map(params![limit_i64], row_to_entry)
                        .map_err(rusqlite_to_eng_error)?,
                );
            }
        }

        let mut entries = Vec::new();
        for row in rows {
            match row {
                Ok(entry) => entries.push(entry),
                Err(e) => tracing::warn!("audit row parse error: {}", e),
            }
        }
        Ok(entries)
    })
    .await
}
