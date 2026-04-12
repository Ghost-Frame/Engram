//! Admin operations -- ported from TS admin/db.ts + admin/operations.ts

pub mod types;

use self::types::*;
use crate::db::Database;
use crate::{EngError, Result};
use chrono::Utc;
use rusqlite::params;

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

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
    let size_before: i64 = db
        .read(|conn| {
            conn.query_row(
                "SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()",
                [],
                |row| row.get(0),
            )
            .map_err(rusqlite_to_eng_error)
        })
        .await?;

    db.write(|conn| {
        conn.execute_batch("VACUUM; ANALYZE").map_err(rusqlite_to_eng_error)
    })
    .await?;

    let size_after: i64 = db
        .read(|conn| {
            conn.query_row(
                "SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()",
                [],
                |row| row.get(0),
            )
            .map_err(rusqlite_to_eng_error)
        })
        .await?;

    Ok(CompactResult {
        size_before,
        size_after,
        saved_bytes: size_before - size_after,
    })
}

// ---------------------------------------------------------------------------
// GC -- garbage collection of forgotten/expired data
// ---------------------------------------------------------------------------

pub async fn gc(db: &Database, user_id: Option<i64>) -> Result<GcResult> {
    let forgotten: i64 = db
        .write(move |conn| {
            let n = if let Some(uid) = user_id {
                conn.execute(
                    "DELETE FROM memories WHERE is_forgotten = 1 AND user_id = ?1",
                    params![uid],
                )
                .map_err(rusqlite_to_eng_error)?
            } else {
                conn.execute("DELETE FROM memories WHERE is_forgotten = 1", [])
                    .map_err(rusqlite_to_eng_error)?
            };
            Ok(n as i64)
        })
        .await?;

    let expired: i64 = db
        .write(move |conn| {
            let n = if let Some(uid) = user_id {
                conn.execute(
                    "DELETE FROM memories WHERE forget_after IS NOT NULL AND forget_after < datetime('now') AND user_id = ?1",
                    params![uid],
                )
                .map_err(rusqlite_to_eng_error)?
            } else {
                conn.execute(
                    "DELETE FROM memories WHERE forget_after IS NOT NULL AND forget_after < datetime('now')",
                    [],
                )
                .map_err(rusqlite_to_eng_error)?
            };
            Ok(n as i64)
        })
        .await?;

    let orphaned = 0i64;

    let old_audit: i64 = if user_id.is_none() {
        db.write(|conn| {
            let n = conn
                .execute(
                    "DELETE FROM audit_log WHERE created_at < datetime('now', '-90 days')",
                    [],
                )
                .map_err(rusqlite_to_eng_error)?;
            Ok(n as i64)
        })
        .await?
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
    let tables: Vec<SchemaTable> = db
        .read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT name, sql FROM sqlite_master WHERE type = ?1 AND name NOT LIKE ?2 ORDER BY name",
                )
                .map_err(rusqlite_to_eng_error)?;
            let rows = stmt
                .query_map(params!["table", "sqlite_%"], |row| {
                    Ok(SchemaTable {
                        name: row.get(0)?,
                        sql: row.get(1)?,
                    })
                })
                .map_err(rusqlite_to_eng_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(rusqlite_to_eng_error)?;
            Ok(rows)
        })
        .await?;

    let indexes: Vec<String> = db
        .read(|conn| {
            let mut stmt = conn
                .prepare("SELECT name FROM sqlite_master WHERE type = ?1 ORDER BY name")
                .map_err(rusqlite_to_eng_error)?;
            let rows = stmt
                .query_map(params!["index"], |row| row.get(0))
                .map_err(rusqlite_to_eng_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(rusqlite_to_eng_error)?;
            Ok(rows)
        })
        .await?;

    Ok(SchemaResult { tables, indexes })
}

// ---------------------------------------------------------------------------
// Maintenance mode
// ---------------------------------------------------------------------------

