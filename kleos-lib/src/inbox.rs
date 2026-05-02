//! Inbox -- pending memory management.
//!
//! Ports: inbox/db.ts, inbox/routes.ts (logic)

use crate::db::Database;
use crate::{EngError, Result};
use serde::{Deserialize, Serialize};

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingMemory {
    pub id: i64,
    pub content: String,
    pub category: String,
    pub source: Option<String>,
    pub session_id: Option<String>,
    pub importance: i64,
    pub created_at: String,
    pub tags: Option<String>,
    pub confidence: Option<f64>,
    pub decay_score: Option<f64>,
    pub status: String,
    pub model: Option<String>,
}

#[tracing::instrument(skip(db), fields(limit, offset))]
pub async fn list_pending(db: &Database, limit: i64, offset: i64) -> Result<Vec<PendingMemory>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, content, category, source, session_id, importance, created_at, tags, confidence, decay_score, status, model \
                 FROM memories \
                 WHERE status = 'pending' AND is_forgotten = 0 \
                 ORDER BY created_at DESC LIMIT ?1 OFFSET ?2",
            )
            .map_err(rusqlite_to_eng_error)?;

        let rows = stmt
            .query_map(rusqlite::params![limit, offset], |row| {
                Ok(PendingMemory {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    category: row.get(2)?,
                    source: row.get(3)?,
                    session_id: row.get(4)?,
                    importance: row.get(5)?,
                    created_at: row.get(6)?,
                    tags: row.get(7)?,
                    confidence: row.get(8)?,
                    decay_score: row.get(9)?,
                    status: row.get(10)?,
                    model: row.get(11)?,
                })
            })
            .map_err(rusqlite_to_eng_error)?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(rusqlite_to_eng_error)?);
        }
        Ok(result)
    })
    .await
}

#[tracing::instrument(skip(db))]
pub async fn count_pending(db: &Database, user_id: i64) -> Result<i64> {
    db.read(move |conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM memories WHERE status = 'pending' AND is_forgotten = 0",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(rusqlite_to_eng_error)
    })
    .await
}

#[tracing::instrument(skip(db))]
pub async fn approve_memory(db: &Database, id: i64, user_id: i64) -> Result<()> {
    db.write(move |conn| {
        conn.execute(
            "UPDATE memories SET status = 'approved', updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

#[tracing::instrument(skip(db), fields(memory_id = id))]
pub async fn reject_memory(db: &Database, id: i64) -> Result<()> {
    db.write(move |conn| {
        conn.execute(
            "UPDATE memories SET status = 'rejected', is_archived = 1, updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

#[tracing::instrument(skip(db, reason), fields(memory_id = id))]
pub async fn set_forget_reason(db: &Database, id: i64, reason: &str) -> Result<()> {
    let reason = reason.to_string();
    db.write(move |conn| {
        conn.execute(
            "UPDATE memories SET forget_reason = ?1 WHERE id = ?2",
            rusqlite::params![reason, id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

#[tracing::instrument(skip(db, content, tags), fields(memory_id = id, category = ?category, importance = ?importance))]
pub async fn edit_and_approve(
    db: &Database,
    id: i64,
    content: Option<&str>,
    category: Option<&str>,
    importance: Option<i64>,
    tags: Option<&str>,
) -> Result<()> {
    let mut sets = vec![
        "status = 'approved'".to_string(),
        "updated_at = datetime('now')".to_string(),
    ];
    let mut vals: Vec<rusqlite::types::Value> = Vec::new();
    let mut idx = 1;
    if let Some(c) = content {
        sets.push(format!("content = ?{}", idx));
        vals.push(rusqlite::types::Value::Text(c.to_string()));
        idx += 1;
    }
    if let Some(c) = category {
        sets.push(format!("category = ?{}", idx));
        vals.push(rusqlite::types::Value::Text(c.to_string()));
        idx += 1;
    }
    if let Some(i) = importance {
        sets.push(format!("importance = ?{}", idx));
        vals.push(rusqlite::types::Value::Integer(i));
        idx += 1;
    }
    if let Some(t) = tags {
        sets.push(format!("tags = ?{}", idx));
        vals.push(rusqlite::types::Value::Text(t.to_string()));
        idx += 1;
    }
    vals.push(rusqlite::types::Value::Integer(id));
    let sql = format!(
        "UPDATE memories SET {} WHERE id = ?{}",
        sets.join(", "),
        idx
    );
    db.write(move |conn| {
        conn.execute(&sql, rusqlite::params_from_iter(vals.iter().cloned()))
            .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}
