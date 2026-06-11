//! Scratchpad: session-based key-value store for agents with TTL.
//!
//! Ports: scratch/db.ts, scratch/types.ts, scratch/routes.ts (logic)

use crate::db::Database;
use crate::Result;
use rusqlite::params;
use serde::{Deserialize, Serialize};

/// Represents one scratchpad row returned from the database.
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

/// Represents the payload accepted by the scratchpad put endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScratchPutBody {
    pub session: Option<String>,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub entries: Option<Vec<ScratchKV>>,
    pub ttl: Option<i64>,
}

/// Represents one key-value pair inside a scratchpad write request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScratchKV {
    pub key: String,
    pub value: Option<String>,
}

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip(db, session, agent, model, key, value))]
/// Inserts or updates one scratchpad entry with a TTL.
pub async fn upsert_entry(
    db: &Database,
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
            "INSERT INTO scratchpad (session, agent, model, entry_key, value, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, datetime('now', '+' || ?6 || ' minutes')) ON CONFLICT(session, agent, entry_key) DO UPDATE SET model = excluded.model, value = excluded.value, updated_at = datetime('now'), expires_at = datetime('now', '+' || ?7 || ' minutes')",
            params![session, agent, model, key, value, ttl_str.clone(), ttl_str],
        )
        ?;
        Ok(())
    })
    .await
}

#[tracing::instrument(skip(db, agent, model, session))]
/// Lists active scratchpad entries filtered by agent, model, and session.
pub async fn list_entries(
    db: &Database,
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
                "SELECT session, agent, model, entry_key, value, created_at, updated_at, expires_at FROM scratchpad WHERE expires_at > datetime('now') AND (?1 IS NULL OR agent = ?2) AND (?3 IS NULL OR model = ?4) AND (?5 IS NULL OR session = ?6) ORDER BY updated_at DESC, agent, session, entry_key",
            )
            ?;
        let rows = stmt
            .query_map(
                params![agent.clone(), agent, model.clone(), model, session.clone(), session],
                row_to_entry_rusqlite,
            )
            ?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    })
    .await
}

#[tracing::instrument(skip(db, session))]
/// Loads every entry for one scratchpad session in creation order.
pub async fn get_session_entries(db: &Database, session: &str) -> Result<Vec<ScratchEntry>> {
    let session = session.to_string();
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT session, agent, model, entry_key, value, created_at, updated_at, expires_at FROM scratchpad WHERE session = ?1 ORDER BY created_at ASC",
            )
            ?;
        let rows = stmt
            .query_map(params![session], row_to_entry_rusqlite)
            ?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    })
    .await
}

#[tracing::instrument(skip(db, session))]
/// Deletes every scratchpad entry for one session.
pub async fn delete_session(db: &Database, session: &str) -> Result<()> {
    let session = session.to_string();
    db.write(move |conn| {
        conn.execute(
            "DELETE FROM scratchpad WHERE session = ?1",
            params![session],
        )?;
        Ok(())
    })
    .await
}

#[tracing::instrument(skip(db, session, key))]
/// Deletes one key from one scratchpad session.
pub async fn delete_session_key(db: &Database, session: &str, key: &str) -> Result<()> {
    let session = session.to_string();
    let key = key.to_string();
    db.write(move |conn| {
        conn.execute(
            "DELETE FROM scratchpad WHERE session = ?1 AND entry_key = ?2",
            params![session, key],
        )?;
        Ok(())
    })
    .await
}

#[tracing::instrument(skip(db))]
/// Removes expired scratchpad entries and returns the number deleted.
pub async fn purge_expired(db: &Database) -> Result<i64> {
    db.write(move |conn| {
        let changes = conn.execute(
            "DELETE FROM scratchpad WHERE expires_at <= datetime('now')",
            params![],
        )?;
        Ok(changes as i64)
    })
    .await
}

