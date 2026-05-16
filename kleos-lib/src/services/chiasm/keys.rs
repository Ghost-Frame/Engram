//! Chiasm per-agent bearer keys -- create, list, revoke, and verify against
//! the `chiasm_agent_keys` table. Mirrors the standalone chiasm admin keys
//! surface. The raw key is shown exactly once at creation; only its SHA-256
//! hash is persisted.

use crate::db::Database;
use crate::{EngError, Result};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Public view of a stored agent key. The hash is never exposed and the
/// raw key is only available from [`CreatedKey`] returned by `create_key`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentKey {
    pub id: i64,
    pub agent: String,
    pub key_prefix: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked: bool,
}

/// Returned exactly once on creation. Callers must capture `key` immediately
/// because the server only persists the hash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatedKey {
    pub id: i64,
    pub agent: String,
    pub key: String,
    pub prefix: String,
    pub created_at: String,
    pub warning: String,
}

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

/// Hash a bearer key into the lookup form used by the table.
pub fn hash_key(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(out, "{:02x}", b);
    }
    out
}

/// Generate a fresh raw key of the form `mc_<64 hex>` matching the
/// standalone chiasm format. Uses the OS CSPRNG.
fn mint_raw_key() -> String {
    let mut buf = [0u8; 32];
    rand::rng().fill_bytes(&mut buf);
    let mut hex = String::with_capacity(3 + buf.len() * 2);
    hex.push_str("mc_");
    for b in buf {
        use std::fmt::Write;
        let _ = write!(hex, "{:02x}", b);
    }
    hex
}

/// Create a new bearer key for `agent`. Returns the freshly minted raw key
/// alongside its record. The raw key is the only opportunity for the caller
/// to capture it; subsequent lookups can only confirm hashes.
#[tracing::instrument(skip(db), fields(agent = %agent))]
pub async fn create_key(db: &Database, agent: &str) -> Result<CreatedKey> {
    let agent = agent.trim();
    if agent.is_empty() {
        return Err(EngError::InvalidInput("agent must not be empty".into()));
    }
    let raw = mint_raw_key();
    let hash = hash_key(&raw);
    let prefix: String = raw.chars().take(11).collect();
    let agent_owned = agent.to_string();
    let hash_for_insert = hash.clone();
    let prefix_for_insert = prefix.clone();

    let (id, created_at): (i64, String) = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO chiasm_agent_keys (agent, key_hash, key_prefix) \
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![agent_owned, hash_for_insert, prefix_for_insert],
            )
            .map_err(rusqlite_to_eng_error)?;
            let id = conn.last_insert_rowid();
            let created_at: String = conn
                .query_row(
                    "SELECT created_at FROM chiasm_agent_keys WHERE id = ?1",
                    rusqlite::params![id],
                    |row| row.get(0),
                )
                .map_err(rusqlite_to_eng_error)?;
            Ok((id, created_at))
        })
        .await?;

    Ok(CreatedKey {
        id,
        agent: agent.to_string(),
        key: raw,
        prefix,
        created_at,
        warning: "Store this key now. It cannot be retrieved again.".to_string(),
    })
}

/// List non-secret metadata for every key (revoked or active).
#[tracing::instrument(skip(db))]
pub async fn list_keys(db: &Database) -> Result<Vec<AgentKey>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, agent, key_prefix, created_at, last_used_at, revoked \
                 FROM chiasm_agent_keys ORDER BY id ASC",
            )
            .map_err(rusqlite_to_eng_error)?;
        let rows = stmt
            .query_map([], |row| {
                Ok(AgentKey {
                    id: row.get(0)?,
                    agent: row.get(1)?,
                    key_prefix: row.get(2)?,
                    created_at: row.get(3)?,
                    last_used_at: row.get(4)?,
                    revoked: row.get::<_, i64>(5)? != 0,
                })
            })
            .map_err(rusqlite_to_eng_error)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(rusqlite_to_eng_error)?);
        }
        Ok(out)
    })
    .await
}