pub async fn get_maintenance(db: &Database) -> Result<MaintenanceStatus> {
    let row_opt: Option<(String, String)> = db
        .read(|conn| {
            let mut stmt = conn
                .prepare("SELECT value, updated_at FROM app_state WHERE key = ?1")
                .map_err(rusqlite_to_eng_error)?;
            let mut rows = stmt
                .query(params!["maintenance_mode"])
                .map_err(rusqlite_to_eng_error)?;
            if let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                let val: String = row.get(0).map_err(rusqlite_to_eng_error)?;
                let since: String = row.get(1).map_err(rusqlite_to_eng_error)?;
                Ok(Some((val, since)))
            } else {
                Ok(None)
            }
        })
        .await?;

    match row_opt {
        Some((val, since)) => {
            let enabled = val == "1" || val == "true";
            let message: Option<String> = db
                .read(|conn| {
                    let mut stmt = conn
                        .prepare("SELECT value FROM app_state WHERE key = ?1")
                        .map_err(rusqlite_to_eng_error)?;
                    let mut rows = stmt
                        .query(params!["maintenance_message"])
                        .map_err(rusqlite_to_eng_error)?;
                    if let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                        Ok(row.get(0).map_err(rusqlite_to_eng_error)?)
                    } else {
                        Ok(None)
                    }
                })
                .await?;
            Ok(MaintenanceStatus {
                enabled,
                message,
                since: Some(since),
            })
        }
        None => Ok(MaintenanceStatus {
            enabled: false,
            message: None,
            since: None,
        }),
    }
}

pub async fn set_maintenance(
    db: &Database,
    enabled: bool,
    message: Option<&str>,
) -> Result<MaintenanceStatus> {
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

    let total_requests: i64 = db
        .read(|conn| {
            conn.query_row("SELECT COUNT(*) FROM audit_log", [], |row| row.get(0))
                .map_err(rusqlite_to_eng_error)
        })
        .await?;

    let total_errors: i64 = db
        .read(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM audit_log WHERE action LIKE '%error%'",
                [],
                |row| row.get(0),
            )
            .map_err(rusqlite_to_eng_error)
        })
        .await?;

    let error_rate = if total_requests > 0 {
        (total_errors as f64 / total_requests as f64) * 100.0
    } else {
        0.0
    };

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
    db.read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT u.id, u.username, COALESCE(m.cnt, 0), COALESCE(c.cnt, 0), COALESCE(k.cnt, 0) \
             FROM users u \
             LEFT JOIN (SELECT user_id, COUNT(*) as cnt FROM memories GROUP BY user_id) m ON u.id = m.user_id \
             LEFT JOIN (SELECT user_id, COUNT(*) as cnt FROM conversations GROUP BY user_id) c ON u.id = c.user_id \
             LEFT JOIN (SELECT user_id, COUNT(*) as cnt FROM api_keys WHERE is_active = 1 GROUP BY user_id) k ON u.id = k.user_id \
             ORDER BY u.id",
        )
        .map_err(rusqlite_to_eng_error)?;
        let rows = stmt
            .query_map([], |row| {
                Ok(UsageRow {
                    user_id: row.get(0)?,
                    username: row.get(1)?,
                    memory_count: row.get(2)?,
                    conversation_count: row.get(3)?,
                    api_key_count: row.get(4)?,
                })
            })
            .map_err(rusqlite_to_eng_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(rusqlite_to_eng_error)?;
        Ok(rows)
    })
    .await
}

pub async fn get_tenants(db: &Database) -> Result<Vec<TenantRow>> {
    db.read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT u.id, u.username, u.role, COALESCE(m.cnt, 0), COALESCE(k.cnt, 0), u.created_at \
             FROM users u \
             LEFT JOIN (SELECT user_id, COUNT(*) as cnt FROM memories GROUP BY user_id) m ON u.id = m.user_id \
             LEFT JOIN (SELECT user_id, COUNT(*) as cnt FROM api_keys WHERE is_active = 1 GROUP BY user_id) k ON u.id = k.user_id \
             ORDER BY u.id",
        )
        .map_err(rusqlite_to_eng_error)?;
        let rows = stmt
            .query_map([], |row| {
                Ok(TenantRow {
                    id: row.get(0)?,
                    username: row.get(1)?,
                    role: row.get(2)?,
                    memory_count: row.get(3)?,
                    key_count: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })
            .map_err(rusqlite_to_eng_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(rusqlite_to_eng_error)?;
        Ok(rows)
    })
    .await
}

// ---------------------------------------------------------------------------
// Provision / Deprovision
// ---------------------------------------------------------------------------

