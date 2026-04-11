//! Database storage layer for encrypted secrets.

use engram_lib::db::Database;

use crate::crypto::{decrypt_secret, encrypt_secret, KEY_SIZE, NONCE_SIZE};
use crate::types::{SecretData, SecretType};
use crate::{CredError, Result};

/// A stored secret row from the database.
#[derive(Debug, Clone)]
pub struct SecretRow {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub category: String,
    pub secret_type: SecretType,
    pub created_at: String,
    pub updated_at: String,
}

/// Store a new secret in the database.
pub async fn store_secret(
    db: &Database,
    user_id: i64,
    category: &str,
    name: &str,
    data: &SecretData,
    key: &[u8; KEY_SIZE],
) -> Result<i64> {
    let (encrypted, nonce) = encrypt_secret(key, data)?;
    let secret_type = data.secret_type().as_str();
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

    db.conn
        .execute(
            "INSERT INTO cred_secrets (user_id, name, category, secret_type, encrypted_data, nonce, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            libsql::params![
                user_id,
                name,
                category,
                secret_type,
                encrypted.as_slice(),
                nonce.as_slice(),
                now.clone(),
                now
            ],
        )
        .await?;

    let mut rows = db.conn.query("SELECT last_insert_rowid()", ()).await?;
    let id: i64 = match rows.next().await? {
        Some(row) => row.get(0)?,
        None => 0,
    };

    Ok(id)
}

/// Get a secret by category and name.
pub async fn get_secret(
    db: &Database,
    user_id: i64,
    category: &str,
    name: &str,
    key: &[u8; KEY_SIZE],
) -> Result<(SecretRow, SecretData)> {
    let mut rows = db
        .conn
        .query(
            "SELECT id, user_id, name, category, secret_type, encrypted_data, nonce, created_at, updated_at
             FROM cred_secrets
             WHERE user_id = ?1 AND category = ?2 AND name = ?3",
            libsql::params![user_id, category, name],
        )
        .await?;

    let row = rows
        .next()
        .await?
        .ok_or_else(|| CredError::NotFound(format!("{}/{}", category, name)))?;

    let id: i64 = row.get(0)?;
    let user_id: i64 = row.get(1)?;
    let name: String = row.get(2)?;
    let category: String = row.get(3)?;
    let secret_type_str: String = row.get(4)?;
    let encrypted_data: Vec<u8> = row.get(5)?;
    let nonce_vec: Vec<u8> = row.get(6)?;
    let created_at: String = row.get(7)?;
    let updated_at: String = row.get(8)?;

    let secret_type = SecretType::from_str(&secret_type_str)
        .ok_or_else(|| CredError::InvalidInput(format!("unknown secret type: {}", secret_type_str)))?;

    let mut nonce = [0u8; NONCE_SIZE];
    if nonce_vec.len() != NONCE_SIZE {
        return Err(CredError::Decryption("invalid nonce length".into()));
    }
    nonce.copy_from_slice(&nonce_vec);

    let data = decrypt_secret(key, &encrypted_data, &nonce)?;

    let secret_row = SecretRow {
        id,
        user_id,
        name,
        category,
        secret_type,
        created_at,
        updated_at,
    };

    Ok((secret_row, data))
}

/// List secrets for a user, optionally filtered by category.
pub async fn list_secrets(
    db: &Database,
    user_id: i64,
    category: Option<&str>,
) -> Result<Vec<SecretRow>> {
    let mut rows = match category {
        Some(cat) => {
            db.conn
                .query(
                    "SELECT id, user_id, name, category, secret_type, created_at, updated_at
                     FROM cred_secrets
                     WHERE user_id = ?1 AND category = ?2
                     ORDER BY category, name",
                    libsql::params![user_id, cat],
                )
                .await?
        }
        None => {
            db.conn
                .query(
                    "SELECT id, user_id, name, category, secret_type, created_at, updated_at
                     FROM cred_secrets
                     WHERE user_id = ?1
                     ORDER BY category, name",
                    libsql::params![user_id],
                )
                .await?
        }
    };

    let mut secrets = Vec::new();
    while let Some(row) = rows.next().await? {
        let id: i64 = row.get(0)?;
        let user_id: i64 = row.get(1)?;
        let name: String = row.get(2)?;
        let category: String = row.get(3)?;
        let secret_type_str: String = row.get(4)?;
        let created_at: String = row.get(5)?;
        let updated_at: String = row.get(6)?;

        let secret_type = SecretType::from_str(&secret_type_str).unwrap_or(SecretType::Note);

        secrets.push(SecretRow {
            id,
            user_id,
            name,
            category,
            secret_type,
            created_at,
            updated_at,
        });
    }

    Ok(secrets)
}

/// Update an existing secret.
pub async fn update_secret(
    db: &Database,
    user_id: i64,
    category: &str,
    name: &str,
    data: &SecretData,
    key: &[u8; KEY_SIZE],
) -> Result<()> {
    let (encrypted, nonce) = encrypt_secret(key, data)?;
    let secret_type = data.secret_type().as_str();
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

    let affected = db
        .conn
        .execute(
            "UPDATE cred_secrets
             SET encrypted_data = ?1, nonce = ?2, secret_type = ?3, updated_at = ?4
             WHERE user_id = ?5 AND category = ?6 AND name = ?7",
            libsql::params![
                encrypted.as_slice(),
                nonce.as_slice(),
                secret_type,
                now,
                user_id,
                category,
                name
            ],
        )
        .await?;

    if affected == 0 {
        return Err(CredError::NotFound(format!("{}/{}", category, name)));
    }

    Ok(())
}

