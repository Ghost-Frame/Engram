use crate::db::Database;
use crate::Result;
use libsql::params;
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
    let conn = db.connection();
    let ids_json = serde_json::to_string(source_memory_ids).unwrap_or_default();

    conn.execute(
        "INSERT INTO reflections (content, reflection_type, source_memory_ids, confidence, user_id) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![content, reflection_type, ids_json, confidence, user_id],
    ).await?;

    let mut rows = conn.query("SELECT last_insert_rowid()", ()).await?;
    let id: i64 = if let Some(row) = rows.next().await? { row.get(0)? } else { 0 };

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
pub async fn list_reflections(db: &Database, user_id: i64, limit: usize) -> Result<Vec<Reflection>> {
    let conn = db.connection();
    let mut rows = conn.query(
        "SELECT id, content, reflection_type, source_memory_ids, confidence, user_id, created_at \
         FROM reflections WHERE user_id = ?1 ORDER BY id DESC LIMIT ?2",
        params![user_id, limit as i64],
    ).await?;

    let mut results = Vec::new();
    while let Some(row) = rows.next().await? {
        let ids_json: String = row.get::<Option<String>>(3)?.unwrap_or_default();
        let source_memory_ids: Vec<i64> = serde_json::from_str(&ids_json).unwrap_or_default();
        results.push(Reflection {
            id: row.get(0)?,
            content: row.get(1)?,
            reflection_type: row.get(2)?,
            source_memory_ids,
            confidence: row.get(4)?,
            user_id: row.get(5)?,
            created_at: row.get(6)?,
        });
    }
    Ok(results)
}