pub async fn provision_tenant(
    db: &Database,
    username: &str,
    email: Option<&str>,
    role: &str,
) -> Result<ProvisionResult> {
    let is_admin = if role == "admin" { 1i32 } else { 0i32 };
    let username_owned = username.to_string();
    let email_owned = email.map(|v| v.to_string());
    let role_owned = role.to_string();

    let (user_id, returned_username, space_id) = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO users (username, email, role, is_admin) VALUES (?1, ?2, ?3, ?4)",
                params![username_owned, email_owned, role_owned, is_admin],
            )
            .map_err(rusqlite_to_eng_error)?;
            let user_id = conn.last_insert_rowid();
            let returned_username: String = conn
                .query_row(
                    "SELECT username FROM users WHERE id = ?1",
                    params![user_id],
                    |row| row.get(0),
                )
                .map_err(rusqlite_to_eng_error)?;

            conn.execute(
                "INSERT INTO spaces (user_id, name, description) VALUES (?1, ?2, ?3)",
                params![user_id, "default", Option::<String>::None],
            )
            .map_err(rusqlite_to_eng_error)?;
            let space_id = conn.last_insert_rowid();

            Ok((user_id, returned_username, space_id))
        })
        .await?;

    let scopes = scopes_for_role(role);
    let (_key, raw) = crate::auth::create_key(db, user_id, "default", scopes, None).await?;
    Ok(ProvisionResult {
        user_id,
        username: returned_username,
        api_key: raw,
        space_id,
    })
}

pub async fn deprovision_tenant(db: &Database, user_id: i64) -> Result<bool> {
    db.write(move |conn| {
        // Revoke all keys
        conn.execute(
            "UPDATE api_keys SET is_active = 0 WHERE user_id = ?1",
            params![user_id],
        )
        .map_err(rusqlite_to_eng_error)?;
        // Delete spaces
        conn.execute(
            "DELETE FROM spaces WHERE user_id = ?1",
            params![user_id],
        )
        .map_err(rusqlite_to_eng_error)?;
        // Soft-delete memories (mark forgotten)
        conn.execute(
            "UPDATE memories SET is_forgotten = 1 WHERE user_id = ?1",
            params![user_id],
        )
        .map_err(rusqlite_to_eng_error)?;
        // Delete user
        let affected = conn
            .execute("DELETE FROM users WHERE id = ?1", params![user_id])
            .map_err(rusqlite_to_eng_error)?;
        Ok(affected > 0)
    })
    .await
}

// ---------------------------------------------------------------------------
// Checkpoint / Backup
// ---------------------------------------------------------------------------

pub async fn checkpoint(db: &Database) -> Result<serde_json::Value> {
    db.write(|conn| {
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .map_err(rusqlite_to_eng_error)
    })
    .await?;
    Ok(serde_json::json!({"status": "ok", "mode": "truncate"}))
}

pub async fn verify_backup(db: &Database) -> Result<BackupVerifyResult> {
    let integrity: String = db
        .read(|conn| {
            conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))
                .map_err(rusqlite_to_eng_error)
        })
        .await
        .unwrap_or_else(|_| "unknown".to_string());
    let ok = integrity == "ok";
    Ok(BackupVerifyResult { integrity, ok })
}

// ---------------------------------------------------------------------------
// State key-value store
// ---------------------------------------------------------------------------

pub async fn get_state(db: &Database, key: &str) -> Result<Option<StateRow>> {
    let key_owned = key.to_string();
    db.read(move |conn| {
        let mut stmt = conn
            .prepare("SELECT key, value, updated_at FROM app_state WHERE key = ?1")
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(params![key_owned])
            .map_err(rusqlite_to_eng_error)?;
        if let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            Ok(Some(StateRow {
                key: row.get(0).map_err(rusqlite_to_eng_error)?,
                value: row.get(1).map_err(rusqlite_to_eng_error)?,
                updated_at: row.get(2).map_err(rusqlite_to_eng_error)?,
            }))
        } else {
            Ok(None)
        }
    })
    .await
}

