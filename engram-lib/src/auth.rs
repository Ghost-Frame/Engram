use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::OnceLock;
use subtle::ConstantTimeEq;

use crate::db::Database;
use crate::Result;

/// Hash version for API keys.
/// v1 = legacy SHA-256(raw_key)
/// v2 = SHA-256(pepper || raw_key) when ENGRAM_API_KEY_PEPPER is set
const HASH_VERSION_LEGACY: i32 = 1;
const HASH_VERSION_PEPPERED: i32 = 2;

/// Cached pepper from ENGRAM_API_KEY_PEPPER environment variable.
/// Must be 64 hex characters (32 bytes).
static API_KEY_PEPPER: OnceLock<Option<[u8; 32]>> = OnceLock::new();

fn get_pepper() -> Option<[u8; 32]> {
    *API_KEY_PEPPER.get_or_init(|| {
        std::env::var("ENGRAM_API_KEY_PEPPER").ok().and_then(|hex| {
            if hex.len() != 64 {
                tracing::warn!(
                    "ENGRAM_API_KEY_PEPPER must be 64 hex chars (32 bytes), got {}",
                    hex.len()
                );
                return None;
            }
            let bytes: std::result::Result<Vec<u8>, _> = (0..32)
                .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16))
                .collect();
            match bytes {
                Ok(v) => {
                    let mut arr = [0u8; 32];
                    arr.copy_from_slice(&v);
                    tracing::info!("API key pepper loaded (v2 hashing enabled)");
                    Some(arr)
                }
                Err(_) => {
                    tracing::warn!("ENGRAM_API_KEY_PEPPER contains invalid hex");
                    None
                }
            }
        })
    })
}

fn rusqlite_to_eng_error(err: rusqlite::Error) -> crate::EngError {
    crate::EngError::DatabaseMessage(err.to_string())
}

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
    /// Hash version: 1 = legacy SHA-256, 2 = peppered SHA-256
    #[serde(default = "default_hash_version")]
    pub hash_version: i32,
}

