//! Scratchpad -- session-based key-value store for agents with TTL.
//!
//! Ports: scratch/db.ts, scratch/types.ts, scratch/routes.ts (logic)

use serde::{Deserialize, Serialize};
use crate::db::Database;
use crate::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScratchEntry {
    pub session: String,
    pub agent: String,
    pub model: String,
    pub key: String,
    pub value: String,
    pub created_at: String,
    pub updated_at: String,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScratchPutBody {
    pub session: Option<String>,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub entries: Option<Vec<ScratchKV>>,
    pub ttl: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScratchKV {
    pub key: String,
    pub value: Option<String>,
}

#[allow(clippy::too_many_arguments)]
pub async fn upsert_entry(db: &Database, user_id: i64, session: &str, agent: &str, model: &str, key: &str, value: &str, ttl_minutes: i64) -> Result<()> {
    let ttl_str = ttl_minutes.to_string();
    db.conn.execute(
        "INSERT INTO scratchpad (user_id, session, agent, model, entry_key, value, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now', '+' || ?7 || ' minutes')) ON CONFLICT(user_id, session, entry_key) DO UPDATE SET agent = excluded.agent, model = excluded.model, value = excluded.value, updated_at = datetime('now'), expires_at = datetime('now', '+' || ?8 || ' minutes')",
        libsql::params![user_id, session.to_string(), agent.to_string(), model.to_string(), key.to_string(), value.to_string(), ttl_str.clone(), ttl_str],
    ).await?;
    Ok(())
}

pub async fn list_entries(db: &Database, user_id: i64, agent: Option<&str>, model: Option<&str>, session: Option<&str>) -> Result<Vec<ScratchEntry>> {
    let mut rows = db.conn.query(
        "SELECT session, agent, model, entry_key, value, created_at, updated_at, expires_at FROM scratchpad WHERE user_id = ?1 AND expires_at > datetime('now') AND (?2 IS NULL OR agent = ?3) AND (?4 IS NULL OR model = ?5) AND (?6 IS NULL OR session = ?7) ORDER BY updated_at DESC, agent, session, entry_key",
        libsql::params![user_id, agent.map(|s| s.to_string()), agent.map(|s| s.to_string()), model.map(|s| s.to_string()), model.map(|s| s.to_string()), session.map(|s| s.to_string()), session.map(|s| s.to_string())],
    ).await?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().await? { result.push(row_to_entry(&row)?); }
    Ok(result)
}

pub async fn get_session_entries(db: &Database, user_id: i64, session: &str) -> Result<Vec<ScratchEntry>> {
    let mut rows = db.conn.query(
        "SELECT session, agent, model, entry_key, value, created_at, updated_at, expires_at FROM scratchpad WHERE user_id = ?1 AND session = ?2 ORDER BY created_at ASC",
        libsql::params![user_id, session.to_string()],
    ).await?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().await? { result.push(row_to_entry(&row)?); }
    Ok(result)
}

pub async fn delete_session(db: &Database, user_id: i64, session: &str) -> Result<()> {
    db.conn.execute(
        "DELETE FROM scratchpad WHERE user_id = ?1 AND session = ?2",
        libsql::params![user_id, session.to_string()],
    ).await?;
    Ok(())
}

pub async fn delete_session_key(db: &Database, user_id: i64, session: &str, key: &str) -> Result<()> {
    db.conn.execute(
        "DELETE FROM scratchpad WHERE user_id = ?1 AND session = ?2 AND entry_key = ?3",
        libsql::params![user_id, session.to_string(), key.to_string()],
    ).await?;
    Ok(())
}

pub async fn purge_expired(db: &Database) -> Result<i64> {
    let changes = db.conn.execute("DELETE FROM scratchpad WHERE expires_at <= datetime('now')", libsql::params![]).await?;
    Ok(changes as i64)
}

/// Promote session entries to permanent memories.
/// Returns list of created memory IDs.
pub async fn promote_entries(db: &Database, user_id: i64, session: &str, keys: Option<&[String]>, combine: bool, category: &str) -> Result<Vec<i64>> {
    let entries = get_session_entries(db, user_id, session).await?;
    if entries.is_empty() { return Err(crate::EngError::NotFound("No entries found for session".into())); }

    let filtered: Vec<&ScratchEntry> = if let Some(ks) = keys {
        entries.iter().filter(|e| ks.iter().any(|k| k == &e.key)).collect()
    } else {
        entries.iter().collect()
    };
    if filtered.is_empty() { return Err(crate::EngError::NotFound("No matching entries for specified keys".into())); }

    let mut promoted = Vec::new();
    if combine {
        let lines: Vec<String> = filtered.iter().map(|r| format!("[{}] {}: {}", r.agent, r.key, r.value)).collect();
        let content = format!("Session {} ({}): {}", &session[..session.len().min(8)], filtered[0].agent, lines.join("; "));
        let mut rows = db.conn.query(
            "INSERT INTO memories (content, category, source, importance, source_count, is_latest, user_id) VALUES (?1, ?2, ?3, 5, 1, 1, ?4) RETURNING id",
            libsql::params![content, category.to_string(), filtered[0].agent.clone(), user_id],
        ).await?;
        if let Some(row) = rows.next().await? {
            promoted.push(row.get::<i64>(0).map_err(|e| crate::EngError::Internal(e.to_string()))?);
        }
    } else {
        for r in &filtered {
            let content = format!("{}: {}", r.key, r.value);
            let mut rows = db.conn.query(
                "INSERT INTO memories (content, category, source, importance, source_count, is_latest, user_id) VALUES (?1, ?2, ?3, 5, 1, 1, ?4) RETURNING id",
                libsql::params![content, category.to_string(), r.agent.clone(), user_id],
            ).await?;
            if let Some(row) = rows.next().await? {
                promoted.push(row.get::<i64>(0).map_err(|e| crate::EngError::Internal(e.to_string()))?);
            }
        }
    }
    Ok(promoted)
}

fn row_to_entry(row: &libsql::Row) -> Result<ScratchEntry> {
    Ok(ScratchEntry {
        session: row.get(0).map_err(|e| crate::EngError::Internal(e.to_string()))?,
        agent: row.get(1).map_err(|e| crate::EngError::Internal(e.to_string()))?,
        model: row.get(2).map_err(|e| crate::EngError::Internal(e.to_string()))?,
        key: row.get(3).map_err(|e| crate::EngError::Internal(e.to_string()))?,
        value: row.get(4).map_err(|e| crate::EngError::Internal(e.to_string()))?,
        created_at: row.get(5).map_err(|e| crate::EngError::Internal(e.to_string()))?,
        updated_at: row.get(6).map_err(|e| crate::EngError::Internal(e.to_string()))?,
        expires_at: row.get(7).unwrap_or(None),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scratch_entry_serialize() {
        let entry = ScratchEntry {
            session: "sess1".into(), agent: "test".into(), model: "gpt".into(),
            key: "status".into(), value: "running".into(),
            created_at: "2024-01-01".into(), updated_at: "2024-01-01".into(),
            expires_at: Some("2024-01-02".into()),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("sess1"));
    }
}