/// Mark a key revoked. Returns true when a row was updated.
#[tracing::instrument(skip(db), fields(key_id = id))]
pub async fn revoke_key(db: &Database, id: i64) -> Result<bool> {
    let changed = db
        .write(move |conn| {
            conn.execute(
                "UPDATE chiasm_agent_keys SET revoked = 1 WHERE id = ?1 AND revoked = 0",
                rusqlite::params![id],
            )
            .map_err(rusqlite_to_eng_error)
        })
        .await?;
    Ok(changed > 0)
}

/// Look up the active key whose hash matches the given bearer token. Returns
/// `Ok(None)` for unknown or revoked tokens. On success, updates the
/// `last_used_at` timestamp so admins can audit key activity.
#[tracing::instrument(skip(db, token))]
pub async fn verify_bearer(db: &Database, token: &str) -> Result<Option<AgentKey>> {
    let hash = hash_key(token);
    let lookup_hash = hash.clone();
    let found: Option<AgentKey> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, agent, key_prefix, created_at, last_used_at, revoked \
                     FROM chiasm_agent_keys \
                     WHERE key_hash = ?1 AND revoked = 0",
                )
                .map_err(rusqlite_to_eng_error)?;
            let mut rows = stmt
                .query(rusqlite::params![lookup_hash])
                .map_err(rusqlite_to_eng_error)?;
            if let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                Ok(Some(AgentKey {
                    id: row.get(0).map_err(rusqlite_to_eng_error)?,
                    agent: row.get(1).map_err(rusqlite_to_eng_error)?,
                    key_prefix: row.get(2).map_err(rusqlite_to_eng_error)?,
                    created_at: row.get(3).map_err(rusqlite_to_eng_error)?,
                    last_used_at: row.get(4).map_err(rusqlite_to_eng_error)?,
                    revoked: row.get::<_, i64>(5).map_err(rusqlite_to_eng_error)? != 0,
                }))
            } else {
                Ok(None)
            }
        })
        .await?;

    if let Some(ref key) = found {
        let id = key.id;
        let _ = db
            .write(move |conn| {
                conn.execute(
                    "UPDATE chiasm_agent_keys SET last_used_at = datetime('now') WHERE id = ?1",
                    rusqlite::params![id],
                )
                .map_err(rusqlite_to_eng_error)
            })
            .await;
    }

    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fresh_db() -> Database {
        // connect_memory bootstraps the full schema (including
        // chiasm_agent_keys), so no manual CREATE TABLE is needed here.
        Database::connect_memory().await.expect("db")
    }

    #[tokio::test]
    async fn hash_is_stable_and_hex() {
        let h = hash_key("mc_test");
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(h, hash_key("mc_test"));
    }

    #[tokio::test]
    async fn create_list_verify_revoke_roundtrip() {
        let db = fresh_db().await;
        let created = create_key(&db, "claude").await.expect("create");
        assert!(created.key.starts_with("mc_"));
        assert_eq!(created.key.len(), 3 + 64);
        assert_eq!(created.prefix.len(), 11);

        let list = list_keys(&db).await.expect("list");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].agent, "claude");
        assert!(!list[0].revoked);

        let found = verify_bearer(&db, &created.key).await.expect("verify");
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, created.id);

        let bogus = verify_bearer(&db, "mc_not_a_real_key")
            .await
            .expect("verify");
        assert!(bogus.is_none());

        let revoked = revoke_key(&db, created.id).await.expect("revoke");
        assert!(revoked);
        let after = verify_bearer(&db, &created.key).await.expect("verify");
        assert!(after.is_none());
    }

    #[tokio::test]
    async fn create_rejects_empty_agent() {
        let db = fresh_db().await;
        let err = create_key(&db, "   ").await.unwrap_err();
        assert!(matches!(err, EngError::InvalidInput(_)));
    }
}
