//! Audit logging for credential access.

use kleos_lib::db::Database;
use kleos_lib::EngError;

use crate::{CredError, Result};

#[allow(dead_code)]
fn rusqlite_to_cred_error(err: rusqlite::Error) -> CredError {
    CredError::Database(err.to_string())
}

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

/// Audit log entry.
#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub id: i64,
    pub user_id: i64,
    pub agent_name: Option<String>,
    pub action: String,
    pub category: String,
    pub secret_name: String,
    pub access_tier: Option<String>,
    pub success: bool,
    pub timestamp: String,
}

/// Actions that can be audited.
#[derive(Debug, Clone, Copy)]
pub enum AuditAction {
    Get,
    Set,
    Update,
    Delete,
    Resolve,
    Proxy,
}

impl AuditAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Set => "set",
            Self::Update => "update",
            Self::Delete => "delete",
            Self::Resolve => "resolve",
            Self::Proxy => "proxy",
        }
    }
}

/// Access tiers for auditing.
#[derive(Debug, Clone, Copy)]
pub enum AccessTier {
    Substitution,
    Proxy,
    Raw,
}

impl AccessTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Substitution => "substitution",
            Self::Proxy => "proxy",
            Self::Raw => "raw",
        }
    }
}

/// Log an audit entry.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip(db), fields(user_id, agent_name = ?agent_name, action = ?action, category = %category, secret_name = %secret_name, access_tier = ?access_tier, success))]
pub async fn log_audit(
    db: &Database,
    user_id: i64,
    agent_name: Option<&str>,
    action: AuditAction,
    category: &str,
    secret_name: &str,
    access_tier: Option<AccessTier>,
    success: bool,
) -> Result<i64> {
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let action_str = action.as_str().to_string();
    let tier_str = access_tier.map(|t| t.as_str().to_string());
    let agent_name_owned = agent_name.map(|s| s.to_string());
    let category_owned = category.to_string();
    let secret_name_owned = secret_name.to_string();

    db.write(move |conn| {
        conn.execute(
            "INSERT INTO cred_audit (user_id, agent_name, action, category, secret_name, access_tier, success, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                user_id,
                agent_name_owned,
                action_str,
                category_owned,
                secret_name_owned,
                tier_str,
                success as i32,
                now
            ],
        )
        .map_err(rusqlite_to_eng_error)?;

        Ok(conn.last_insert_rowid())
    })
    .await
    .map_err(|e| CredError::Database(e.to_string()))
}

fn collect_audit_rows(
    stmt: &mut rusqlite::Statement<'_>,
    params: &[&dyn rusqlite::types::ToSql],
) -> kleos_lib::Result<Vec<AuditEntry>> {
    let v: Vec<AuditEntry> = stmt
        .query_map(params, row_to_audit_entry)
        .map_err(rusqlite_to_eng_error)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(rusqlite_to_eng_error)?;
    Ok(v)
}

/// Query audit entries for a user.
#[tracing::instrument(skip(db), fields(user_id, limit, category = ?category, agent_name = ?agent_name))]
pub async fn query_audit(
    db: &Database,
    user_id: i64,
    limit: usize,
    category: Option<&str>,
    agent_name: Option<&str>,
) -> Result<Vec<AuditEntry>> {
    let category_owned = category.map(|s| s.to_string());
    let agent_name_owned = agent_name.map(|s| s.to_string());
    let limit_i64 = limit as i64;

    db.read(move |conn| {
        match (&category_owned, &agent_name_owned) {
            (Some(cat), Some(agent)) => {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, user_id, agent_name, action, category, secret_name, access_tier, success, timestamp
                         FROM cred_audit
                         WHERE user_id = ?1 AND category = ?2 AND agent_name = ?3
                         ORDER BY timestamp DESC
                         LIMIT ?4",
                    )
                    .map_err(rusqlite_to_eng_error)?;
                collect_audit_rows(&mut stmt, rusqlite::params![user_id, cat, agent, limit_i64])
            }
            (Some(cat), None) => {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, user_id, agent_name, action, category, secret_name, access_tier, success, timestamp
                         FROM cred_audit
                         WHERE user_id = ?1 AND category = ?2
                         ORDER BY timestamp DESC
                         LIMIT ?3",
                    )
                    .map_err(rusqlite_to_eng_error)?;
                collect_audit_rows(&mut stmt, rusqlite::params![user_id, cat, limit_i64])
            }
            (None, Some(agent)) => {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, user_id, agent_name, action, category, secret_name, access_tier, success, timestamp
                         FROM cred_audit
                         WHERE user_id = ?1 AND agent_name = ?2
                         ORDER BY timestamp DESC
                         LIMIT ?3",
                    )
                    .map_err(rusqlite_to_eng_error)?;
                collect_audit_rows(&mut stmt, rusqlite::params![user_id, agent, limit_i64])
            }
            (None, None) => {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, user_id, agent_name, action, category, secret_name, access_tier, success, timestamp
                         FROM cred_audit
                         WHERE user_id = ?1
                         ORDER BY timestamp DESC
                         LIMIT ?2",
                    )
                    .map_err(rusqlite_to_eng_error)?;
                collect_audit_rows(&mut stmt, rusqlite::params![user_id, limit_i64])
            }
        }
    })
    .await
    .map_err(|e| CredError::Database(e.to_string()))
}

