//! Recovery key system for lost YubiKey scenarios.
//!
//! When a user sets up their credential vault, they can generate a recovery key.
//! This recovery key is encrypted with the master encryption key and stored.
//! If the YubiKey is lost, the recovery key can be used to re-derive the master key.

use rand::RngCore;

use crate::crypto::{decrypt_secret, encrypt_secret, KEY_SIZE};
use crate::types::SecretData;
use crate::{CredError, Result};
use engram_lib::db::Database;

/// Recovery key length (256 bits = 32 bytes, displayed as 64 hex chars).
pub const RECOVERY_KEY_SIZE: usize = 32;

/// Number of words in a recovery phrase (BIP39-style).
pub const RECOVERY_PHRASE_WORDS: usize = 24;

/// Recovery key info.
#[derive(Debug, Clone)]
pub struct RecoveryInfo {
    pub id: i64,
    pub user_id: i64,
    pub hint: Option<String>,
    pub created_at: String,
}

/// Generate a new recovery key.
///
/// Returns the raw recovery key bytes. The user should write this down
/// and store it securely offline.
pub fn generate_recovery_key() -> [u8; RECOVERY_KEY_SIZE] {
    let mut key = [0u8; RECOVERY_KEY_SIZE];
    rand::thread_rng().fill_bytes(&mut key);
    key
}

/// Format a recovery key as a displayable hex string.
pub fn format_recovery_key(key: &[u8; RECOVERY_KEY_SIZE]) -> String {
    hex::encode(key)
}

/// Parse a hex-encoded recovery key.
pub fn parse_recovery_key(encoded: &str) -> Result<[u8; RECOVERY_KEY_SIZE]> {
    let bytes = hex::decode(encoded.trim())
        .map_err(|e| CredError::InvalidInput(format!("invalid recovery key format: {}", e)))?;

    if bytes.len() != RECOVERY_KEY_SIZE {
        return Err(CredError::InvalidInput(format!(
            "recovery key must be {} bytes, got {}",
            RECOVERY_KEY_SIZE,
            bytes.len()
        )));
    }

    let mut key = [0u8; RECOVERY_KEY_SIZE];
    key.copy_from_slice(&bytes);
    Ok(key)
}

/// Store a recovery key for a user.
///
/// The master key is encrypted with the recovery key and stored in the database.
/// This allows recovering access if the primary authentication (YubiKey/password) is lost.
pub async fn store_recovery_key(
    db: &Database,
    user_id: i64,
    recovery_key: &[u8; RECOVERY_KEY_SIZE],
    master_key: &[u8; KEY_SIZE],
    hint: Option<&str>,
) -> Result<i64> {
    // Encrypt the master key with the recovery key
    let master_secret = SecretData::Note {
        content: hex::encode(master_key),
    };
    let (encrypted, nonce) = encrypt_secret(recovery_key, &master_secret)?;

    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

    // Delete any existing recovery key for this user
    db.conn
        .execute(
            "DELETE FROM cred_recovery WHERE user_id = ?1",
            libsql::params![user_id],
        )
        .await?;

    // Store the new recovery key
    let mut encrypted_blob = nonce.to_vec();
    encrypted_blob.extend_from_slice(&encrypted);

    db.conn
        .execute(
            "INSERT INTO cred_recovery (user_id, encrypted_master, recovery_hint, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            libsql::params![user_id, encrypted_blob, hint, now],
        )
        .await?;

    let mut rows = db.conn.query("SELECT last_insert_rowid()", ()).await?;
    let id: i64 = match rows.next().await? {
        Some(row) => row.get(0)?,
        None => 0,
    };

    Ok(id)
}

/// Recover the master key using a recovery key.
pub async fn recover_master_key(
    db: &Database,
    user_id: i64,
    recovery_key: &[u8; RECOVERY_KEY_SIZE],
) -> Result<[u8; KEY_SIZE]> {
    let mut rows = db
        .conn
        .query(
            "SELECT encrypted_master FROM cred_recovery WHERE user_id = ?1",
            libsql::params![user_id],
        )
        .await?;

    let row = rows
        .next()
        .await?
        .ok_or_else(|| CredError::NotFound("no recovery key stored".into()))?;

    let encrypted_blob: Vec<u8> = row.get(0)?;

    if encrypted_blob.len() < 12 {
        return Err(CredError::Decryption("invalid recovery data".into()));
    }

    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&encrypted_blob[..12]);
    let encrypted = &encrypted_blob[12..];

    let secret = decrypt_secret(recovery_key, encrypted, &nonce)?;

    match secret {
        SecretData::Note { content } => {
            let master_bytes = hex::decode(&content)
                .map_err(|e| CredError::Decryption(format!("invalid master key: {}", e)))?;

            if master_bytes.len() != KEY_SIZE {
                return Err(CredError::Decryption("invalid master key length".into()));
            }

            let mut master_key = [0u8; KEY_SIZE];
            master_key.copy_from_slice(&master_bytes);
            Ok(master_key)
        }
        _ => Err(CredError::Decryption("unexpected recovery data type".into())),
    }
}

