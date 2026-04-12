use crate::db::Database;
use crate::{EngError, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

// -- Types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPreference {
    pub id: i64,
    pub user_id: i64,
    pub key: String,
    pub value: String,
    pub created_at: String,
    pub updated_at: String,
}

// -- Constants ---

const PREF_COLUMNS: &str = "id, user_id, key, value, created_at, updated_at";

// -- Helpers ---

fn row_to_preference(row: &rusqlite::Row<'_>) -> rusqlite::Result<UserPreference> {
    Ok(UserPreference {
        id: row.get(0)?,
        user_id: row.get(1)?,
        key: row.get(2)?,
        value: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

// -- Public CRUD functions ---

/// Upsert a preference for the given user/key pair.
pub async fn set_preference(
    db: &Database,
    user_id: i64,
    key: &str,
    value: &str,
) -> Result<UserPreference> {
    let key_owned = key.to_string();
    let value_owned = value.to_string();
    let key_for_get = key_owned.clone();
    db.write(move |conn| {
        conn.execute(
            "INSERT INTO user_preferences (user_id, key, value) \
             VALUES (?1, ?2, ?3) \
             ON CONFLICT(user_id, key) DO UPDATE SET \
                 value = excluded.value, \
                 updated_at = datetime('now')",
            params![user_id, key_owned, value_owned],
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        Ok(())
    })
    .await?;

    get_preference(db, user_id, &key_for_get).await
}

/// Fetch a single preference by user/key. Returns NotFound if absent.
pub async fn get_preference(db: &Database, user_id: i64, key: &str) -> Result<UserPreference> {
    let key = key.to_string();
    let sql = format!(
        "SELECT {} FROM user_preferences WHERE user_id = ?1 AND key = ?2",
        PREF_COLUMNS
    );
    db.read(move |conn| {
        conn.query_row(&sql, params![user_id, key], |row| row_to_preference(row))
            .optional()
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
            .ok_or_else(|| {
                EngError::NotFound(format!("preference not found for user {}", user_id))
            })
    })
    .await
}

/// List all preferences for a user, ordered by key.
pub async fn list_preferences(db: &Database, user_id: i64) -> Result<Vec<UserPreference>> {
    let sql = format!(
        "SELECT {} FROM user_preferences WHERE user_id = ?1 ORDER BY key ASC",
        PREF_COLUMNS
    );
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        let rows = stmt
            .query_map(params![user_id], |row| row_to_preference(row))
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        let mut prefs = Vec::new();
        for row in rows {
            prefs.push(row.map_err(|e| EngError::DatabaseMessage(e.to_string()))?);
        }
        Ok(prefs)
    })
    .await
}

/// Delete all preferences for a user. Returns count deleted.
pub async fn delete_all_preferences(db: &Database, user_id: i64) -> Result<u64> {
    let affected = db
        .write(move |conn| {
            conn.execute(
                "DELETE FROM user_preferences WHERE user_id = ?1",
                params![user_id],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))
        })
        .await?;
    Ok(affected as u64)
}

/// Delete a preference by user/key. Returns NotFound if it does not exist.
pub async fn delete_preference(db: &Database, user_id: i64, key: &str) -> Result<()> {
    let key = key.to_string();
    let affected = db
        .write(move |conn| {
            conn.execute(
                "DELETE FROM user_preferences WHERE user_id = ?1 AND key = ?2",
                params![user_id, key],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))
        })
        .await?;

    if affected == 0 {
        return Err(EngError::NotFound(format!(
            "preference not found for user {}",
            user_id
        )));
    }
    Ok(())
}
