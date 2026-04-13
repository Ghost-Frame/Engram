//! Database storage layer for encrypted secrets.

use engram_lib::db::Database;
use engram_lib::EngError;

use crate::crypto::{decrypt_secret, encrypt_secret, KEY_SIZE, NONCE_SIZE};
use crate::types::{SecretData, SecretType};
use crate::{CredError, Result};

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

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
    let secret_type = data.secret_type().as_str().to_string();
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let name = name.to_string();
    let category = category.to_string();

    let id = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO cred_secrets (user_id, name, category, secret_type, encrypted_data, nonce, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
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
            .map_err(rusqlite_to_eng_error)?;

            Ok(conn.last_insert_rowid())
        })
        .await
        .map_err(|e| CredError::Database(e.to_string()))?;

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
    let category = category.to_string();
    let name = name.to_string();
    let category_name = format!("{}/{}", category, name);

    type RawRow = (i64, i64, String, String, String, Vec<u8>, Vec<u8>, String, String);

    let raw: Option<RawRow> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, user_id, name, category, secret_type, encrypted_data, nonce, created_at, updated_at
                     FROM cred_secrets
                     WHERE user_id = ?1 AND category = ?2 AND name = ?3",
                )
                .map_err(rusqlite_to_eng_error)?;

            let mut rows = stmt
                .query(rusqlite::params![user_id, category, name])
                .map_err(rusqlite_to_eng_error)?;

            match rows.next().map_err(rusqlite_to_eng_error)? {
                Some(row) => {
                    let id: i64 = row.get(0).map_err(rusqlite_to_eng_error)?;
                    let uid: i64 = row.get(1).map_err(rusqlite_to_eng_error)?;
                    let rname: String = row.get(2).map_err(rusqlite_to_eng_error)?;
                    let rcat: String = row.get(3).map_err(rusqlite_to_eng_error)?;
                    let stype: String = row.get(4).map_err(rusqlite_to_eng_error)?;
                    let enc: Vec<u8> = row.get(5).map_err(rusqlite_to_eng_error)?;
                    let nonce: Vec<u8> = row.get(6).map_err(rusqlite_to_eng_error)?;
                    let created: String = row.get(7).map_err(rusqlite_to_eng_error)?;
                    let updated: String = row.get(8).map_err(rusqlite_to_eng_error)?;
                    Ok(Some((id, uid, rname, rcat, stype, enc, nonce, created, updated)))
                }
                None => Ok(None),
            }
        })
        .await
        .map_err(|e| CredError::Database(e.to_string()))?;

    let (id, uid, rname, rcat, secret_type_str, encrypted_data, nonce_vec, created_at, updated_at) =
        raw.ok_or(CredError::NotFound(category_name))?;

    let secret_type = SecretType::parse(&secret_type_str).ok_or_else(|| {
        CredError::InvalidInput(format!("unknown secret type: {}", secret_type_str))
    })?;

    let mut nonce = [0u8; NONCE_SIZE];
    if nonce_vec.len() != NONCE_SIZE {
        return Err(CredError::Decryption("invalid nonce length".into()));
    }
    nonce.copy_from_slice(&nonce_vec);

    let data = decrypt_secret(key, &encrypted_data, &nonce)?;

    let secret_row = SecretRow {
        id,
        user_id: uid,
        name: rname,
        category: rcat,
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
    type RawRow = (i64, i64, String, String, String, String, String);

    let category = category.map(|s| s.to_string());

    let rows: Vec<RawRow> = db
        .read(move |conn| {
            let (sql, with_cat) = match &category {
                Some(_) => (
                    "SELECT id, user_id, name, category, secret_type, created_at, updated_at
                     FROM cred_secrets
                     WHERE user_id = ?1 AND category = ?2
                     ORDER BY category, name",
                    true,
                ),
                None => (
                    "SELECT id, user_id, name, category, secret_type, created_at, updated_at
                     FROM cred_secrets
                     WHERE user_id = ?1
                     ORDER BY category, name",
                    false,
                ),
            };

            let mut stmt = conn.prepare(sql).map_err(rusqlite_to_eng_error)?;

            #[allow(clippy::type_complexity)]
            fn map_row(row: &rusqlite::Row<'_>) -> std::result::Result<(i64, i64, String, String, String, String, String), rusqlite::Error> {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            }

            if with_cat {
                stmt.query_map(rusqlite::params![user_id, category.as_deref()], map_row)
                    .and_then(|rows| rows.collect::<std::result::Result<Vec<_>, _>>())
                    .map_err(rusqlite_to_eng_error)
            } else {
                stmt.query_map(rusqlite::params![user_id], map_row)
                    .and_then(|rows| rows.collect::<std::result::Result<Vec<_>, _>>())
                    .map_err(rusqlite_to_eng_error)
            }
        })
        .await
        .map_err(|e| CredError::Database(e.to_string()))?;

    let secrets = rows
        .into_iter()
        .map(|(id, uid, name, cat, stype_str, created, updated)| {
            let secret_type = SecretType::parse(&stype_str).unwrap_or(SecretType::Note);
            SecretRow {
                id,
                user_id: uid,
                name,
                category: cat,
                secret_type,
                created_at: created,
                updated_at: updated,
            }
        })
        .collect();

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
    let secret_type = data.secret_type().as_str().to_string();
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let category = category.to_string();
    let name = name.to_string();
    let category_name = format!("{}/{}", category, name);

    let affected = db
        .write(move |conn| {
            let n = conn
                .execute(
                    "UPDATE cred_secrets
                     SET encrypted_data = ?1, nonce = ?2, secret_type = ?3, updated_at = ?4
                     WHERE user_id = ?5 AND category = ?6 AND name = ?7",
                    rusqlite::params![
                        encrypted.as_slice(),
                        nonce.as_slice(),
                        secret_type,
                        now,
                        user_id,
                        category,
                        name
                    ],
                )
                .map_err(rusqlite_to_eng_error)?;
            Ok(n)
        })
        .await
        .map_err(|e| CredError::Database(e.to_string()))?;

    if affected == 0 {
        return Err(CredError::NotFound(category_name));
    }

    Ok(())
}

/// Delete a secret.
pub async fn delete_secret(db: &Database, user_id: i64, category: &str, name: &str) -> Result<()> {
    let category = category.to_string();
    let name = name.to_string();
    let category_name = format!("{}/{}", category, name);

    let affected = db
        .write(move |conn| {
            let n = conn
                .execute(
                    "DELETE FROM cred_secrets WHERE user_id = ?1 AND category = ?2 AND name = ?3",
                    rusqlite::params![user_id, category, name],
                )
                .map_err(rusqlite_to_eng_error)?;
            Ok(n)
        })
        .await
        .map_err(|e| CredError::Database(e.to_string()))?;

    if affected == 0 {
        return Err(CredError::NotFound(category_name));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::derive_key;

    async fn setup_db() -> Database {
        let db = Database::connect_memory().await.expect("db");
        db.write(|conn| {
            conn.execute_batch(
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
                );",
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))
        })
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
