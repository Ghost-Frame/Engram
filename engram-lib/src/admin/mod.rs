//! Admin operations -- ported from TS admin/db.ts + admin/operations.ts

pub mod types;

use chrono::Utc;
use crate::db::Database;
use crate::Result;
use self::types::*;

fn scopes_for_role(role: &str) -> Vec<crate::auth::Scope> {
    match role {
        "admin" => vec![
            crate::auth::Scope::Read,
            crate::auth::Scope::Write,
            crate::auth::Scope::Admin,
        ],
        "writer" => vec![crate::auth::Scope::Read, crate::auth::Scope::Write],
        _ => vec![crate::auth::Scope::Read],
    }
}

// ---------------------------------------------------------------------------
// Compact (VACUUM + ANALYZE)
// ---------------------------------------------------------------------------

pub async fn compact(db: &Database) -> Result<CompactResult> {
    let mut rows = db.conn.query("SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()", ()).await?;
    let size_before: i64 = match rows.next().await? {
        Some(row) => row.get(0).unwrap_or(0),
        None => 0,
    };
    db.conn.execute("VACUUM", ()).await?;
    db.conn.execute("ANALYZE", ()).await?;
    let mut rows = db.conn.query("SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()", ()).await?;
    let size_after: i64 = match rows.next().await? {
        Some(row) => row.get(0).unwrap_or(0),
        None => 0,
    };
    Ok(CompactResult { size_before, size_after, saved_bytes: size_before - size_after })
}

// ---------------------------------------------------------------------------
// GC -- garbage collection of forgotten/expired data
// ---------------------------------------------------------------------------

pub async fn gc(db: &Database, user_id: Option<i64>) -> Result<GcResult> {
    let forgotten = match user_id {
        Some(uid) => db.conn.execute(
            "DELETE FROM memories WHERE is_forgotten = 1 AND user_id = ?1",
            libsql::params![uid],
        ).await? as i64,
        None => db.conn.execute(
            "DELETE FROM memories WHERE is_forgotten = 1",
            (),
        ).await? as i64,
    };
    let expired = match user_id {
        Some(uid) => db.conn.execute(
            "DELETE FROM memories WHERE expires_at IS NOT NULL AND expires_at < datetime('now') AND user_id = ?1",
            libsql::params![uid],
        ).await? as i64,
        None => db.conn.execute(
            "DELETE FROM memories WHERE expires_at IS NOT NULL AND expires_at < datetime('now')",
            (),
        ).await? as i64,
    };
    let orphaned = 0i64;
    let old_audit = if user_id.is_none() {
        db.conn.execute(
            "DELETE FROM audit_log WHERE created_at < datetime('now', '-90 days')",
            (),
        ).await? as i64
    } else {
        0
    };
    let total = forgotten + expired + orphaned + old_audit;
    Ok(GcResult {
        total_cleaned: total,
        breakdown: GcBreakdown {
            forgotten_memories: forgotten,
            expired_memories: expired,
            orphaned_embeddings: orphaned,
            old_audit_entries: old_audit,
        },
    })
}

// ---------------------------------------------------------------------------
// Schema inspection
// ---------------------------------------------------------------------------

pub async fn get_schema(db: &Database) -> Result<SchemaResult> {
    let mut rows = db.conn.query("SELECT name, sql FROM sqlite_master WHERE type = ?1 AND name NOT LIKE ?2 ORDER BY name", libsql::params!["table", "sqlite_%"]).await?;
    let mut tables = Vec::new();
    while let Some(row) = rows.next().await? {
        tables.push(SchemaTable {
            name: row.get(0).unwrap_or_default(),
            sql: row.get(1).unwrap_or_default(),
        });
    }
    let mut rows = db.conn.query("SELECT name FROM sqlite_master WHERE type = ?1 ORDER BY name", libsql::params!["index"]).await?;
    let mut indexes = Vec::new();
    while let Some(row) = rows.next().await? {
        indexes.push(row.get::<String>(0).unwrap_or_default());
    }
    Ok(SchemaResult { tables, indexes })
}

// ---------------------------------------------------------------------------
// Maintenance mode
// ---------------------------------------------------------------------------

pub async fn get_maintenance(db: &Database) -> Result<MaintenanceStatus> {
    let mut rows = db.conn.query("SELECT value, updated_at FROM app_state WHERE key = ?1", libsql::params!["maintenance_mode"]).await?;
    match rows.next().await? {
        Some(row) => {
            let val: String = row.get(0).unwrap_or_default();
            let since: String = row.get(1).unwrap_or_default();
            let enabled = val == "1" || val == "true";
            let mut msg_rows = db.conn.query("SELECT value FROM app_state WHERE key = ?1", libsql::params!["maintenance_message"]).await?;
            let message = match msg_rows.next().await? {
                Some(r) => r.get::<String>(0).ok(),
                None => None,
            };
            Ok(MaintenanceStatus { enabled, message, since: Some(since) })
        }
        None => Ok(MaintenanceStatus { enabled: false, message: None, since: None }),
    }
}

