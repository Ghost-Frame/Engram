//! Scratchpad -- session-based key-value store for agents with TTL.
//!
//! Ports: scratch/db.ts, scratch/types.ts, scratch/routes.ts (logic)

use crate::db::Database;
use crate::{EngError, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

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
pub async fn upsert_entry(
    db: &Database,
    user_id: i64,
    session: &str,
    agent: &str,
    model: &str,
    key: &str,
    value: &str,
    ttl_minutes: i64,
) -> Result<()> {
    let ttl_str = ttl_minutes.to_string();
    let session = session.to_string();
    let agent = agent.to_string();
    let model = model.to_string();
    let key = key.to_string();
    let value = value.to_string();
    db.write(move |conn| {
        conn.execute(
            "INSERT INTO scratchpad (user_id, session, agent, model, entry_key, value, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now', '+' || ?7 || ' minutes')) ON CONFLICT(user_id, session, entry_key) DO UPDATE SET agent = excluded.agent, model = excluded.model, value = excluded.value, updated_at = datetime('now'), expires_at = datetime('now', '+' || ?8 || ' minutes')",
            params![user_id, session, agent, model, key, value, ttl_str.clone(), ttl_str],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

pub async fn list_entries(
    db: &Database,
    user_id: i64,
    agent: Option<&str>,
    model: Option<&str>,
    session: Option<&str>,
) -> Result<Vec<ScratchEntry>> {
    let agent = agent.map(|s| s.to_string());
    let model = model.map(|s| s.to_string());
    let session = session.map(|s| s.to_string());
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT session, agent, model, entry_key, value, created_at, updated_at, expires_at FROM scratchpad WHERE user_id = ?1 AND expires_at > datetime('now') AND (?2 IS NULL OR agent = ?3) AND (?4 IS NULL OR model = ?5) AND (?6 IS NULL OR session = ?7) ORDER BY updated_at DESC, agent, session, entry_key",
            )
            .map_err(rusqlite_to_eng_error)?;
        let rows = stmt
            .query_map(
                params![user_id, agent.clone(), agent, model.clone(), model, session.clone(), session],
                row_to_entry_rusqlite,
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(rusqlite_to_eng_error)?);
        }
        Ok(result)
    })
    .await
}

pub async fn get_session_entries(
    db: &Database,
    user_id: i64,
    session: &str,
) -> Result<Vec<ScratchEntry>> {
    let session = session.to_string();
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT session, agent, model, entry_key, value, created_at, updated_at, expires_at FROM scratchpad WHERE user_id = ?1 AND session = ?2 ORDER BY created_at ASC",
            )
            .map_err(rusqlite_to_eng_error)?;
        let rows = stmt
            .query_map(params![user_id, session], row_to_entry_rusqlite)
            .map_err(rusqlite_to_eng_error)?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(rusqlite_to_eng_error)?);
        }
        Ok(result)
    })
    .await
}

pub async fn delete_session(db: &Database, user_id: i64, session: &str) -> Result<()> {
    let session = session.to_string();
    db.write(move |conn| {
        conn.execute(
            "DELETE FROM scratchpad WHERE user_id = ?1 AND session = ?2",
            params![user_id, session],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

pub async fn delete_session_key(
    db: &Database,
    user_id: i64,
    session: &str,
    key: &str,
) -> Result<()> {
    let session = session.to_string();
    let key = key.to_string();
    db.write(move |conn| {
        conn.execute(
            "DELETE FROM scratchpad WHERE user_id = ?1 AND session = ?2 AND entry_key = ?3",
            params![user_id, session, key],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

pub async fn purge_expired(db: &Database) -> Result<i64> {
    db.write(move |conn| {
        let changes = conn
            .execute(
                "DELETE FROM scratchpad WHERE expires_at <= datetime('now')",
                params![],
            )
            .map_err(rusqlite_to_eng_error)?;
        Ok(changes as i64)
    })
    .await
}

/// Promote session entries to permanent memories.
/// Returns list of created memory IDs.
pub async fn promote_entries(
    db: &Database,
    user_id: i64,
    session: &str,
    keys: Option<&[String]>,
    combine: bool,
    category: &str,
) -> Result<Vec<i64>> {
    let entries = get_session_entries(db, user_id, session).await?;
    if entries.is_empty() {
        return Err(crate::EngError::NotFound(
            "No entries found for session".into(),
        ));
    }

    let filtered: Vec<ScratchEntry> = if let Some(ks) = keys {
        entries
            .into_iter()
            .filter(|e| ks.iter().any(|k| k == &e.key))
            .collect()
    } else {
        entries
    };
    if filtered.is_empty() {
        return Err(crate::EngError::NotFound(
            "No matching entries for specified keys".into(),
        ));
    }

    let category = category.to_string();
    let session_prefix = session[..session.len().min(8)].to_string();

    db.write(move |conn| {
        let mut promoted = Vec::new();
        if combine {
            let lines: Vec<String> = filtered
                .iter()
                .map(|r| format!("[{}] {}: {}", r.agent, r.key, r.value))
                .collect();
            let content = format!(
                "Session {} ({}): {}",
                session_prefix,
                filtered[0].agent,
                lines.join("; ")
            );
            let source = filtered[0].agent.clone();
            conn.execute(
                "INSERT INTO memories (content, category, source, importance, source_count, is_latest, user_id) VALUES (?1, ?2, ?3, 5, 1, 1, ?4)",
                params![content, category, source, user_id],
            )
            .map_err(rusqlite_to_eng_error)?;
            promoted.push(conn.last_insert_rowid());
        } else {
            for r in &filtered {
                let content = format!("{}: {}", r.key, r.value);
                let source = r.agent.clone();
                conn.execute(
                    "INSERT INTO memories (content, category, source, importance, source_count, is_latest, user_id) VALUES (?1, ?2, ?3, 5, 1, 1, ?4)",
                    params![content, category, source, user_id],
                )
                .map_err(rusqlite_to_eng_error)?;
                promoted.push(conn.last_insert_rowid());
            }
        }
        Ok(promoted)
    })
    .await
}

fn row_to_entry_rusqlite(row: &rusqlite::Row<'_>) -> rusqlite::Result<ScratchEntry> {
    Ok(ScratchEntry {
        session: row.get(0)?,
        agent: row.get(1)?,
        model: row.get(2)?,
        key: row.get(3)?,
        value: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
        expires_at: row.get(7)?,
    })
}

fn row_to_entry(row: &rusqlite::Row<'_>) -> Result<ScratchEntry> {
    row_to_entry_rusqlite(row).map_err(rusqlite_to_eng_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scratch_entry_serialize() {
        let entry = ScratchEntry {
            session: "sess1".into(),
            agent: "test".into(),
            model: "gpt".into(),
            key: "status".into(),
            value: "running".into(),
            created_at: "2024-01-01".into(),
            updated_at: "2024-01-01".into(),
            expires_at: Some("2024-01-02".into()),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("sess1"));
    }
}
