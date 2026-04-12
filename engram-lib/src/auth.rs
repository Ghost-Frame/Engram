use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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

fn normalize_key(raw_key: &str) -> Option<String> {
    let hex_portion = if let Some(rest) = raw_key.strip_prefix("engram_") {
        rest
    } else if let Some(rest) = raw_key.strip_prefix("eg_") {
        rest
    } else {
        return None;
    };

    if hex_portion.len() != 32 || !hex_portion.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }

    Some(format!("engram_{}", hex_portion.to_ascii_lowercase()))
}

/// Generate a new random API key.
///
/// SECURITY (SEC-HIGH-2): keys draw 16 raw bytes from `OsRng`, giving a
/// full 128 bits of unpredictability rendered as 32 lowercase hex chars.
/// The previous implementation concatenated two UUID v4 strings and
/// truncated to 32 characters, which embedded fixed version/variant bits
/// in the middle of the key and reduced effective entropy below the
/// advertised 128-bit strength. Returns `(full_key, key_prefix, key_hash)`.
fn generate_key() -> (String, String, String) {
    use rand::RngCore;
    let mut raw = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    let mut raw_hex = String::with_capacity(32);
    for byte in raw {
        use std::fmt::Write;
        let _ = write!(&mut raw_hex, "{:02x}", byte);
    }

    let full_key = format!("engram_{}", raw_hex);
    // key_prefix = first 8 chars of the hex portion (chars 7..15 of full_key)
    let key_prefix = raw_hex[..8].to_string();
    let key_hash = hash_key(&full_key);

    (full_key, key_prefix, key_hash)
}

/// Parse a comma-separated scopes string into a Vec<Scope>.
fn parse_scopes(s: &str) -> Vec<Scope> {
    // Legacy "*" means "all scopes". Without this translation legacy keys
    // stored before the stricter scope model would parse to an empty Vec and
    // lose all access when scope checks were introduced.
    let trimmed = s.trim();
    if trimmed == "*" {
        return vec![Scope::Read, Scope::Write, Scope::Admin];
    }
    let mut out: Vec<Scope> = Vec::new();
    for part in trimmed.split(',') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        if p == "*" {
            return vec![Scope::Read, Scope::Write, Scope::Admin];
        }
        if let Ok(scope) = p.parse::<Scope>() {
            out.push(scope);
        }
    }
    out
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
            libsql::params![
                user_id,
                key_prefix.clone(),
                key_hash.clone(),
                name,
                scopes_str
            ],
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
    let normalized_key =
        normalize_key(raw_key).ok_or_else(|| crate::EngError::Auth("invalid key format".into()))?;
    let hex_portion = &normalized_key[7..];
    let key_prefix = &hex_portion[..8];
    let key_hash = hash_key(&normalized_key);

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

    // Check expiration. Parse expires_at as a real timestamp rather than a
    // lexical string compare -- the previous compare broke on timezone
    // suffixes and on any ISO-8601 variant that doesn't match the exact
    // `%Y-%m-%d %H:%M:%S` formatting we produce on insert.
    if let Some(ref expires_at) = api_key.expires_at {
        let parsed = chrono::NaiveDateTime::parse_from_str(expires_at, "%Y-%m-%d %H:%M:%S")
            .or_else(|_| {
                chrono::DateTime::parse_from_rfc3339(expires_at).map(|dt| dt.naive_utc())
            })
            .map_err(|_| crate::EngError::Auth("invalid key expiry format".into()))?;
        if parsed <= chrono::Utc::now().naive_utc() {
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

/// Deactivate an API key by id, scoped to the owning user.
///
/// SECURITY (SEC-HIGH-5): the `user_id` filter is defense-in-depth. All
/// callers should already verify ownership before reaching this function,
/// but constraining the UPDATE here means any future caller that forgets
/// to check ownership still cannot revoke another tenant's keys.
pub async fn revoke_key(db: &Database, user_id: i64, key_id: i64) -> Result<()> {
    db.conn
        .execute(
            "UPDATE api_keys SET is_active = 0 WHERE id = ?1 AND user_id = ?2",
            libsql::params![key_id, user_id],
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