/// Check if a user has a recovery key stored.
pub async fn has_recovery_key(db: &Database, user_id: i64) -> Result<bool> {
    let mut rows = db
        .conn
        .query(
            "SELECT id FROM cred_recovery WHERE user_id = ?1",
            libsql::params![user_id],
        )
        .await?;

    Ok(rows.next().await?.is_some())
}

/// Get recovery key info for a user.
pub async fn get_recovery_info(db: &Database, user_id: i64) -> Result<Option<RecoveryInfo>> {
    let mut rows = db
        .conn
        .query(
            "SELECT id, user_id, recovery_hint, created_at FROM cred_recovery WHERE user_id = ?1",
            libsql::params![user_id],
        )
        .await?;

    match rows.next().await? {
        Some(row) => {
            let id: i64 = row.get(0)?;
            let user_id: i64 = row.get(1)?;
            let hint: Option<String> = row.get(2)?;
            let created_at: String = row.get(3)?;

            Ok(Some(RecoveryInfo {
                id,
                user_id,
                hint,
                created_at,
            }))
        }
        None => Ok(None),
    }
}

/// Delete the recovery key for a user.
pub async fn delete_recovery_key(db: &Database, user_id: i64) -> Result<()> {
    let affected = db
        .conn
        .execute(
            "DELETE FROM cred_recovery WHERE user_id = ?1",
            libsql::params![user_id],
        )
        .await?;

    if affected == 0 {
        return Err(CredError::NotFound("no recovery key to delete".into()));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::derive_key;

    async fn setup_db() -> Database {
        let db = Database::connect_memory().await.expect("db");
        db.conn
            .execute(
                "CREATE TABLE IF NOT EXISTS cred_recovery (
                    id INTEGER PRIMARY KEY,
                    user_id INTEGER NOT NULL UNIQUE,
                    encrypted_master BLOB NOT NULL,
                    recovery_hint TEXT,
                    created_at TEXT NOT NULL
                )",
                (),
            )
            .await
            .expect("create table");
        db
    }

    #[test]
    fn generate_recovery_key_random() {
        let k1 = generate_recovery_key();
        let k2 = generate_recovery_key();
        assert_ne!(k1, k2);
    }

    #[test]
    fn format_and_parse_recovery_key() {
        let key = generate_recovery_key();
        let formatted = format_recovery_key(&key);
        assert_eq!(formatted.len(), 64); // 32 bytes * 2 hex chars

        let parsed = parse_recovery_key(&formatted).unwrap();
        assert_eq!(key, parsed);
    }

    #[test]
    fn parse_recovery_key_with_whitespace() {
        let key = generate_recovery_key();
        let formatted = format_recovery_key(&key);
        let with_spaces = format!("  {}  ", formatted);

        let parsed = parse_recovery_key(&with_spaces).unwrap();
        assert_eq!(key, parsed);
    }

    #[tokio::test]
    async fn store_and_recover_master_key() {
        let db = setup_db().await;
        let recovery_key = generate_recovery_key();
        let master_key = derive_key(1, b"password", None);

        store_recovery_key(&db, 1, &recovery_key, &master_key, Some("test hint"))
            .await
            .expect("store");

        assert!(has_recovery_key(&db, 1).await.expect("check"));

        let info = get_recovery_info(&db, 1).await.expect("info").unwrap();
        assert_eq!(info.hint, Some("test hint".into()));

        let recovered = recover_master_key(&db, 1, &recovery_key)
            .await
            .expect("recover");
        assert_eq!(master_key, recovered);
    }

    #[tokio::test]
    async fn wrong_recovery_key_fails() {
        let db = setup_db().await;
        let recovery_key = generate_recovery_key();
        let wrong_key = generate_recovery_key();
        let master_key = derive_key(1, b"password", None);

        store_recovery_key(&db, 1, &recovery_key, &master_key, None)
            .await
            .expect("store");

        let result = recover_master_key(&db, 1, &wrong_key).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn delete_recovery_key_removes_it() {
        let db = setup_db().await;
        let recovery_key = generate_recovery_key();
        let master_key = derive_key(1, b"password", None);

        store_recovery_key(&db, 1, &recovery_key, &master_key, None)
            .await
            .expect("store");

        delete_recovery_key(&db, 1).await.expect("delete");

        assert!(!has_recovery_key(&db, 1).await.expect("check"));
    }
}
