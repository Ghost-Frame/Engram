use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::db::Database;
use crate::Result;

// ---------------------------------------------------------------------------
// Scope
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Read,
    Write,
    Admin,
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read => write!(f, "read"),
            Self::Write => write!(f, "write"),
            Self::Admin => write!(f, "admin"),
        }
    }
}

impl std::str::FromStr for Scope {
    type Err = crate::EngError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "read" => Ok(Self::Read),
            "write" => Ok(Self::Write),
            "admin" => Ok(Self::Admin),
            _ => Err(crate::EngError::InvalidInput(format!(
                "unknown scope: {}",
                s
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// ApiKey
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKey {
    pub id: i64,
    pub user_id: i64,
    pub key_prefix: String,
    pub name: String,
    pub scopes: Vec<Scope>,
    pub rate_limit: i32,
    pub is_active: bool,
    pub agent_id: Option<i64>,
    pub last_used_at: Option<String>,
    pub expires_at: Option<String>,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// AuthContext
// ---------------------------------------------------------------------------

/// Result of validating an API key -- includes resolved user info.
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub key: ApiKey,
    pub user_id: i64,
}

impl AuthContext {
    pub fn has_scope(&self, scope: &Scope) -> bool {
        self.key.scopes.contains(scope) || self.key.scopes.contains(&Scope::Admin)
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Hash a raw key with SHA-256, returning a lowercase hex string.
fn hash_key(raw_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw_key.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Generate a new random API key.
/// Returns (full_key, key_prefix, key_hash).
fn generate_key() -> (String, String, String) {
    // Two UUIDs concatenated and stripped of hyphens give 64 hex chars.
    // Take the first 32.
    let part_a = Uuid::new_v4().simple().to_string(); // 32 hex chars
    let part_b = Uuid::new_v4().simple().to_string(); // 32 hex chars
    let raw_hex: String = format!("{}{}", part_a, part_b)
        .chars()
        .take(32)
        .collect();

    let full_key = format!("engram_{}", raw_hex);
    // key_prefix = first 8 chars of the hex portion (chars 7..15 of full_key)
    let key_prefix = raw_hex[..8].to_string();
    let key_hash = hash_key(&full_key);

    (full_key, key_prefix, key_hash)
}

/// Parse a comma-separated scopes string into a Vec<Scope>.
fn parse_scopes(s: &str) -> Vec<Scope> {
    s.split(',')
        .filter_map(|part| part.trim().parse().ok())
        .collect()
}

/// Serialize a slice of Scope values to a comma-separated string.
fn scopes_to_string(scopes: &[Scope]) -> String {
    scopes
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new API key for a user and store it in the database.
/// Returns (ApiKey, raw_key). The raw_key is shown once and never stored.
pub async fn create_key(
    db: &Database,
    user_id: i64,
    name: &str,
    scopes: Vec<Scope>,
) -> Result<(ApiKey, String)> {
    let (full_key, key_prefix, key_hash) = generate_key();
    let scopes_str = scopes_to_string(&scopes);

    db.conn
        .execute(
            "INSERT INTO api_keys (user_id, key_prefix, key_hash, name, scopes)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            libsql::params![user_id, key_prefix.clone(), key_hash.clone(), name, scopes_str],
        )
        .await?;

    // Fetch the inserted row to get generated fields (id, rate_limit, created_at, etc.)
    let mut rows = db
        .conn
        .query(
            "SELECT id, user_id, key_prefix, name, scopes, rate_limit, is_active,
                    agent_id, last_used_at, expires_at, created_at
             FROM api_keys
             WHERE key_prefix = ?1 AND key_hash = ?2
             LIMIT 1",
            libsql::params![key_prefix, key_hash],
        )
        .await?;

    let row = rows
        .next()
        .await?
        .ok_or_else(|| crate::EngError::Internal("failed to fetch newly created key".into()))?;

    let api_key = row_to_api_key(&row)?;
    Ok((api_key, full_key))
}

/// Validate a raw API key from a request. Returns an AuthContext on success.
pub async fn validate_key(db: &Database, raw_key: &str) -> Result<AuthContext> {
    // Basic format check: "engram_" + 32 hex chars = 39 chars total
    if !raw_key.starts_with("engram_") || raw_key.len() < 39 {
        return Err(crate::EngError::Auth("invalid key format".into()));
    }

    let hex_portion = &raw_key[7..]; // everything after "engram_"
    let key_prefix = &hex_portion[..8];
    let key_hash = hash_key(raw_key);

    let mut rows = db
        .conn
        .query(
            "SELECT id, user_id, key_prefix, name, scopes, rate_limit, is_active,
                    agent_id, last_used_at, expires_at, created_at
             FROM api_keys
             WHERE key_prefix = ?1 AND key_hash = ?2 AND is_active = 1
             LIMIT 1",
            libsql::params![key_prefix, key_hash],
        )
        .await?;

    let row = rows
        .next()
        .await?
        .ok_or_else(|| crate::EngError::Auth("invalid or revoked key".into()))?;

    let api_key = row_to_api_key(&row)?;

    // Check expiration
    if let Some(ref expires_at) = api_key.expires_at {
        let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        if expires_at.as_str() < now.as_str() {
            return Err(crate::EngError::Auth("key has expired".into()));
        }
    }

    let user_id = api_key.user_id;
    let key_id = api_key.id;

    // Update last_used_at -- fire and don't fail validation on error
    let _ = db
        .conn
        .execute(
            "UPDATE api_keys SET last_used_at = datetime('now') WHERE id = ?1",
            libsql::params![key_id],
        )
        .await;

    Ok(AuthContext {
        key: api_key,
        user_id,
    })
}

/// Deactivate an API key by id.
pub async fn revoke_key(db: &Database, key_id: i64) -> Result<()> {
    db.conn
        .execute(
            "UPDATE api_keys SET is_active = 0 WHERE id = ?1",
            libsql::params![key_id],
        )
        .await?;
    Ok(())
}

/// List active API keys for a user. Never exposes key_hash.
pub async fn list_keys(db: &Database, user_id: i64) -> Result<Vec<ApiKey>> {
    let mut rows = db
        .conn
        .query(
            "SELECT id, user_id, key_prefix, name, scopes, rate_limit, is_active,
                    agent_id, last_used_at, expires_at, created_at
             FROM api_keys
             WHERE user_id = ?1 AND is_active = 1
             ORDER BY created_at DESC",
            libsql::params![user_id],
        )
        .await?;

    let mut keys = Vec::new();
    while let Some(row) = rows.next().await? {
        keys.push(row_to_api_key(&row)?);
    }
    Ok(keys)
}

// ---------------------------------------------------------------------------
// Row mapping
// ---------------------------------------------------------------------------

fn row_to_api_key(row: &libsql::Row) -> Result<ApiKey> {
    let id: i64 = row
        .get(0)
        .map_err(|e| crate::EngError::Internal(e.to_string()))?;
    let user_id: i64 = row
        .get(1)
        .map_err(|e| crate::EngError::Internal(e.to_string()))?;
    let key_prefix: String = row
        .get(2)
        .map_err(|e| crate::EngError::Internal(e.to_string()))?;
    let name: String = row
        .get(3)
        .map_err(|e| crate::EngError::Internal(e.to_string()))?;
    let scopes_str: String = row
        .get(4)
        .map_err(|e| crate::EngError::Internal(e.to_string()))?;
    let rate_limit: i32 = row
        .get(5)
        .map_err(|e| crate::EngError::Internal(e.to_string()))?;
    let is_active_int: i32 = row
        .get(6)
        .map_err(|e| crate::EngError::Internal(e.to_string()))?;
    let agent_id: Option<i64> = row
        .get(7)
        .map_err(|e| crate::EngError::Internal(e.to_string()))?;
    let last_used_at: Option<String> = row
        .get(8)
        .map_err(|e| crate::EngError::Internal(e.to_string()))?;
    let expires_at: Option<String> = row
        .get(9)
        .map_err(|e| crate::EngError::Internal(e.to_string()))?;
    let created_at: String = row
        .get(10)
        .map_err(|e| crate::EngError::Internal(e.to_string()))?;

    Ok(ApiKey {
        id,
        user_id,
        key_prefix,
        name,
        scopes: parse_scopes(&scopes_str),
        rate_limit,
        is_active: is_active_int != 0,
        agent_id,
        last_used_at,
        expires_at,
        created_at,
    })
}