pub async fn upsert_state(db: &Database, key: &str, value: &str) -> Result<()> {
    let key_owned = key.to_string();
    let value_owned = value.to_string();
    db.write(move |conn| {
        conn.execute(
            "INSERT INTO app_state (key, value, updated_at) VALUES (?1, ?2, datetime('now')) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![key_owned, value_owned],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

pub async fn delete_state(db: &Database, key: &str) -> Result<bool> {
    let key_owned = key.to_string();
    db.write(move |conn| {
        let affected = conn
            .execute("DELETE FROM app_state WHERE key = ?1", params![key_owned])
            .map_err(rusqlite_to_eng_error)?;
        Ok(affected > 0)
    })
    .await
}

pub async fn list_state(db: &Database) -> Result<Vec<StateRow>> {
    db.read(|conn| {
        let mut stmt = conn
            .prepare("SELECT key, value, updated_at FROM app_state ORDER BY key")
            .map_err(rusqlite_to_eng_error)?;
        let rows = stmt
            .query_map([], |row| {
                Ok(StateRow {
                    key: row.get(0)?,
                    value: row.get(1)?,
                    updated_at: row.get(2)?,
                })
            })
            .map_err(rusqlite_to_eng_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(rusqlite_to_eng_error)?;
        Ok(rows)
    })
    .await
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

pub async fn export_user_data(db: &Database, user_id: i64) -> Result<UserExport> {
    let memories = export_table_user(
        db,
        "SELECT id, content, category, source, importance, tags, \
         created_at, updated_at, space_id, is_archived \
         FROM memories WHERE user_id = ?1 AND is_forgotten = 0 \
         ORDER BY created_at DESC",
        user_id,
    )
    .await?;
    let conversations = export_table_user(
        db,
        "SELECT id, session_id, agent, model, title, message_count, created_at, updated_at \
         FROM conversations WHERE user_id = ?1 ORDER BY created_at DESC",
        user_id,
    )
    .await?;
    let episodes = export_table_user(
        db,
        "SELECT id, title, summary, session_id, status, created_at, updated_at \
         FROM episodes WHERE user_id = ?1 ORDER BY created_at DESC",
        user_id,
    )
    .await?;
    let entities = export_table_user(
        db,
        "SELECT id, name, entity_type, description, metadata, created_at \
         FROM entities WHERE user_id = ?1 ORDER BY name",
        user_id,
    )
    .await?;
    let facts = export_table_user(
        db,
        "SELECT id, memory_id, subject, predicate, object, confidence, created_at \
         FROM structured_facts \
         WHERE user_id = ?1 ORDER BY created_at DESC",
        user_id,
    )
    .await?;
    let preferences = export_table_user(
        db,
        "SELECT id, key, value, created_at, updated_at \
         FROM user_preferences WHERE user_id = ?1 ORDER BY key",
        user_id,
    )
    .await?;
    let skills = export_table_user(
        db,
        "SELECT id, name, description, content, language, tags, created_at \
         FROM skills WHERE user_id = ?1 ORDER BY name",
        user_id,
    )
    .await?;
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

async fn export_table_user(
    db: &Database,
    sql: &str,
    user_id: i64,
) -> Result<Vec<serde_json::Value>> {
    let sql_owned = sql.to_string();
    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql_owned).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(params![user_id])
            .map_err(rusqlite_to_eng_error)?;
        let mut result = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            let mut obj = serde_json::Map::new();
            for i in 0..20usize {
                match row.get::<_, String>(i) {
                    Ok(val) => {
                        obj.insert(format!("col_{}", i), serde_json::Value::String(val));
                    }
                    Err(_) => break,
                }
            }
            if !obj.is_empty() {
                result.push(serde_json::Value::Object(obj));
            }
        }
        Ok(result)
    })
    .await
}

pub async fn export_data(db: &Database) -> Result<ExportData> {
    let users = export_table(db, "SELECT * FROM users").await?;
    let memories = export_table(db, "SELECT id, content, category, source, importance, user_id, space_id, created_at FROM memories WHERE is_forgotten = 0").await?;
    let conversations = export_table(db, "SELECT * FROM conversations").await?;
    let api_keys = export_table(db, "SELECT id, user_id, key_prefix, name, scopes, rate_limit, is_active, created_at FROM api_keys").await?;
    Ok(ExportData {
        users,
        memories,
        conversations,
        api_keys,
    })
}

async fn export_table(db: &Database, sql: &str) -> Result<Vec<serde_json::Value>> {
    let sql_owned = sql.to_string();
    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql_owned).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt.query([]).map_err(rusqlite_to_eng_error)?;
        let mut result = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            let mut obj = serde_json::Map::new();
            for i in 0..20usize {
                match row.get::<_, String>(i) {
                    Ok(val) => {
                        obj.insert(format!("col_{}", i), serde_json::Value::String(val));
                    }
                    Err(_) => break,
                }
            }
            if !obj.is_empty() {
                result.push(serde_json::Value::Object(obj));
            }
        }
        Ok(result)
    })
    .await
}