/// Get audit entries for a specific secret.
#[tracing::instrument(skip(db), fields(user_id, category = %category, secret_name = %secret_name, limit))]
pub async fn get_secret_audit(
    db: &Database,
    user_id: i64,
    category: &str,
    secret_name: &str,
    limit: usize,
) -> Result<Vec<AuditEntry>> {
    let category_owned = category.to_string();
    let secret_name_owned = secret_name.to_string();
    let limit_i64 = limit as i64;

    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, user_id, agent_name, action, category, secret_name, access_tier, success, timestamp
                 FROM cred_audit
                 WHERE user_id = ?1 AND category = ?2 AND secret_name = ?3
                 ORDER BY timestamp DESC
                 LIMIT ?4",
            )
            .map_err(rusqlite_to_eng_error)?;

        collect_audit_rows(
            &mut stmt,
            rusqlite::params![user_id, category_owned, secret_name_owned, limit_i64],
        )
    })
    .await
    .map_err(|e| CredError::Database(e.to_string()))
}

fn row_to_audit_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<AuditEntry> {
    let id: i64 = row.get(0)?;
    let user_id: i64 = row.get(1)?;
    let agent_name: Option<String> = row.get(2)?;
    let action: String = row.get(3)?;
    let category: String = row.get(4)?;
    let secret_name: String = row.get(5)?;
    let access_tier: Option<String> = row.get(6)?;
    let success: i32 = row.get(7)?;
    let timestamp: String = row.get(8)?;

    Ok(AuditEntry {
        id,
        user_id,
        agent_name,
        action,
        category,
        secret_name,
        access_tier,
        success: success != 0,
        timestamp,
    })
}

/// Prune old audit entries.
#[tracing::instrument(skip(db), fields(user_id, days_to_keep))]
pub async fn prune_audit(db: &Database, user_id: i64, days_to_keep: u32) -> Result<usize> {
    let cutoff = chrono::Utc::now()
        .checked_sub_signed(chrono::Duration::days(days_to_keep as i64))
        .unwrap_or_else(chrono::Utc::now)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();

    db.write(move |conn| {
        let affected = conn
            .execute(
                "DELETE FROM cred_audit WHERE user_id = ?1 AND timestamp < ?2",
                rusqlite::params![user_id, cutoff],
            )
            .map_err(rusqlite_to_eng_error)?;
        Ok(affected)
    })
    .await
    .map_err(|e| CredError::Database(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_db() -> Database {
        let db = Database::connect_memory().await.expect("db");
        db.write(move |conn| {
            conn.execute(
                "CREATE TABLE IF NOT EXISTS cred_audit (
                    id INTEGER PRIMARY KEY,
                    user_id INTEGER NOT NULL,
                    agent_name TEXT,
                    action TEXT NOT NULL,
                    category TEXT NOT NULL,
                    secret_name TEXT NOT NULL,
                    access_tier TEXT,
                    success INTEGER NOT NULL,
                    timestamp TEXT NOT NULL
                )",
                [],
            )
            .map_err(rusqlite_to_eng_error)?;
            Ok(())
        })
        .await
        .expect("create table");
        db
    }

    #[tokio::test]
    async fn log_and_query_audit() {
        let db = setup_db().await;

        log_audit(
            &db,
            1,
            Some("test-agent"),
            AuditAction::Get,
            "aws",
            "api-key",
            Some(AccessTier::Substitution),
            true,
        )
        .await
        .expect("log 1");

        log_audit(
            &db,
            1,
            None,
            AuditAction::Set,
            "gcp",
            "service-account",
            None,
            true,
        )
        .await
        .expect("log 2");

        let all = query_audit(&db, 1, 10, None, None).await.expect("query");
        assert_eq!(all.len(), 2);

        let aws_only = query_audit(&db, 1, 10, Some("aws"), None)
            .await
            .expect("query aws");
        assert_eq!(aws_only.len(), 1);
        assert_eq!(aws_only[0].category, "aws");
        assert_eq!(aws_only[0].agent_name, Some("test-agent".into()));
    }

    #[tokio::test]
    async fn get_secret_specific_audit() {
        let db = setup_db().await;

        log_audit(&db, 1, None, AuditAction::Get, "svc", "key1", None, true)
            .await
            .expect("log 1");
        log_audit(&db, 1, None, AuditAction::Get, "svc", "key2", None, true)
            .await
            .expect("log 2");
        log_audit(&db, 1, None, AuditAction::Get, "svc", "key1", None, true)
            .await
            .expect("log 3");

        let entries = get_secret_audit(&db, 1, "svc", "key1", 10)
            .await
            .expect("query");
        assert_eq!(entries.len(), 2);
    }
}
