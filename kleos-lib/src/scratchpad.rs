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
/// Inserts or updates one scratchpad entry with a TTL, scoped to `user_id`.
///
/// The ON CONFLICT target includes `user_id` so two tenants writing the same
/// session+agent+entry_key produce two distinct rows rather than clobbering
/// each other.
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
            "INSERT INTO scratchpad (user_id, session, agent, model, entry_key, value, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now', '+' || ?7 || ' minutes')) ON CONFLICT(user_id, session, agent, entry_key) DO UPDATE SET model = excluded.model, value = excluded.value, updated_at = datetime('now'), expires_at = datetime('now', '+' || ?8 || ' minutes')",
            params![user_id, session, agent, model, key, value, ttl_str.clone(), ttl_str],
        )
        ?;
        Ok(())
    })
    .await
}

#[tracing::instrument(skip(db, agent, model, session))]
/// Lists active scratchpad entries for `user_id`, filtered by agent, model, and
/// session. The `user_id` predicate is always applied so a caller can never see
/// another tenant's entries.
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
            ?;
        let rows = stmt
            .query_map(
                params![user_id, agent.clone(), agent, model.clone(), model, session.clone(), session],
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
/// Loads every entry for one scratchpad session owned by `user_id`, in creation
/// order.
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
            ?;
        let rows = stmt
            .query_map(params![user_id, session], row_to_entry_rusqlite)
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
/// Deletes every scratchpad entry for one session owned by `user_id`. The
/// `user_id` predicate prevents one tenant deleting another tenant's session
/// when session ids collide in single-DB mode.
pub async fn delete_session(db: &Database, user_id: i64, session: &str) -> Result<()> {
    let session = session.to_string();
    db.write(move |conn| {
        conn.execute(
            "DELETE FROM scratchpad WHERE user_id = ?1 AND session = ?2",
            params![user_id, session],
        )?;
        Ok(())
    })
    .await
}

#[tracing::instrument(skip(db, session, key))]
/// Deletes one key from one scratchpad session owned by `user_id`.
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
        )?;
        Ok(())
    })
    .await
}

#[tracing::instrument(skip(db))]
/// Looks up a non-expired scratchpad entry by namespace (agent column) and
/// entry_key, ignoring session boundaries since the session is embedded in the
/// key by the `ke` edit-gate (`format!("{session_id}:{path}")`).
///
/// Returns `Some(value)` when found, `None` when absent or expired.
#[tracing::instrument(skip(db, namespace, key))]
pub async fn get_by_namespace_key(
    db: &Database,
    user_id: i64,
    namespace: &str,
    key: &str,
) -> Result<Option<String>> {
    let namespace = namespace.to_string();
    let key = key.to_string();
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT value FROM scratchpad WHERE user_id = ?1 AND agent = ?2 AND entry_key = ?3 AND expires_at > datetime('now') LIMIT 1",
        )?;
        let mut rows = stmt.query(rusqlite::params![user_id, namespace, key])?;
        if let Some(row) = rows.next()? {
            let value: String = row.get(0)?;
            Ok(Some(value))
        } else {
            Ok(None)
        }
    })
    .await
}

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
            2,
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
            2,
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
            2,
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
            2,
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

    /// Tenant isolation: scratchpad reads and deletes are scoped to user_id, so
    /// one tenant can never see or remove another tenant's entries even when
    /// they share a session+agent+key. This is the cross-tenant fix restored by
    /// tenant migration v75 / monolith migration 92.
    #[tokio::test]
    async fn scratchpad_is_user_scoped() {
        let db = crate::db::Database::connect_memory()
            .await
            .expect("in-memory db");

        // User 1 and user 2 both write the SAME session+agent+entry_key.
        upsert_entry(&db, 1, "shared", "agent", "model", "k", "u1-secret", 30)
            .await
            .expect("u1 write");
        upsert_entry(&db, 2, "shared", "agent", "model", "k", "u2-secret", 30)
            .await
            .expect("u2 write");

        // The user-scoped UNIQUE keeps them distinct: no clobber.
        let u1 = list_entries(&db, 1, None, None, None)
            .await
            .expect("u1 list");
        let u2 = list_entries(&db, 2, None, None, None)
            .await
            .expect("u2 list");
        assert_eq!(u1.len(), 1);
        assert_eq!(u2.len(), 1);
        assert_eq!(u1[0].value, "u1-secret");
        assert_eq!(
            u2[0].value, "u2-secret",
            "user 2's write must not clobber user 1's row"
        );

        // list_entries never returns the other tenant's value.
        assert!(
            u2.iter().all(|e| e.value != "u1-secret"),
            "list_entries leaked across tenants"
        );

        // get_session_entries is scoped too.
        let u2_session = get_session_entries(&db, 2, "shared")
            .await
            .expect("u2 session");
        assert!(
            u2_session.iter().all(|e| e.value != "u1-secret"),
            "get_session_entries leaked across tenants"
        );

        // delete_session is scoped: user 2 deleting the shared session must NOT
        // remove user 1's row.
        delete_session(&db, 2, "shared").await.expect("u2 delete");
        let u1_after = list_entries(&db, 1, None, None, None)
            .await
            .expect("u1 list after");
        assert_eq!(u1_after.len(), 1, "user 2's delete removed user 1's entry");
        let u2_after = list_entries(&db, 2, None, None, None)
            .await
            .expect("u2 list after");
        assert_eq!(u2_after.len(), 0, "user 2's own entry was not deleted");
    }
}