// ---------------------------------------------------------------------------
// Re-embed: clear embeddings so they get regenerated
// ---------------------------------------------------------------------------

/// Clear embeddings on every live memory so ingestion regenerates them on
/// next access. Both the legacy `embedding` BLOB column and the active
/// `embedding_vec_1024` column are cleared; clearing only the legacy
/// column would leave the live index serving stale vectors and is the
/// bug pre-round-5 callers hit when swapping models.
pub async fn reembed_all(db: &Database, user_id: Option<i64>) -> Result<i64> {
    db.write(move |conn| {
        let n = if let Some(uid) = user_id {
            conn.execute(
                "UPDATE memories SET embedding = NULL, embedding_vec_1024 = NULL \
                 WHERE user_id = ?1 AND is_forgotten = 0",
                params![uid],
            )
            .map_err(rusqlite_to_eng_error)?
        } else {
            conn.execute(
                "UPDATE memories SET embedding = NULL, embedding_vec_1024 = NULL \
                 WHERE is_forgotten = 0",
                [],
            )
            .map_err(rusqlite_to_eng_error)?
        };
        Ok(n as i64)
    })
    .await
}

// ---------------------------------------------------------------------------
// Backfill: fetch memories without structured facts
// ---------------------------------------------------------------------------

pub async fn get_memories_without_facts(
    db: &Database,
    limit: i64,
) -> Result<Vec<(i64, String, i64)>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT m.id, m.content, m.user_id FROM memories m \
                 WHERE m.is_forgotten = 0 \
                 AND NOT EXISTS (SELECT 1 FROM structured_facts f WHERE f.memory_id = m.id) \
                 LIMIT ?1",
            )
            .map_err(rusqlite_to_eng_error)?;
        let rows = stmt
            .query_map(params![limit], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(rusqlite_to_eng_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(rusqlite_to_eng_error)?;
        Ok(rows)
    })
    .await
}

// ---------------------------------------------------------------------------
// Rebuild FTS index
// ---------------------------------------------------------------------------