/// Delete a secret.
pub async fn delete_secret(
    db: &Database,
    user_id: i64,
    category: &str,
    name: &str,
) -> Result<()> {
    let affected = db
        .conn
        .execute(
            "DELETE FROM cred_secrets WHERE user_id = ?1 AND category = ?2 AND name = ?3",
            libsql::params![user_id, category, name],
        )
        .await?;

    if affected == 0 {
        return Err(CredError::NotFound(format!("{}/{}", category, name)));
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
                "CREATE TABLE IF NOT EXISTS cred_secrets (
                    id INTEGER PRIMARY KEY,
                    user_id INTEGER NOT NULL,
                    name TEXT NOT NULL,
                    category TEXT NOT NULL,
                    secret_type TEXT NOT NULL,
                    encrypted_data BLOB NOT NULL,
                    nonce BLOB NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    UNIQUE(user_id, category, name)
                )",
                (),
            )
            .await
            .expect("create table");
        db
    }

    #[tokio::test]
    async fn store_and_get_secret() {
        let db = setup_db().await;
        let key = derive_key(1, b"password", None);
        let data = SecretData::ApiKey {
            key: "my-api-key".into(),
            endpoint: Some("https://api.example.com".into()),
            notes: None,
        };

        let id = store_secret(&db, 1, "service", "api-key", &data, &key)
            .await
            .expect("store");
        assert!(id > 0);

        let (row, retrieved) = get_secret(&db, 1, "service", "api-key", &key)
            .await
            .expect("get");
        assert_eq!(row.name, "api-key");
        assert_eq!(row.category, "service");
        assert_eq!(row.secret_type, SecretType::ApiKey);

        match retrieved {
            SecretData::ApiKey { key, endpoint, .. } => {
                assert_eq!(key, "my-api-key");
                assert_eq!(endpoint, Some("https://api.example.com".into()));
            }
            _ => panic!("wrong type"),
        }
    }

    #[tokio::test]
    async fn list_secrets_filters_by_category() {
        let db = setup_db().await;
        let key = derive_key(1, b"password", None);

        store_secret(
            &db,
            1,
            "aws",
            "prod-key",
            &SecretData::ApiKey {
                key: "k1".into(),
                endpoint: None,
                notes: None,
            },
            &key,
        )
        .await
        .expect("store 1");

        store_secret(
            &db,
            1,
            "gcp",
            "dev-key",
            &SecretData::ApiKey {
                key: "k2".into(),
                endpoint: None,
                notes: None,
            },
            &key,
        )
        .await
        .expect("store 2");

        let all = list_secrets(&db, 1, None).await.expect("list all");
        assert_eq!(all.len(), 2);

        let aws_only = list_secrets(&db, 1, Some("aws")).await.expect("list aws");
        assert_eq!(aws_only.len(), 1);
        assert_eq!(aws_only[0].name, "prod-key");
    }

    #[tokio::test]
    async fn update_secret_changes_data() {
        let db = setup_db().await;
        let key = derive_key(1, b"password", None);

        store_secret(
            &db,
            1,
            "svc",
            "token",
            &SecretData::ApiKey {
                key: "old-key".into(),
                endpoint: None,
                notes: None,
            },
            &key,
        )
        .await
        .expect("store");

        update_secret(
            &db,
            1,
            "svc",
            "token",
            &SecretData::ApiKey {
                key: "new-key".into(),
                endpoint: Some("https://new.api".into()),
                notes: None,
            },
            &key,
        )
        .await
        .expect("update");

        let (_, data) = get_secret(&db, 1, "svc", "token", &key).await.expect("get");
        match data {
            SecretData::ApiKey { key, endpoint, .. } => {
                assert_eq!(key, "new-key");
                assert_eq!(endpoint, Some("https://new.api".into()));
            }
            _ => panic!("wrong type"),
        }
    }

    #[tokio::test]
    async fn delete_secret_removes_row() {
        let db = setup_db().await;
        let key = derive_key(1, b"password", None);

        store_secret(
            &db,
            1,
            "svc",
            "key",
            &SecretData::Note {
                content: "test".into(),
            },
            &key,
        )
        .await
        .expect("store");

        delete_secret(&db, 1, "svc", "key").await.expect("delete");

        let result = get_secret(&db, 1, "svc", "key", &key).await;
        assert!(matches!(result, Err(CredError::NotFound(_))));
    }

    #[tokio::test]
    async fn wrong_key_fails_get() {
        let db = setup_db().await;
        let key1 = derive_key(1, b"correct", None);
        let key2 = derive_key(1, b"wrong", None);

        store_secret(
            &db,
            1,
            "svc",
            "secret",
            &SecretData::Note {
                content: "hidden".into(),
            },
            &key1,
        )
        .await
        .expect("store");

        let result = get_secret(&db, 1, "svc", "secret", &key2).await;
        assert!(matches!(result, Err(CredError::Decryption(_))));
    }
}
