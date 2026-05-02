//! Agent key management with permission scoping and revocation.

use kleos_lib::db::Database;
use kleos_lib::EngError;
use rand::Rng;
use rusqlite::params;

use crate::crypto::hash_key;
use crate::{CredError, Result};

/// An agent key for service authentication.
#[derive(Debug, Clone)]
pub struct AgentKey {
    pub id: i64,
    pub user_id: i64,
    pub key_hash: String,
    pub name: String,
    pub permissions: AgentKeyPermissions,
    pub created_at: String,
    pub revoked_at: Option<String>,
}

impl AgentKey {
    /// Check if this key is currently valid (not revoked).
    pub fn is_valid(&self) -> bool {
        self.revoked_at.is_none()
    }

    /// Check if this key has permission to access a category.
    pub fn can_access(&self, category: &str) -> bool {
        if !self.is_valid() {
            return false;
        }
        self.permissions.allows_category(category)
    }

    /// Check if this key can use raw access tier.
    pub fn can_access_raw(&self) -> bool {
        self.is_valid() && self.permissions.allow_raw
    }
}

/// Permissions for an agent key.
#[derive(Debug, Clone, Default)]
pub struct AgentKeyPermissions {
    /// Allowed category patterns (empty = all categories).
    pub categories: Vec<String>,
    /// Whether raw access tier is allowed.
    pub allow_raw: bool,
}

impl AgentKeyPermissions {
    /// Check if a category is allowed.
    pub fn allows_category(&self, category: &str) -> bool {
        if self.categories.is_empty() {
            return true;
        }
        self.categories.iter().any(|pat| {
            if pat.ends_with('*') {
                category.starts_with(&pat[..pat.len() - 1])
            } else {
                category == pat
            }
        })
    }

    /// Serialize to JSON for storage.
    pub fn to_json(&self) -> String {
        serde_json::json!({
            "categories": self.categories,
            "allow_raw": self.allow_raw
        })
        .to_string()
    }

    /// Deserialize from JSON.
    pub fn from_json(json: &str) -> Self {
        let value: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
        let categories = value
            .get("categories")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let allow_raw = value
            .get("allow_raw")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        Self {
            categories,
            allow_raw,
        }
    }
}

/// Generate a new agent key.
///
/// Returns (raw_key_bytes, key_hash).
pub fn generate_agent_key() -> ([u8; 32], String) {
    let mut key = [0u8; 32];
    rand::rng().fill(&mut key);
    let hash = hash_key(&key);
    (key, hash)
}

/// Format a raw key as a displayable string (hex).
pub fn format_agent_key(key: &[u8]) -> String {
    hex::encode(key)
}

/// Parse a hex-encoded agent key.
pub fn parse_agent_key(encoded: &str) -> Result<Vec<u8>> {
    hex::decode(encoded).map_err(|e| CredError::InvalidInput(format!("invalid key format: {}", e)))
}

/// Create a new agent key in the database.
#[tracing::instrument(skip(db, permissions), fields(user_id, name = %name))]
pub async fn create_agent_key(
    db: &Database,
    user_id: i64,
    name: &str,
    permissions: &AgentKeyPermissions,
) -> Result<(String, AgentKey)> {
    let (raw_key, key_hash) = generate_agent_key();
    let permissions_json = permissions.to_json();
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let name_owned = name.to_string();
    let permissions_clone = permissions.clone();
    let key_hash_ret = key_hash.clone();
    let now_ret = now.clone();

    let id = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO cred_agent_keys (user_id, key_hash, name, permissions, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![user_id, key_hash, name_owned, permissions_json, now],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            Ok(conn.last_insert_rowid())
        })
        .await
        .map_err(|e| CredError::Database(e.to_string()))?;

    let key = AgentKey {
        id,
        user_id,
        key_hash: key_hash_ret,
        name: name.to_string(),
        permissions: permissions_clone,
        created_at: now_ret,
        revoked_at: None,
    };

    Ok((format_agent_key(&raw_key), key))
}