pub async fn rebuild_fts(db: &Database) -> Result<i64> {
    db.write(|conn| {
        conn.execute(
            "INSERT INTO memories_fts(memories_fts) VALUES('rebuild')",
            [],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await?;

    db.read(|conn| {
        conn.query_row("SELECT COUNT(*) FROM memories_fts", [], |row| row.get(0))
            .map_err(rusqlite_to_eng_error)
    })
    .await
}

// ---------------------------------------------------------------------------
// Scale report
// ---------------------------------------------------------------------------

pub async fn scale_report(db: &Database) -> Result<serde_json::Value> {
    let tables = &[
        "memories",
        "conversations",
        "messages",
        "episodes",
        "entities",
        "structured_facts",
        "skills",
        "events",
        "action_log",
        "tasks",
        "agents",
        "api_keys",
        "audit_log",
        "webhooks",
        "user_preferences",
    ];
    let mut counts = serde_json::Map::new();
    for table in tables {
        let sql = format!("SELECT COUNT(*) FROM {}", table);
        let result = db
            .read(move |conn| {
                conn.query_row(&sql, [], |row| row.get::<_, i64>(0))
                    .map_err(rusqlite_to_eng_error)
            })
            .await;
        match result {
            Ok(count) => {
                counts.insert(table.to_string(), serde_json::json!(count));
            }
            Err(_) => {
                counts.insert(table.to_string(), serde_json::json!("table not found"));
            }
        }
    }

    let db_size: i64 = db
        .read(|conn| {
            conn.query_row(
                "SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()",
                [],
                |row| row.get(0),
            )
            .map_err(rusqlite_to_eng_error)
        })
        .await?;

    Ok(serde_json::json!({ "table_counts": counts, "database_size_bytes": db_size }))
}

// ---------------------------------------------------------------------------
// Cold storage stats
// ---------------------------------------------------------------------------

pub async fn cold_storage_stats(db: &Database, days: i64) -> Result<serde_json::Value> {
    let threshold = format!("-{} days", days);
    let eligible: i64 = db
        .read(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM memories \
                 WHERE is_forgotten = 0 AND is_archived = 0 \
                 AND created_at < datetime('now', ?1)",
                params![threshold],
                |row| row.get(0),
            )
            .map_err(rusqlite_to_eng_error)
        })
        .await?;
    Ok(serde_json::json!({ "eligible_count": eligible, "threshold_days": days }))
}

// ---------------------------------------------------------------------------
// Crash-loop detection
// ---------------------------------------------------------------------------

const CRASH_WINDOW_KEY: &str = "crash_window";
const CRASH_WINDOW_SECONDS: i64 = 300; // 5 minutes
const CRASH_THRESHOLD: usize = 3;

/// Record the current timestamp as a crash/restart event.
/// Prunes timestamps older than the 5-minute window before saving.
pub async fn record_crash(db: &Database) -> Result<()> {
    let now = Utc::now();
    let cutoff = now - chrono::Duration::seconds(CRASH_WINDOW_SECONDS);

    // Load existing timestamps.
    let existing = match get_state(db, CRASH_WINDOW_KEY).await? {
        Some(row) => serde_json::from_str::<Vec<String>>(&row.value).unwrap_or_default(),
        None => Vec::new(),
    };

    // Keep only timestamps within the window, then append now.
    let mut timestamps: Vec<String> = existing
        .into_iter()
        .filter(|ts| {
            chrono::DateTime::parse_from_rfc3339(ts)
                .map(|t| t.with_timezone(&Utc) > cutoff)
                .unwrap_or(false)
        })
        .collect();
    timestamps.push(now.to_rfc3339());

    let value = serde_json::to_string(&timestamps)
        .map_err(|e| crate::EngError::Internal(format!("serialize crash window: {e}")))?;
    upsert_state(db, CRASH_WINDOW_KEY, &value).await?;
    Ok(())
}

/// Returns true when there have been >= 3 crash/restart events in the last 5 minutes.
pub async fn should_enter_safe_mode(db: &Database) -> Result<bool> {
    let cutoff = Utc::now() - chrono::Duration::seconds(CRASH_WINDOW_SECONDS);

    let existing = match get_state(db, CRASH_WINDOW_KEY).await? {
        Some(row) => serde_json::from_str::<Vec<String>>(&row.value).unwrap_or_default(),
        None => return Ok(false),
    };

    let recent = existing
        .iter()
        .filter(|ts| {
            chrono::DateTime::parse_from_rfc3339(ts)
                .map(|t| t.with_timezone(&Utc) > cutoff)
                .unwrap_or(false)
        })
        .count();

    Ok(recent >= CRASH_THRESHOLD)
}

/// Clear the crash window. Called when exiting safe mode manually.
pub async fn clear_crash_window(db: &Database) -> Result<()> {
    delete_state(db, CRASH_WINDOW_KEY).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

pub async fn get_stats(db: &Database) -> Result<serde_json::Value> {
    let memory_count: i64 = db
        .read(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM memories WHERE is_forgotten = 0",
                [],
                |row| row.get(0),
            )
            .map_err(rusqlite_to_eng_error)
        })
        .await?;

    let user_count: i64 = db
        .read(|conn| {
            conn.query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
                .map_err(rusqlite_to_eng_error)
        })
        .await?;

    let key_count: i64 = db
        .read(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM api_keys WHERE is_active = 1",
                [],
                |row| row.get(0),
            )
            .map_err(rusqlite_to_eng_error)
        })
        .await?;

    let conv_count: i64 = db
        .read(|conn| {
            conn.query_row("SELECT COUNT(*) FROM conversations", [], |row| row.get(0))
                .map_err(rusqlite_to_eng_error)
        })
        .await?;

    Ok(serde_json::json!({
        "memories": memory_count,
        "users": user_count,
        "api_keys": key_count,
        "conversations": conv_count,
    }))
}
