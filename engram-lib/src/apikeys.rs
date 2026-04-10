use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::db::Database;
use crate::Result;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKey {
    pub id: i64,
    pub user_id: i64,
    pub key_prefix: String,
    pub scopes: String,
    pub rate_limit: i64,
    pub agent_id: Option<i64>,
    pub expires_at: Option<String>,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// SHA-256 hash of a raw key, returned as lowercase hex.
fn hash_key(raw: &str) -> String {
    let mut h = Sha256::new();
    h.update(raw.as_bytes());
    format!("{:x}", h.finalize())
}

/// Generate a new API key.
/// Returns (full_key, prefix, hash).
fn generate_key() -> (String, String, String) {
    let part_a = Uuid::new_v4().simple().to_string(); // 32 hex chars
    let part_b = Uuid::new_v4().simple().to_string(); // 32 hex chars
    let hex: String = format!("{}{}", part_a, part_b).chars().take(32).collect();
    let full_key = format!("engram_{}", hex);
    let prefix = hex[..8].to_string();
    let h = hash_key(&full_key);
    (full_key, prefix, h)
}

// ---------------------------------------------------------------------------
// Row mapping
// ---------------------------------------------------------------------------

fn row_to_key(row: &libsql::Row) -> Result<ApiKey> {
    Ok(ApiKey {
        id: row
            .get::<i64>(0)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?,
        user_id: row
            .get::<i64>(1)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?,
        key_prefix: row
            .get::<String>(2)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?,
        scopes: row
            .get::<String>(3)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?,
        rate_limit: row
            .get::<i64>(4)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?,
        agent_id: row
            .get::<Option<i64>>(5)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?,
        expires_at: row
            .get::<Option<String>>(6)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?,
        created_at: row
            .get::<String>(7)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?,
    })
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new API key for a user.
/// Returns the stored ApiKey record and the raw (plaintext) key, which is
/// shown once and never stored.
pub async fn create_api_key(
    db: &Database,
    user_id: i64,
    scopes: &str,
    rate_limit: i64,
) -> Result<(ApiKey, String)> {
    let (full_key, prefix, key_hash) = generate_key();

    db.conn
        .execute(
            "INSERT INTO api_keys (user_id, key_prefix, key_hash, scopes, rate_limit)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            libsql::params![
                user_id,
                prefix.clone(),
                key_hash.clone(),
                scopes,
                rate_limit
            ],
        )
        .await?;

    // Re-fetch the row using prefix + hash to get id and server-generated created_at.
    let mut rows = db
        .conn
        .query(
            "SELECT id, user_id, key_prefix, scopes, rate_limit, agent_id, expires_at, created_at
             FROM api_keys
             WHERE key_prefix = ?1 AND key_hash = ?2
             LIMIT 1",
            libsql::params![prefix, key_hash],
        )
        .await?;

    let row = rows
        .next()
        .await?
        .ok_or_else(|| crate::EngError::Internal("failed to fetch newly created api key".into()))?;

    let api_key = row_to_key(&row)?;
    Ok((api_key, full_key))
}

/// Validate an API key by prefix and hash.
/// Returns None if no matching active key is found.
pub async fn validate_api_key(
    db: &Database,
    key_prefix: &str,
    key_hash: &str,
) -> Result<Option<ApiKey>> {
    let mut rows = db
        .conn
        .query(
            "SELECT id, user_id, key_prefix, scopes, rate_limit, agent_id, expires_at, created_at
             FROM api_keys
             WHERE key_prefix = ?1 AND key_hash = ?2
             LIMIT 1",
            libsql::params![key_prefix, key_hash],
        )
        .await?;

    if let Some(row) = rows.next().await? {
        Ok(Some(row_to_key(&row)?))
    } else {
        Ok(None)
    }
}

/// List all API keys for a user (never exposes key_hash).
pub async fn list_api_keys(db: &Database, user_id: i64) -> Result<Vec<ApiKey>> {
    let mut rows = db
        .conn
        .query(
            "SELECT id, user_id, key_prefix, scopes, rate_limit, agent_id, expires_at, created_at
             FROM api_keys
             WHERE user_id = ?1
             ORDER BY created_at DESC",
            libsql::params![user_id],
        )
        .await?;

    let mut keys = Vec::new();
    while let Some(row) = rows.next().await? {
        keys.push(row_to_key(&row)?);
    }
    Ok(keys)
}

/// Delete an API key by id (no ownership check -- admin use only).
pub async fn delete_api_key(db: &Database, id: i64) -> Result<()> {
    db.conn
        .execute("DELETE FROM api_keys WHERE id = ?1", libsql::params![id])
        .await?;
    Ok(())
}

/// Delete an API key by id, but only if it belongs to the specified user.
/// Returns true if the key was deleted, false if not found or not owned.
pub async fn delete_api_key_for_user(db: &Database, id: i64, user_id: i64) -> Result<bool> {
    let rows_affected = db
        .conn
        .execute(
            "DELETE FROM api_keys WHERE id = ?1 AND user_id = ?2",
            libsql::params![id, user_id],
        )
        .await?;
    Ok(rows_affected > 0)
}