fn default_hash_version() -> i32 {
    HASH_VERSION_LEGACY
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

/// Hash a raw key with SHA-256 (v1 legacy, no pepper).
fn hash_key_v1(raw_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw_key.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Hash a raw key with SHA-256 using pepper (v2).
/// Returns None if pepper is not configured.
fn hash_key_v2(raw_key: &str) -> Option<String> {
    get_pepper().map(|pepper| {
        let mut hasher = Sha256::new();
        hasher.update(pepper);
        hasher.update(raw_key.as_bytes());
        format!("{:x}", hasher.finalize())
    })
}

/// Hash a raw key using the specified version.
fn hash_key_versioned(raw_key: &str, version: i32) -> Option<String> {
    match version {
        HASH_VERSION_LEGACY => Some(hash_key_v1(raw_key)),
        HASH_VERSION_PEPPERED => hash_key_v2(raw_key),
        _ => None,
    }
}

/// Normalise a raw API key to its canonical `engram_<hex>` form.
///
/// SECURITY (SEC-LOW-2): the `eg_` prefix is a legacy shorthand alias kept
/// for backwards compatibility with older clients. Both prefixes map to the
/// same canonical form so hash lookups succeed regardless of which prefix the
/// caller used.
fn normalize_key(raw_key: &str) -> Option<String> {
    let hex_portion = if let Some(rest) = raw_key.strip_prefix("engram_") {
        rest
    } else if let Some(rest) = raw_key.strip_prefix("eg_") {
        rest
    } else {
        return None;
    };

    // Accept both 32-char (new format) and 64-char (legacy TS format) keys
    let valid_len = hex_portion.len() == 32 || hex_portion.len() == 64;
    if !valid_len || !hex_portion.chars().all(|c| c.is_ascii_hexdigit()) {
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
/// advertised 128-bit strength.
///
/// Returns `(full_key, key_prefix, key_hash, hash_version)`.
/// Uses v2 (peppered) hashing if ENGRAM_API_KEY_PEPPER is set, otherwise v1.
fn generate_key() -> (String, String, String, i32) {
    use rand::Rng;
    let mut raw = [0u8; 16];
    rand::rng().fill(&mut raw);
    let mut raw_hex = String::with_capacity(32);
    for byte in raw {
        use std::fmt::Write;
        let _ = write!(&mut raw_hex, "{:02x}", byte);
    }

    let full_key = format!("engram_{}", raw_hex);
    // key_prefix = first 8 chars of the hex portion (chars 7..15 of full_key)
    let key_prefix = raw_hex[..8].to_string();

    // Use v2 if pepper is configured, otherwise v1
    let (key_hash, hash_version) = if let Some(hash) = hash_key_v2(&full_key) {
        (hash, HASH_VERSION_PEPPERED)
    } else {
        (hash_key_v1(&full_key), HASH_VERSION_LEGACY)
    };

    (full_key, key_prefix, key_hash, hash_version)
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
/// `rate_limit`: requests-per-minute cap. None uses the column default (1000).
#[tracing::instrument(skip(db, name, scopes), fields(name = %name, scope_count = scopes.len()))]
pub async fn create_key(
    db: &Database,
    user_id: i64,
    name: &str,
    scopes: Vec<Scope>,
    rate_limit: Option<i64>,
) -> Result<(ApiKey, String)> {
    create_key_with_expiry(db, user_id, name, scopes, rate_limit, None).await
}

/// Create a new API key with an optional absolute `expires_at` (as a
/// chrono-serialized `YYYY-MM-DD HH:MM:SS` string or RFC3339). When `None`,
/// the key does not expire.
#[tracing::instrument(skip(db, name, scopes, expires_at), fields(name = %name, scope_count = scopes.len()))]
pub async fn create_key_with_expiry(
    db: &Database,
    user_id: i64,
    name: &str,
    scopes: Vec<Scope>,
    rate_limit: Option<i64>,
    expires_at: Option<String>,
) -> Result<(ApiKey, String)> {
    let (full_key, key_prefix, key_hash, hash_version) = generate_key();
    let scopes_str = scopes_to_string(&scopes);
    let rate_limit_val = rate_limit.unwrap_or(1000).max(1);
    let name_owned = name.to_string();

    if let Some(ref s) = expires_at {
        chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
            .map(|_| ())
            .or_else(|_| chrono::DateTime::parse_from_rfc3339(s).map(|_| ()))
            .map_err(|_| crate::EngError::InvalidInput("invalid expires_at format".into()))?;
    }

    let key_prefix_for_read = key_prefix.clone();
    let key_hash_for_read = key_hash.clone();

    db.write(move |conn| {
        conn.execute(
            "INSERT INTO api_keys (user_id, key_prefix, key_hash, name, scopes, rate_limit, hash_version, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                user_id,
                key_prefix,
                key_hash,
                name_owned,
                scopes_str,
                rate_limit_val,
                hash_version,
                expires_at
            ],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await?;

    let api_key = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, user_id, key_prefix, name, scopes, rate_limit, is_active,
                            agent_id, last_used_at, expires_at, created_at, hash_version
                     FROM api_keys
                     WHERE key_prefix = ?1 AND key_hash = ?2
                     LIMIT 1",
                )
                .map_err(rusqlite_to_eng_error)?;

            let key = stmt
                .query_row(
                    rusqlite::params![key_prefix_for_read, key_hash_for_read],
                    row_to_api_key_rusqlite,
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => {
                        crate::EngError::Internal("failed to fetch newly created key".into())
                    }
                    other => rusqlite_to_eng_error(other),
                })?;

            Ok(key)
        })
        .await?;

    Ok((api_key, full_key))
}

/// Validate a raw API key from a request. Returns an AuthContext on success.
#[tracing::instrument(skip(db, raw_key))]
pub async fn validate_key(db: &Database, raw_key: &str) -> Result<AuthContext> {
    let normalized_key =
        normalize_key(raw_key).ok_or_else(|| crate::EngError::Auth("invalid key format".into()))?;
    let hex_portion = normalized_key[7..].to_string();
    let key_prefix = hex_portion[..8].to_string();

    // Compute hashes for both versions upfront
    let hash_v1 = hash_key_v1(&normalized_key);
    let hash_v2 = hash_key_v2(&normalized_key);

    let api_key = db
        .read(move |conn| {
            // Fetch candidate keys by prefix (there should typically be only one)
            let mut stmt = conn
                .prepare(
                    "SELECT id, user_id, key_prefix, name, scopes, rate_limit, is_active,
                            agent_id, last_used_at, expires_at, created_at, hash_version, key_hash
                     FROM api_keys
                     WHERE key_prefix = ?1 AND is_active = 1",
                )
                .map_err(rusqlite_to_eng_error)?;

            let mut rows = stmt
                .query(rusqlite::params![key_prefix])
                .map_err(rusqlite_to_eng_error)?;

            // Check each candidate against the appropriate hash version
            while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                let hash_version: i32 = row.get(11).unwrap_or(HASH_VERSION_LEGACY);
                let stored_hash: String = row.get(12).map_err(rusqlite_to_eng_error)?;

                let expected_hash = match hash_version {
                    HASH_VERSION_PEPPERED => hash_v2.as_ref(),
                    _ => {
                        // SECURITY (SEC-C5): reject v1 (unpeppered) keys when
                        // pepper is configured. This prevents a downgrade attack
                        // where an attacker who can modify the api_keys table
                        // flips hash_version to bypass the pepper.
                        if hash_v2.is_some() {
                            tracing::warn!(
                                key_prefix = %key_prefix,
                                "rejecting v1 (unpeppered) key while pepper is configured -- run key migration"
                            );
                            continue;
                        }
                        Some(&hash_v1)
                    }
                };

                // SECURITY (SEC-C2): constant-time comparison to prevent
                // timing oracle attacks on hash values.
                let matches = match expected_hash {
                    Some(expected) => {
                        expected.len() == stored_hash.len()
                            && expected
                                .as_bytes()
                                .ct_eq(stored_hash.as_bytes())
                                .unwrap_u8()
                                == 1
                    }
                    None => false,
                };

                if matches {
                    // Found matching key -- reconstruct without key_hash column
                    return row_to_api_key_rusqlite_with_offset(row);
                }
            }

            Err(crate::EngError::Auth("invalid or revoked key".into()))
        })
        .await?;

    // Check expiration. Parse expires_at as a real timestamp rather than a
    // lexical string compare -- the previous compare broke on timezone
    // suffixes and on any ISO-8601 variant that doesn't match the exact
    // `%Y-%m-%d %H:%M:%S` formatting we produce on insert.
    if let Some(ref expires_at) = api_key.expires_at {
        let parsed = chrono::NaiveDateTime::parse_from_str(expires_at, "%Y-%m-%d %H:%M:%S")
            .or_else(|_| chrono::DateTime::parse_from_rfc3339(expires_at).map(|dt| dt.naive_utc()))
            .map_err(|_| crate::EngError::Auth("invalid key expiry format".into()))?;
        if parsed <= chrono::Utc::now().naive_utc() {
            return Err(crate::EngError::Auth("key has expired".into()));
        }
    }

    let user_id = api_key.user_id;
    let key_id = api_key.id;

    // Update last_used_at -- fire and don't fail validation on error
    let _ = db
        .write(move |conn| {
            conn.execute(
                "UPDATE api_keys SET last_used_at = datetime('now') WHERE id = ?1",
                rusqlite::params![key_id],
            )
            .map_err(rusqlite_to_eng_error)?;
            Ok(())
        })
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
#[tracing::instrument(skip(db))]
pub async fn revoke_key(db: &Database, user_id: i64, key_id: i64) -> Result<()> {
    db.write(move |conn| {
        conn.execute(
            "UPDATE api_keys SET is_active = 0 WHERE id = ?1 AND user_id = ?2",
            rusqlite::params![key_id, user_id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

/// Deactivate any API key by id regardless of owner (admin use only).
#[tracing::instrument(skip(db))]
pub async fn revoke_key_admin(db: &Database, key_id: i64) -> Result<()> {
    db.write(move |conn| {
        conn.execute(
            "UPDATE api_keys SET is_active = 0 WHERE id = ?1",
            rusqlite::params![key_id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

/// Look up an active, unexpired API key by id.
#[tracing::instrument(skip(db))]
pub async fn get_active_key_by_id(db: &Database, key_id: i64) -> Result<ApiKey> {
    let api_key = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, user_id, key_prefix, name, scopes, rate_limit, is_active,
                            agent_id, last_used_at, expires_at, created_at, hash_version
                     FROM api_keys
                     WHERE id = ?1 AND is_active = 1
                     LIMIT 1",
                )
                .map_err(rusqlite_to_eng_error)?;

            let key = stmt
                .query_row(rusqlite::params![key_id], row_to_api_key_rusqlite)
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => {
                        crate::EngError::Auth("invalid or revoked key".into())
                    }
                    other => rusqlite_to_eng_error(other),
                })?;

            Ok(key)
        })
        .await?;

    if let Some(ref expires_at) = api_key.expires_at {
        let parsed = chrono::NaiveDateTime::parse_from_str(expires_at, "%Y-%m-%d %H:%M:%S")
            .or_else(|_| chrono::DateTime::parse_from_rfc3339(expires_at).map(|dt| dt.naive_utc()))
            .map_err(|_| crate::EngError::Auth("invalid key expiry format".into()))?;
        if parsed <= chrono::Utc::now().naive_utc() {
            return Err(crate::EngError::Auth("key has expired".into()));
        }
    }

    Ok(api_key)
}

/// List active API keys for a user. Never exposes key_hash.
#[tracing::instrument(skip(db))]
pub async fn list_keys(db: &Database, user_id: i64) -> Result<Vec<ApiKey>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, user_id, key_prefix, name, scopes, rate_limit, is_active,
                        agent_id, last_used_at, expires_at, created_at, hash_version
                 FROM api_keys
                 WHERE user_id = ?1 AND is_active = 1
                 ORDER BY created_at DESC",
            )
            .map_err(rusqlite_to_eng_error)?;

        let keys = stmt
            .query_map(rusqlite::params![user_id], |row| {
                row_to_api_key_rusqlite(row)
            })
            .map_err(rusqlite_to_eng_error)?
            .map(|r| r.map_err(rusqlite_to_eng_error))
            .collect::<Result<Vec<ApiKey>>>()?;

        Ok(keys)
    })
    .await
}

// ---------------------------------------------------------------------------
// Row mapping
// ---------------------------------------------------------------------------

/// Standard row mapping: expects columns 0-11 in order:
/// id, user_id, key_prefix, name, scopes, rate_limit, is_active,
/// agent_id, last_used_at, expires_at, created_at, hash_version
fn row_to_api_key_rusqlite(row: &rusqlite::Row<'_>) -> rusqlite::Result<ApiKey> {
    let id: i64 = row.get(0)?;
    let user_id: i64 = row.get(1)?;
    let key_prefix: String = row.get(2)?;
    let name: String = row.get(3)?;
    let scopes_str: String = row.get(4)?;
    let rate_limit: i32 = row.get(5)?;
    let is_active_int: i32 = row.get(6)?;
    let agent_id: Option<i64> = row.get(7)?;
    let last_used_at: Option<String> = row.get(8)?;
    let expires_at: Option<String> = row.get(9)?;
    let created_at: String = row.get(10)?;
    let hash_version: i32 = row.get(11).unwrap_or(HASH_VERSION_LEGACY);

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
        hash_version,
    })
}

/// Variant for validate_key: same columns but with key_hash at position 12.
/// We read hash_version from position 11, skip key_hash.
fn row_to_api_key_rusqlite_with_offset(row: &rusqlite::Row<'_>) -> crate::Result<ApiKey> {
    let id: i64 = row.get(0).map_err(rusqlite_to_eng_error)?;
    let user_id: i64 = row.get(1).map_err(rusqlite_to_eng_error)?;
    let key_prefix: String = row.get(2).map_err(rusqlite_to_eng_error)?;
    let name: String = row.get(3).map_err(rusqlite_to_eng_error)?;
    let scopes_str: String = row.get(4).map_err(rusqlite_to_eng_error)?;
    let rate_limit: i32 = row.get(5).map_err(rusqlite_to_eng_error)?;
    let is_active_int: i32 = row.get(6).map_err(rusqlite_to_eng_error)?;
    let agent_id: Option<i64> = row.get(7).map_err(rusqlite_to_eng_error)?;
    let last_used_at: Option<String> = row.get(8).map_err(rusqlite_to_eng_error)?;
    let expires_at: Option<String> = row.get(9).map_err(rusqlite_to_eng_error)?;
    let created_at: String = row.get(10).map_err(rusqlite_to_eng_error)?;
    let hash_version: i32 = row.get(11).unwrap_or(HASH_VERSION_LEGACY);
    // position 12 is key_hash, not needed in ApiKey struct

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
        hash_version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::db::Database;

    async fn setup_db() -> Database {
        let db_path = std::env::temp_dir()
            .join(format!("engram-auth-test-{}.db", uuid::Uuid::new_v4()))
            .to_string_lossy()
            .into_owned();
        let config = Config {
            db_path,
            use_lance_index: false,
            ..Config::default()
        };
        Database::connect_with_config(&config, None).await.unwrap()
    }

    async fn make_user(db: &Database, username: &str) -> i64 {
        let username = username.to_string();
        db.write(move |conn| {
            conn.query_row(
                "INSERT INTO users (username, role, is_admin) VALUES (?1, 'admin', 1) RETURNING id",
                rusqlite::params![username],
                |row| row.get::<_, i64>(0),
            )
            .map_err(rusqlite_to_eng_error)
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn create_key_with_expiry_persists_absolute_timestamp() {
        let db = setup_db().await;
        let uid = make_user(&db, "alice").await;

        let expiry = (chrono::Utc::now() + chrono::Duration::hours(6))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();

        let (api_key, _raw) = create_key_with_expiry(
            &db,
            uid,
            "rotated",
            vec![Scope::Read],
            Some(250),
            Some(expiry.clone()),
        )
        .await
        .expect("create_key_with_expiry should succeed");

        assert_eq!(api_key.expires_at.as_deref(), Some(expiry.as_str()));
        assert_eq!(api_key.rate_limit, 250);
    }

    #[tokio::test]
    async fn create_key_with_expiry_rejects_malformed_timestamp() {
        let db = setup_db().await;
        let uid = make_user(&db, "bob").await;

        let err = create_key_with_expiry(
            &db,
            uid,
            "bad",
            vec![Scope::Read],
            None,
            Some("not-a-timestamp".into()),
        )
        .await
        .expect_err("malformed expires_at must be rejected");

        assert!(matches!(err, crate::EngError::InvalidInput(_)));
    }
}
