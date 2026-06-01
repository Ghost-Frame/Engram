//! Per-secret SSH key settings.

use kleos_cred::{CredError, Result};
use kleos_lib::db::Database;
use rusqlite::params;

/// SSH key settings for a specific secret.
#[derive(Debug, Clone)]
pub struct SshSettings {
    /// Row ID.
    pub id: i64,
    /// Owner user ID.
    pub user_id: i64,
    /// Secret category.
    pub category: String,
    /// Secret name.
    pub secret_name: String,
    /// Whether to sign requests without approval.
    pub auto_sign: bool,
    /// Whether to load key into SSH agent on startup.
    pub auto_load: bool,
    /// When settings were created.
    pub created_at: String,
    /// When settings were last updated.
    pub updated_at: String,
}

/// Serializable settings for JSON responses.
impl SshSettings {
    /// Convert to a serde_json::Value for API responses.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "category": self.category,
            "secret_name": self.secret_name,
            "auto_sign": self.auto_sign,
            "auto_load": self.auto_load,
            "created_at": self.created_at,
            "updated_at": self.updated_at,
        })
    }
}

/// Get SSH settings for a specific secret, if any exist.
pub async fn get_ssh_settings(
    db: &Database,
    user_id: i64,
    category: &str,
    secret_name: &str,
) -> Result<Option<SshSettings>> {
    let cat = category.to_string();
    let sec = secret_name.to_string();

    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT id, user_id, category, secret_name, auto_sign, auto_load,
                    created_at, updated_at
             FROM phylax_ssh_settings
             WHERE user_id = ?1 AND category = ?2 AND secret_name = ?3",
        )?;
        let settings = stmt
            .query_row(params![user_id, cat, sec], row_to_settings)
            .ok();
        Ok(settings)
    })
    .await
    .map_err(|e| CredError::Database(e.to_string()))
}

/// List all SSH settings rows for a user, ordered by category then name.
pub async fn list_ssh_settings(db: &Database, user_id: i64) -> Result<Vec<SshSettings>> {
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT id, user_id, category, secret_name, auto_sign, auto_load,
                    created_at, updated_at
             FROM phylax_ssh_settings
             WHERE user_id = ?1
             ORDER BY category, secret_name",
        )?;
        let rows = stmt
            .query_map(params![user_id], row_to_settings)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
    .await
    .map_err(|e| CredError::Database(e.to_string()))
}

/// Create or update SSH settings (upsert).
pub async fn upsert_ssh_settings(
    db: &Database,
    user_id: i64,
    category: &str,
    secret_name: &str,
    auto_sign: bool,
    auto_load: bool,
) -> Result<SshSettings> {
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let cat = category.to_string();
    let sec = secret_name.to_string();
    let now2 = now.clone();

    let id = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO phylax_ssh_settings
                 (user_id, category, secret_name, auto_sign, auto_load, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
                 ON CONFLICT(user_id, category, secret_name)
                 DO UPDATE SET auto_sign = ?4, auto_load = ?5, updated_at = ?6",
                params![user_id, cat, sec, auto_sign as i32, auto_load as i32, now2],
            )?;
            Ok(conn.last_insert_rowid())
        })
        .await
        .map_err(|e| CredError::Database(e.to_string()))?;

    Ok(SshSettings {
        id,
        user_id,
        category: category.to_string(),
        secret_name: secret_name.to_string(),
        auto_sign,
        auto_load,
        created_at: now.clone(),
        updated_at: now,
    })
}

/// Parse a database row into an SshSettings struct.
fn row_to_settings(row: &rusqlite::Row<'_>) -> rusqlite::Result<SshSettings> {
    Ok(SshSettings {
        id: row.get(0)?,
        user_id: row.get(1)?,
        category: row.get(2)?,
        secret_name: row.get(3)?,
        auto_sign: row.get::<_, i32>(4)? != 0,
        auto_load: row.get::<_, i32>(5)? != 0,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}