/// Promote session entries to permanent memories.
/// Returns list of created memory IDs.
#[tracing::instrument(skip(db, session, keys, category))]
/// Promotes selected session entries into permanent memories.
pub async fn promote_entries(
    db: &Database,
    user_id: i64,
    session: &str,
    keys: Option<&[String]>,
    combine: bool,
    category: &str,
) -> Result<Vec<i64>> {
    let entries = get_session_entries(db, session).await?;
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
    let session_prefix = session_short_prefix(session).to_string();

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
            ?;
            promoted.push(conn.last_insert_rowid());
        } else {
            for r in &filtered {
                let content = format!("{}: {}", r.key, r.value);
                let source = r.agent.clone();
                conn.execute(
                    "INSERT INTO memories (content, category, source, importance, source_count, is_latest, user_id) VALUES (?1, ?2, ?3, 5, 1, 1, ?4)",
                    params![content, category, source, user_id],
                )
                ?;
                promoted.push(conn.last_insert_rowid());
            }
        }
        Ok(promoted)
    })
    .await
}

/// Returns a short session prefix without cutting through UTF-8 bytes.
fn session_short_prefix(session: &str) -> &str {
    crate::validation::truncate_on_char_boundary(session, 8)
}

/// Maps one rusqlite row into a scratchpad entry.
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

/// Tests for scratchpad serialization and UTF-8-safe promotion previews.
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies scratchpad entries serialize with their session fields intact.
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

    /// Regression: multibyte session prefixes must not panic during promotion.
    #[test]
    fn session_short_prefix_handles_multibyte_boundaries() {
        assert_eq!(session_short_prefix("sess-💥-alpha"), "sess-");
    }

    /// Regression: promoted memories keep the caller user id in both promotion modes.
    #[tokio::test]
    async fn promote_entries_binds_user_id_for_inserted_memories() {
        let db = crate::db::Database::connect_memory()
            .await
            .expect("in-memory db");

        upsert_entry(
            &db,
            "combined-session",
            "agent-a",
            "model-a",
            "alpha",
            "one",
            30,
        )
        .await
        .expect("combined alpha entry");
        upsert_entry(
            &db,
            "combined-session",
            "agent-a",
            "model-a",
            "beta",
            "two",
            30,
        )
        .await
        .expect("combined beta entry");
        let combined_ids = promote_entries(&db, 2, "combined-session", None, true, "test")
            .await
            .expect("combined promotion");
        assert_eq!(combined_ids.len(), 1);
        let combined_id = combined_ids[0];
        let combined_owner = db
            .read(move |conn| {
                let owner = conn.query_row(
                    "SELECT user_id FROM memories WHERE id = ?1",
                    params![combined_id],
                    |row| row.get::<_, i64>(0),
                )?;
                Ok(owner)
            })
            .await
            .expect("combined memory owner");
        assert_eq!(combined_owner, 2);

        upsert_entry(
            &db,
            "individual-session",
            "agent-b",
            "model-b",
            "gamma",
            "three",
            30,
        )
        .await
        .expect("individual gamma entry");
        upsert_entry(
            &db,
            "individual-session",
            "agent-b",
            "model-b",
            "delta",
            "four",
            30,
        )
        .await
        .expect("individual delta entry");
        let individual_ids = promote_entries(&db, 2, "individual-session", None, false, "test")
            .await
            .expect("individual promotion");
        assert_eq!(individual_ids.len(), 2);
        let individual_owners = db
            .read(move |conn| {
                let mut owners = Vec::new();
                for id in individual_ids {
                    let owner = conn.query_row(
                        "SELECT user_id FROM memories WHERE id = ?1",
                        params![id],
                        |row| row.get::<_, i64>(0),
                    )?;
                    owners.push(owner);
                }
                Ok(owners)
            })
            .await
            .expect("individual memory owners");
        assert_eq!(individual_owners, vec![2, 2]);
    }
}