pub async fn set_maintenance(db: &Database, enabled: bool, message: Option<&str>) -> Result<MaintenanceStatus> {
    let val = if enabled { "1" } else { "0" };
    upsert_state(db, "maintenance_mode", val).await?;
    if let Some(msg) = message {
        upsert_state(db, "maintenance_message", msg).await?;
    }
    get_maintenance(db).await
}

// ---------------------------------------------------------------------------
// SLA
// ---------------------------------------------------------------------------

pub async fn get_sla(db: &Database) -> Result<SlaResult> {
    let targets = SlaTargets::default();
    let mut rows = db.conn.query("SELECT COUNT(*) FROM audit_log", ()).await?;
    let total_requests = match rows.next().await? {
        Some(row) => row.get::<i64>(0).unwrap_or(0),
        None => 0,
    };
    let mut rows = db.conn.query("SELECT COUNT(*) FROM audit_log WHERE action LIKE '%error%'", ()).await?;
    let total_errors = match rows.next().await? {
        Some(row) => row.get::<i64>(0).unwrap_or(0),
        None => 0,
    };
    let error_rate = if total_requests > 0 { (total_errors as f64 / total_requests as f64) * 100.0 } else { 0.0 };
    Ok(SlaResult {
        targets,
        current_uptime_pct: 100.0, // Would need monitoring data
        current_error_rate_pct: error_rate,
        total_requests,
        total_errors,
    })
}

// ---------------------------------------------------------------------------
// Usage / Tenants
// ---------------------------------------------------------------------------

pub async fn get_usage(db: &Database) -> Result<Vec<UsageRow>> {
    let mut rows = db.conn.query(
        "SELECT u.id, u.username, COALESCE(m.cnt, 0), COALESCE(c.cnt, 0), COALESCE(k.cnt, 0) FROM users u LEFT JOIN (SELECT user_id, COUNT(*) as cnt FROM memories GROUP BY user_id) m ON u.id = m.user_id LEFT JOIN (SELECT user_id, COUNT(*) as cnt FROM conversations GROUP BY user_id) c ON u.id = c.user_id LEFT JOIN (SELECT user_id, COUNT(*) as cnt FROM api_keys WHERE is_active = 1 GROUP BY user_id) k ON u.id = k.user_id ORDER BY u.id",
        (),
    ).await?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().await? {
        result.push(UsageRow {
            user_id: row.get(0).unwrap_or(0),
            username: row.get(1).unwrap_or_default(),
            memory_count: row.get(2).unwrap_or(0),
            conversation_count: row.get(3).unwrap_or(0),
            api_key_count: row.get(4).unwrap_or(0),
        });
    }
    Ok(result)
}