/// Validate an agent key and return its info.
#[tracing::instrument(skip(db, raw_key))]
pub async fn validate_agent_key(db: &Database, raw_key: &[u8]) -> Result<AgentKey> {
    let key_hash = hash_key(raw_key);

    let key = db
        .read(move |conn| {
            let result = conn.query_row(
                "SELECT id, user_id, key_hash, name, permissions, created_at, revoked_at
                 FROM cred_agent_keys
                 WHERE key_hash = ?1",
                params![key_hash],
                |row| {
                    let id: i64 = row.get(0)?;
                    let user_id: i64 = row.get(1)?;
                    let key_hash: String = row.get(2)?;
                    let name: String = row.get(3)?;
                    let permissions_json: String = row.get(4)?;
                    let created_at: String = row.get(5)?;
                    let revoked_at: Option<String> = row.get(6)?;
                    Ok((
                        id,
                        user_id,
                        key_hash,
                        name,
                        permissions_json,
                        created_at,
                        revoked_at,
                    ))
                },
            );
            match result {
                Ok((id, user_id, key_hash, name, permissions_json, created_at, revoked_at)) => {
                    let permissions = AgentKeyPermissions::from_json(&permissions_json);
                    Ok(Some(AgentKey {
                        id,
                        user_id,
                        key_hash,
                        name,
                        permissions,
                        created_at,
                        revoked_at,
                    }))
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(EngError::DatabaseMessage(e.to_string())),
            }
        })
        .await
        .map_err(|e| CredError::Database(e.to_string()))?
        .ok_or_else(|| CredError::AuthFailed("invalid agent key".into()))?;

    if !key.is_valid() {
        return Err(CredError::KeyRevoked(key.name.clone()));
    }

    Ok(key)
}

/// List agent keys for a user.
#[tracing::instrument(skip(db), fields(user_id))]
pub async fn list_agent_keys(db: &Database, user_id: i64) -> Result<Vec<AgentKey>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, user_id, key_hash, name, permissions, created_at, revoked_at
                 FROM cred_agent_keys
                 WHERE user_id = ?1
                 ORDER BY created_at DESC",
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        let rows = stmt
            .query_map(params![user_id], |row| {
                let id: i64 = row.get(0)?;
                let user_id: i64 = row.get(1)?;
                let key_hash: String = row.get(2)?;
                let name: String = row.get(3)?;
                let permissions_json: String = row.get(4)?;
                let created_at: String = row.get(5)?;
                let revoked_at: Option<String> = row.get(6)?;
                Ok((
                    id,
                    user_id,
                    key_hash,
                    name,
                    permissions_json,
                    created_at,
                    revoked_at,
                ))
            })
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        let mut keys = Vec::new();
        for row_result in rows {
            let (id, user_id, key_hash, name, permissions_json, created_at, revoked_at) =
                row_result.map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            let permissions = AgentKeyPermissions::from_json(&permissions_json);
            keys.push(AgentKey {
                id,
                user_id,
                key_hash,
                name,
                permissions,
                created_at,
                revoked_at,
            });
        }

        Ok(keys)
    })
    .await
    .map_err(|e| CredError::Database(e.to_string()))
}

/// Revoke an agent key.
#[tracing::instrument(skip(db), fields(user_id, name = %name))]
pub async fn revoke_agent_key(db: &Database, user_id: i64, name: &str) -> Result<()> {
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let name_owned = name.to_string();

    let affected = db
        .write(move |conn| {
            conn.execute(
                "UPDATE cred_agent_keys SET revoked_at = ?1 WHERE user_id = ?2 AND name = ?3 AND revoked_at IS NULL",
                params![now, user_id, name_owned],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))
        })
        .await
        .map_err(|e| CredError::Database(e.to_string()))?;

    if affected == 0 {
        return Err(CredError::NotFound(format!("agent key: {}", name)));
    }

    Ok(())
}

/// Delete an agent key entirely (for cleanup).
#[tracing::instrument(skip(db), fields(user_id, name = %name))]
pub async fn delete_agent_key(db: &Database, user_id: i64, name: &str) -> Result<()> {
    let name_owned = name.to_string();

    let affected = db
        .write(move |conn| {
            conn.execute(
                "DELETE FROM cred_agent_keys WHERE user_id = ?1 AND name = ?2",
                params![user_id, name_owned],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))
        })
        .await
        .map_err(|e| CredError::Database(e.to_string()))?;

    if affected == 0 {
        return Err(CredError::NotFound(format!("agent key: {}", name)));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_permissions() -> AgentKeyPermissions {
        AgentKeyPermissions {
            categories: vec!["aws".into(), "gcp*".into()],
            allow_raw: true,
        }
    }

    #[test]
    fn permissions_allows_exact_match() {
        let perms = setup_permissions();
        assert!(perms.allows_category("aws"));
        assert!(!perms.allows_category("azure"));
    }

    #[test]
    fn permissions_allows_wildcard() {
        let perms = setup_permissions();
        assert!(perms.allows_category("gcp"));
        assert!(perms.allows_category("gcp-prod"));
        assert!(perms.allows_category("gcp/project"));
    }

    #[test]
    fn permissions_empty_allows_all() {
        let perms = AgentKeyPermissions::default();
        assert!(perms.allows_category("anything"));
        assert!(perms.allows_category("really/anything"));
    }

    #[test]
    fn permissions_json_roundtrip() {
        let perms = setup_permissions();
        let json = perms.to_json();
        let restored = AgentKeyPermissions::from_json(&json);
        assert_eq!(perms.categories, restored.categories);
        assert_eq!(perms.allow_raw, restored.allow_raw);
    }

    #[test]
    fn generate_key_produces_valid_hash() {
        let (key, hash) = generate_agent_key();
        assert_eq!(key.len(), 32);
        assert_eq!(hash.len(), 64); // SHA-256 hex
                                    // Verify hash matches key
        assert_eq!(hash_key(&key), hash);
    }

    #[test]
    fn format_and_parse_key() {
        let (key, _) = generate_agent_key();
        let formatted = format_agent_key(&key);
        let parsed = parse_agent_key(&formatted).unwrap();
        assert_eq!(key.as_slice(), parsed.as_slice());
    }
}
