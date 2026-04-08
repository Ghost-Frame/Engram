use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::db::Database;
use crate::Result;
use tracing::warn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: String,
    pub operation: String,
    pub resource_type: String,
    pub resource_id: String,
    pub actor: Option<String>,
    pub before: Option<serde_json::Value>,
    pub after: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

/// Log a mutation to the audit trail.
pub async fn log_mutation(
    db: &Database,
    operation: &str,
    resource_type: &str,
    resource_id: &str,
    actor: Option<&str>,
    before: Option<serde_json::Value>,
    after: Option<serde_json::Value>,
) -> Result<AuditEntry> {
    let conn = db.connection();

    let details = serde_json::json!({
        "before": before,
        "after": after,
    });

    conn.execute(
        "INSERT INTO audit_log (action, target_type, target_id, details, ip, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
        libsql::params![
            operation.to_string(),
            resource_type.to_string(),
            resource_id.to_string(),
            details.to_string(),
            actor.unwrap_or("system").to_string()
        ],
    )
    .await
    .map_err(|e| {
        warn!(error = %e, "audit_log_failed");
        e
    })?;

    // Fetch the created entry
    let mut rows = conn
        .query("SELECT last_insert_rowid()", ())
        .await?;
    let new_id: i64 = if let Some(row) = rows.next().await? {
        row.get(0)?
    } else {
        0
    };

    Ok(AuditEntry {
        id: new_id.to_string(),
        operation: operation.to_string(),
        resource_type: resource_type.to_string(),
        resource_id: resource_id.to_string(),
        actor: actor.map(String::from),
        before,
        after,
        created_at: Utc::now(),
    })
}

/// Query the audit log with optional filters.
pub async fn query_audit_log(
    db: &Database,
    resource_type: Option<&str>,
    resource_id: Option<&str>,
    limit: usize,
) -> Result<Vec<AuditEntry>> {
    let conn = db.connection();

    let (query, params) = match (resource_type, resource_id) {
        (Some(rt), Some(ri)) => (
            "SELECT id, action, target_type, target_id, ip, details, created_at \
             FROM audit_log \
             WHERE target_type = ?1 AND target_id = ?2 \
             ORDER BY created_at DESC \
             LIMIT ?3"
                .to_string(),
            vec![
                libsql::Value::Text(rt.to_string()),
                libsql::Value::Text(ri.to_string()),
                libsql::Value::Integer(limit as i64),
            ],
        ),
        (Some(rt), None) => (
            "SELECT id, action, target_type, target_id, ip, details, created_at \
             FROM audit_log \
             WHERE target_type = ?1 \
             ORDER BY created_at DESC \
             LIMIT ?2"
                .to_string(),
            vec![
                libsql::Value::Text(rt.to_string()),
                libsql::Value::Integer(limit as i64),
            ],
        ),
        (None, Some(ri)) => (
            "SELECT id, action, target_type, target_id, ip, details, created_at \
             FROM audit_log \
             WHERE target_id = ?1 \
             ORDER BY created_at DESC \
             LIMIT ?2"
                .to_string(),
            vec![
                libsql::Value::Text(ri.to_string()),
                libsql::Value::Integer(limit as i64),
            ],
        ),
        (None, None) => (
            "SELECT id, action, target_type, target_id, ip, details, created_at \
             FROM audit_log \
             ORDER BY created_at DESC \
             LIMIT ?1"
                .to_string(),
            vec![libsql::Value::Integer(limit as i64)],
        ),
    };

    let mut rows = conn.query(&query, params).await?;
    let mut entries = Vec::new();

    while let Some(row) = rows.next().await? {
        let id: i64 = row.get(0)?;
        let action: String = row.get(1)?;
        let target_type: String = row.get(2)?;
        let target_id: String = row.get::<String>(3).unwrap_or_default();
        let actor: Option<String> = row.get(4)?;
        let details_str: Option<String> = row.get(5)?;
        let created_at_str: String = row.get(6)?;

        // Parse details JSON
        let (before, after) = if let Some(ref d) = details_str {
            let parsed: serde_json::Value =
                serde_json::from_str(d).unwrap_or(serde_json::Value::Null);
            (
                parsed.get("before").cloned(),
                parsed.get("after").cloned(),
            )
        } else {
            (None, None)
        };

        // Parse timestamp
        let created_at = chrono::NaiveDateTime::parse_from_str(&created_at_str, "%Y-%m-%d %H:%M:%S")
            .map(|dt| DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc))
            .unwrap_or_else(|_| Utc::now());

        entries.push(AuditEntry {
            id: id.to_string(),
            operation: action,
            resource_type: target_type,
            resource_id: target_id,
            actor,
            before,
            after,
            created_at,
        });
    }

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_entry_serialization() {
        let entry = AuditEntry {
            id: "1".to_string(),
            operation: "create".to_string(),
            resource_type: "memory".to_string(),
            resource_id: "42".to_string(),
            actor: Some("agent-1".to_string()),
            before: None,
            after: Some(serde_json::json!({"content": "hello"})),
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("create"));
        assert!(json.contains("memory"));
    }
}