pub async fn get_tenants(db: &Database) -> Result<Vec<TenantRow>> {
    let mut rows = db.conn.query(
        "SELECT u.id, u.username, u.role, COALESCE(m.cnt, 0), COALESCE(k.cnt, 0), u.created_at FROM users u LEFT JOIN (SELECT user_id, COUNT(*) as cnt FROM memories GROUP BY user_id) m ON u.id = m.user_id LEFT JOIN (SELECT user_id, COUNT(*) as cnt FROM api_keys WHERE is_active = 1 GROUP BY user_id) k ON u.id = k.user_id ORDER BY u.id",
        (),
    ).await?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().await? {
        result.push(TenantRow {
            id: row.get(0).unwrap_or(0),
            username: row.get(1).unwrap_or_default(),
            role: row.get(2).unwrap_or_default(),
            memory_count: row.get(3).unwrap_or(0),
            key_count: row.get(4).unwrap_or(0),
            created_at: row.get(5).unwrap_or_default(),
        });
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// Provision / Deprovision
// ---------------------------------------------------------------------------

pub async fn provision_tenant(db: &Database, username: &str, email: Option<&str>, role: &str) -> Result<ProvisionResult> {
    let is_admin = if role == "admin" { 1 } else { 0 };
    let mut user_rows = db.conn.query(
        "INSERT INTO users (username, email, role, is_admin) VALUES (?1, ?2, ?3, ?4) RETURNING id, username",
        libsql::params![username, email.map(|v| v.to_string()), role, is_admin],
    ).await?;
    let user_row = user_rows
        .next()
        .await?
        .ok_or_else(|| crate::EngError::Internal("insert user failed".into()))?;
    let user_id: i64 = user_row.get(0).map_err(|e| crate::EngError::Internal(e.to_string()))?;
    let username: String = user_row.get(1).map_err(|e| crate::EngError::Internal(e.to_string()))?;

    let mut space_rows = db.conn.query(
        "INSERT INTO spaces (user_id, name, description) VALUES (?1, ?2, ?3) RETURNING id",
        libsql::params![user_id, "default", Option::<String>::None],
    ).await?;
    let space_row = space_rows
        .next()
        .await?
        .ok_or_else(|| crate::EngError::Internal("insert default space failed".into()))?;
    let space_id: i64 = space_row.get(0).map_err(|e| crate::EngError::Internal(e.to_string()))?;

    let scopes = scopes_for_role(role);
    let (_key, raw) = crate::auth::create_key(db, user_id, "default", scopes).await?;
    Ok(ProvisionResult {
        user_id,
        username,
        api_key: raw,
        space_id,
    })
}

pub async fn deprovision_tenant(db: &Database, user_id: i64) -> Result<bool> {
    // Revoke all keys
    db.conn.execute("UPDATE api_keys SET is_active = 0 WHERE user_id = ?1", libsql::params![user_id]).await?;
    // Delete spaces
    db.conn.execute("DELETE FROM spaces WHERE user_id = ?1", libsql::params![user_id]).await?;
    // Soft-delete memories (mark forgotten)
    db.conn.execute("UPDATE memories SET is_forgotten = 1 WHERE user_id = ?1", libsql::params![user_id]).await?;
    // Delete user
    let affected = db.conn.execute("DELETE FROM users WHERE id = ?1", libsql::params![user_id]).await?;
    Ok(affected > 0)
}

// ---------------------------------------------------------------------------
// Checkpoint / Backup
// ---------------------------------------------------------------------------

pub async fn checkpoint(db: &Database) -> Result<serde_json::Value> {
    db.conn.execute("PRAGMA wal_checkpoint(TRUNCATE)", ()).await?;
    Ok(serde_json::json!({"status": "ok", "mode": "truncate"}))
}

pub async fn verify_backup(db: &Database) -> Result<BackupVerifyResult> {
    let mut rows = db.conn.query("PRAGMA integrity_check", ()).await?;
    let integrity = match rows.next().await? {
        Some(row) => row.get::<String>(0).unwrap_or_default(),
        None => "unknown".to_string(),
    };
    let ok = integrity == "ok";
    Ok(BackupVerifyResult { integrity, ok })
}

// ---------------------------------------------------------------------------
// State key-value store
// ---------------------------------------------------------------------------

pub async fn get_state(db: &Database, key: &str) -> Result<Option<StateRow>> {
    let mut rows = db.conn.query("SELECT key, value, updated_at FROM app_state WHERE key = ?1", libsql::params![key]).await?;
    match rows.next().await? {
        Some(row) => Ok(Some(StateRow {
            key: row.get(0).unwrap_or_default(),
            value: row.get(1).unwrap_or_default(),
            updated_at: row.get(2).unwrap_or_default(),
        })),
        None => Ok(None),
    }
}

pub async fn upsert_state(db: &Database, key: &str, value: &str) -> Result<()> {
    db.conn.execute("INSERT INTO app_state (key, value, updated_at) VALUES (?1, ?2, datetime('now')) ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at", libsql::params![key, value]).await?;
    Ok(())
}

pub async fn delete_state(db: &Database, key: &str) -> Result<bool> {
    let affected = db.conn.execute("DELETE FROM app_state WHERE key = ?1", libsql::params![key]).await?;
    Ok(affected > 0)
}

pub async fn list_state(db: &Database) -> Result<Vec<StateRow>> {
    let mut rows = db.conn.query("SELECT key, value, updated_at FROM app_state ORDER BY key", ()).await?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().await? {
        result.push(StateRow {
            key: row.get(0).unwrap_or_default(),
            value: row.get(1).unwrap_or_default(),
            updated_at: row.get(2).unwrap_or_default(),
        });
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

pub async fn export_user_data(db: &Database, user_id: i64) -> Result<UserExport> {
    let memories = export_table_user(db,
        "SELECT id, content, category, source, importance, tags, \
         created_at, updated_at, space_id, is_archived \
         FROM memories WHERE user_id = ?1 AND is_forgotten = 0 \
         ORDER BY created_at DESC",
        user_id,
    ).await?;
    let conversations = export_table_user(db,
        "SELECT id, session_id, agent, model, title, message_count, created_at, updated_at \
         FROM conversations WHERE user_id = ?1 ORDER BY created_at DESC",
        user_id,
    ).await?;
    let episodes = export_table_user(db,
        "SELECT id, title, summary, session_id, status, created_at, updated_at \
         FROM episodes WHERE user_id = ?1 ORDER BY created_at DESC",
        user_id,
    ).await?;
    let entities = export_table_user(db,
        "SELECT id, name, entity_type, description, metadata, created_at \
         FROM entities WHERE user_id = ?1 ORDER BY name",
        user_id,
    ).await?;
    let facts = export_table_user(db,
        "SELECT f.id, f.memory_id, f.content, f.fact_type, f.confidence, f.created_at \
         FROM facts f JOIN memories m ON f.memory_id = m.id \
         WHERE m.user_id = ?1 ORDER BY f.created_at DESC",
        user_id,
    ).await?;
    let preferences = export_table_user(db,
        "SELECT id, key, value, created_at, updated_at \
         FROM user_preferences WHERE user_id = ?1 ORDER BY key",
        user_id,
    ).await?;
    let skills = export_table_user(db,
        "SELECT id, name, description, content, language, tags, created_at \
         FROM skills WHERE user_id = ?1 ORDER BY name",
        user_id,
    ).await?;
    Ok(UserExport {
        version: "1.0".to_string(),
        exported_at: Utc::now().to_rfc3339(),
        user_id,
        memories,
        conversations,
        episodes,
        entities,
        facts,
        preferences,
        skills,
    })
}

async fn export_table_user(db: &Database, sql: &str, user_id: i64) -> Result<Vec<serde_json::Value>> {
    let mut rows = db.conn.query(sql, libsql::params![user_id]).await?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().await? {
        let mut obj = serde_json::Map::new();
        for i in 0..20 {
            match row.get::<String>(i) {
                Ok(val) => { obj.insert(format!("col_{}", i), serde_json::Value::String(val)); }
                Err(_) => break,
            }
        }
        if !obj.is_empty() {
            result.push(serde_json::Value::Object(obj));
        }
    }
    Ok(result)
}

pub async fn export_data(db: &Database) -> Result<ExportData> {
    let users = export_table(db, "SELECT * FROM users").await?;
    let memories = export_table(db, "SELECT id, content, category, source, importance, user_id, space_id, created_at FROM memories WHERE is_forgotten = 0").await?;
    let conversations = export_table(db, "SELECT * FROM conversations").await?;
    let api_keys = export_table(db, "SELECT id, user_id, key_prefix, name, scopes, rate_limit, is_active, created_at FROM api_keys").await?;
    Ok(ExportData { users, memories, conversations, api_keys })
}

async fn export_table(db: &Database, sql: &str) -> Result<Vec<serde_json::Value>> {
    let mut rows = db.conn.query(sql, ()).await?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().await? {
        // Build a JSON object from row columns
        let mut obj = serde_json::Map::new();
        for i in 0..20 {
            match row.get::<String>(i) {
                Ok(val) => { obj.insert(format!("col_{}", i), serde_json::Value::String(val)); }
                Err(_) => break,
            }
        }
        if !obj.is_empty() {
            result.push(serde_json::Value::Object(obj));
        }
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// Export -- user-scoped
// ---------------------------------------------------------------------------

pub async fn export_user_data(db: &Database, user_id: i64) -> Result<ExportData> {
    let sql_memories = format!(
        "SELECT id, content, category, source, importance, user_id, space_id, created_at \
         FROM memories WHERE is_forgotten = 0 AND user_id = {}",
        user_id
    );
    let memories = export_table(db, &sql_memories).await?;
    let sql_conv = format!(
        "SELECT * FROM conversations WHERE user_id = {}",
        user_id
    );
    let conversations = export_table(db, &sql_conv).await?;
    let sql_keys = format!(
        "SELECT id, user_id, key_prefix, name, scopes, rate_limit, is_active, created_at \
         FROM api_keys WHERE user_id = {}",
        user_id
    );
    let api_keys = export_table(db, &sql_keys).await?;
    Ok(ExportData { users: vec![], memories, conversations, api_keys })
}

// ---------------------------------------------------------------------------
// Re-embed: clear embeddings so they get regenerated
// ---------------------------------------------------------------------------

pub async fn reembed_all(db: &Database, user_id: Option<i64>) -> Result<i64> {
    let affected = match user_id {
        Some(uid) => db.conn.execute(
            "UPDATE memories SET embedding = NULL WHERE user_id = ?1 AND is_forgotten = 0",
            libsql::params![uid],
        ).await?,
        None => db.conn.execute(
            "UPDATE memories SET embedding = NULL WHERE is_forgotten = 0",
            (),
        ).await?,
    };
    Ok(affected as i64)
}

// ---------------------------------------------------------------------------
// Backfill: fetch memories without structured facts
// ---------------------------------------------------------------------------

pub async fn get_memories_without_facts(db: &Database, limit: i64) -> Result<Vec<(i64, String, i64)>> {
    let mut rows = db.conn.query(
        "SELECT m.id, m.content, m.user_id FROM memories m \
         WHERE m.is_forgotten = 0 \
         AND NOT EXISTS (SELECT 1 FROM structured_facts f WHERE f.memory_id = m.id) \
         LIMIT ?1",
        libsql::params![limit],
    ).await?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().await? {
        result.push((
            row.get(0).unwrap_or(0),
            row.get(1).unwrap_or_default(),
            row.get(2).unwrap_or(0),
        ));
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// Rebuild FTS index
// ---------------------------------------------------------------------------

pub async fn rebuild_fts(db: &Database) -> Result<i64> {
    db.conn.execute(
        "INSERT INTO memories_fts(memories_fts) VALUES('rebuild')",
        (),
    ).await?;
    let mut rows = db.conn.query("SELECT COUNT(*) FROM memories_fts", ()).await?;
    match rows.next().await? {
        Some(row) => Ok(row.get(0).unwrap_or(0)),
        None => Ok(0),
    }
}

// ---------------------------------------------------------------------------
// Scale report
// ---------------------------------------------------------------------------

pub async fn scale_report(db: &Database) -> Result<serde_json::Value> {
    let tables = &[
        "memories", "conversations", "messages", "episodes", "entities",
        "structured_facts", "skills", "events", "action_log", "tasks", "agents",
        "api_keys", "audit_log", "webhooks", "user_preferences",
    ];
    let mut counts = serde_json::Map::new();
    for table in tables {
        let sql = format!("SELECT COUNT(*) FROM {}", table);
        match db.conn.query(&sql, ()).await {
            Ok(mut rows) => {
                let count: i64 = match rows.next().await {
                    Ok(Some(row)) => row.get(0).unwrap_or(0),
                    _ => 0,
                };
                counts.insert(table.to_string(), serde_json::json!(count));
            }
            Err(_) => {
                counts.insert(table.to_string(), serde_json::json!("table not found"));
            }
        }
    }
    let mut rows = db.conn.query(
        "SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()",
        (),
    ).await?;
    let db_size: i64 = match rows.next().await? {
        Some(row) => row.get(0).unwrap_or(0),
        None => 0,
    };
    Ok(serde_json::json!({ "table_counts": counts, "database_size_bytes": db_size }))
}

// ---------------------------------------------------------------------------
// Cold storage stats
// ---------------------------------------------------------------------------

pub async fn cold_storage_stats(db: &Database, days: i64) -> Result<serde_json::Value> {
    let threshold = format!("-{} days", days);
    let mut rows = db.conn.query(
        "SELECT COUNT(*) FROM memories \
         WHERE is_forgotten = 0 AND is_archived = 0 \
         AND created_at < datetime('now', ?1)",
        libsql::params![threshold],
    ).await?;
    let eligible: i64 = match rows.next().await? {
        Some(row) => row.get(0).unwrap_or(0),
        None => 0,
    };
    Ok(serde_json::json!({ "eligible_count": eligible, "threshold_days": days }))
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

pub async fn get_stats(db: &Database) -> Result<serde_json::Value> {
    let mut rows = db.conn.query("SELECT COUNT(*) FROM memories WHERE is_forgotten = 0", ()).await?;
    let memory_count: i64 = match rows.next().await? { Some(r) => r.get(0).unwrap_or(0), None => 0 };
    let mut rows = db.conn.query("SELECT COUNT(*) FROM users", ()).await?;
    let user_count: i64 = match rows.next().await? { Some(r) => r.get(0).unwrap_or(0), None => 0 };
    let mut rows = db.conn.query("SELECT COUNT(*) FROM api_keys WHERE is_active = 1", ()).await?;
    let key_count: i64 = match rows.next().await? { Some(r) => r.get(0).unwrap_or(0), None => 0 };
    let mut rows = db.conn.query("SELECT COUNT(*) FROM conversations", ()).await?;
    let conv_count: i64 = match rows.next().await? { Some(r) => r.get(0).unwrap_or(0), None => 0 };
    Ok(serde_json::json!({
        "memories": memory_count,
        "users": user_count,
        "api_keys": key_count,
        "conversations": conv_count,
    }))
}
