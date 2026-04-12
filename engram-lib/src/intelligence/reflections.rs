use crate::db::Database;
use crate::{EngError, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reflection {
    pub id: i64,
    pub content: String,
    pub reflection_type: String,
    pub source_memory_ids: Vec<i64>,
    pub confidence: f64,
    pub user_id: i64,
    pub created_at: String,
}

/// Create a reflection from source memories.
pub async fn create_reflection(
    db: &Database,
    content: &str,
    reflection_type: &str,
    source_memory_ids: &[i64],
    confidence: f64,
    user_id: i64,
) -> Result<Reflection> {
    let ids_json = serde_json::to_string(source_memory_ids).unwrap_or_default();
    let content_owned = content.to_string();
    let reflection_type_owned = reflection_type.to_string();

    let id = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO reflections (content, reflection_type, source_memory_ids, confidence, user_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![content_owned, reflection_type_owned, ids_json, confidence, user_id],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            Ok(conn.last_insert_rowid())
        })
        .await?;

    Ok(Reflection {
        id,
        content: content.into(),
        reflection_type: reflection_type.into(),
        source_memory_ids: source_memory_ids.to_vec(),
        confidence,
        user_id,
        created_at: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    })
}

/// List reflections.
pub async fn list_reflections(
    db: &Database,
    user_id: i64,
    limit: usize,
) -> Result<Vec<Reflection>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, content, reflection_type, source_memory_ids, confidence, user_id, created_at \
                 FROM reflections WHERE user_id = ?1 ORDER BY id DESC LIMIT ?2",
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        let rows = stmt
            .query_map(params![user_id, limit as i64], |row| {
                let ids_json: Option<String> = row.get(3)?;
                let source_memory_ids: Vec<i64> = ids_json
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_default();
                Ok(Reflection {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    reflection_type: row.get(2)?,
                    source_memory_ids,
                    confidence: row.get(4)?,
                    user_id: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))
    })
    .await
}
