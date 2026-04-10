use crate::db::Database;
use crate::Result;
use serde::{Deserialize, Serialize};

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

fn row_to_entry(row: &libsql::Row) -> Result<AuditEntry> {
    Ok(AuditEntry {
        id: row
            .get(0)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?,
        user_id: row.get(1).ok(),
        agent_id: row.get(2).ok(),
        action: row
            .get(3)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?,
        target_type: row.get(4).ok(),
        target_id: row.get(5).ok(),
        details: row.get(6).ok(),
        ip: row.get(7).ok(),
        request_id: row.get(8).ok(),
        created_at: row
            .get(9)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?,
    })
}

/// Log a mutation event (create/update/delete) to the audit trail.
///
/// Maps legacy "operation/resource" terminology onto the DB schema columns.
/// The `before`/`after` snapshots and `actor` are merged into the `details` JSON field.
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
    let target_type: Option<&str> = if resource_type.is_empty() {
        None
    } else {
        Some(resource_type)
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

    let mut rows = db.conn.query(
        "INSERT INTO audit_log (action, target_type, target_id, details)
         VALUES (?1, ?2, ?3, ?4)
         RETURNING id, user_id, agent_id, action, target_type, target_id, details, ip, request_id, created_at",
        libsql::params![operation, target_type, target_id, details],
    ).await?;

    let row = rows
        .next()
        .await?
        .ok_or_else(|| crate::EngError::Internal("audit insert returned no row".into()))?;
    row_to_entry(&row)
}

/// Log an HTTP request to the audit trail. Used by the audit middleware.
///
/// Fire-and-forget compatible -- errors are discarded by the caller.
#[allow(clippy::too_many_arguments)]
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
) -> Result<()> {
    db.conn.execute(
        "INSERT INTO audit_log (user_id, agent_id, action, target_type, target_id, details, ip, request_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        libsql::params![user_id, agent_id, action, target_type, target_id, details, ip, request_id],
    ).await?;
    Ok(())
}

/// Query audit log entries, optionally filtered by target_type and target_id.
pub async fn query_audit_log(
    db: &Database,
    resource_type: Option<&str>,
    resource_id: Option<&str>,
    limit: usize,
    _user_id: i64,
) -> Result<Vec<AuditEntry>> {
    let target_id: Option<i64> = resource_id.and_then(|r| r.parse().ok());
    let limit_i64 = limit as i64;

    let mut rows = match (resource_type, target_id) {
        (Some(rt), Some(tid)) => db.conn.query(
            "SELECT id, user_id, agent_id, action, target_type, target_id, details, ip, request_id, created_at
             FROM audit_log WHERE target_type = ?1 AND target_id = ?2
             ORDER BY id DESC LIMIT ?3",
            libsql::params![rt, tid, limit_i64],
        ).await?,
        (Some(rt), None) => db.conn.query(
            "SELECT id, user_id, agent_id, action, target_type, target_id, details, ip, request_id, created_at
             FROM audit_log WHERE target_type = ?1
             ORDER BY id DESC LIMIT ?2",
            libsql::params![rt, limit_i64],
        ).await?,
        _ => db.conn.query(
            "SELECT id, user_id, agent_id, action, target_type, target_id, details, ip, request_id, created_at
             FROM audit_log ORDER BY id DESC LIMIT ?1",
            libsql::params![limit_i64],
        ).await?,
    };

    let mut entries = Vec::new();
    while let Some(row) = rows.next().await? {
        match row_to_entry(&row) {
            Ok(entry) => entries.push(entry),
            Err(e) => tracing::warn!("audit row parse error: {}", e),
        }
    }
    Ok(entries)
}
