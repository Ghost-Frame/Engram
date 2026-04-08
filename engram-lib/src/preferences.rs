use crate::db::Database;
use crate::{EngError, Result};
use libsql::params;
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

fn row_to_preference(row: &libsql::Row) -> Result<UserPreference> {
    Ok(UserPreference {
        id: row.get::<i64>(0)?,
        user_id: row.get::<i64>(1)?,
        key: row.get::<String>(2)?,
        value: row.get::<String>(3)?,
        created_at: row.get::<String>(4)?,
        updated_at: row.get::<String>(5)?,
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
    db.conn
        .execute(
            "INSERT INTO user_preferences (user_id, key, value) \
             VALUES (?1, ?2, ?3) \
             ON CONFLICT(user_id, key) DO UPDATE SET \
                 value = excluded.value, \
                 updated_at = datetime('now')",
            params![user_id, key, value],
        )
        .await?;

    get_preference(db, user_id, key).await
}

/// Fetch a single preference by user/key. Returns NotFound if absent.
pub async fn get_preference(
    db: &Database,
    user_id: i64,
    key: &str,
) -> Result<UserPreference> {
    let sql = format!(
        "SELECT {} FROM user_preferences WHERE user_id = ?1 AND key = ?2",
        PREF_COLUMNS
    );
    let mut rows = db.conn.query(&sql, params![user_id, key]).await?;
    if let Some(row) = rows.next().await? {
        row_to_preference(&row)
    } else {
        Err(EngError::NotFound(format!(
            "preference '{}' not found for user {}",
            key, user_id
        )))
    }
}

/// List all preferences for a user, ordered by key.
pub async fn list_preferences(db: &Database, user_id: i64) -> Result<Vec<UserPreference>> {
    let sql = format!(
        "SELECT {} FROM user_preferences WHERE user_id = ?1 ORDER BY key ASC",
        PREF_COLUMNS
    );
    let mut rows = db.conn.query(&sql, params![user_id]).await?;
    let mut prefs = Vec::new();
    while let Some(row) = rows.next().await? {
        prefs.push(row_to_preference(&row)?);
    }
    Ok(prefs)
}

/// Delete a preference by user/key. Returns NotFound if it does not exist.
pub async fn delete_preference(db: &Database, user_id: i64, key: &str) -> Result<()> {
    let affected = db
        .conn
        .execute(
            "DELETE FROM user_preferences WHERE user_id = ?1 AND key = ?2",
            params![user_id, key],
        )
        .await?;

    if affected == 0 {
        return Err(EngError::NotFound(format!(
            "preference '{}' not found for user {}",
            key, user_id
        )));
    }
    Ok(())
}
