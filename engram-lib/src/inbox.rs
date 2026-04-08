//! Inbox -- pending memory management.
//!
//! Ports: inbox/db.ts, inbox/routes.ts (logic)

use serde::{Deserialize, Serialize};
use crate::db::Database;
use crate::Result;

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

pub async fn list_pending(db: &Database, user_id: i64, limit: i64, offset: i64) -> Result<Vec<PendingMemory>> {
    let mut rows = db.conn.query(
        "SELECT id, content, category, source, session_id, importance, created_at, tags, confidence, decay_score, status, model FROM memories WHERE status = 'pending' AND is_forgotten = 0 AND user_id = ?1 ORDER BY created_at DESC LIMIT ?2 OFFSET ?3",
        libsql::params![user_id, limit, offset],
    ).await?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().await? {
        result.push(PendingMemory {
            id: row.get(0).map_err(|e| crate::EngError::Internal(e.to_string()))?,
            content: row.get(1).map_err(|e| crate::EngError::Internal(e.to_string()))?,
            category: row.get(2).map_err(|e| crate::EngError::Internal(e.to_string()))?,
            source: row.get(3).unwrap_or(None),
            session_id: row.get(4).unwrap_or(None),
            importance: row.get(5).map_err(|e| crate::EngError::Internal(e.to_string()))?,
            created_at: row.get(6).map_err(|e| crate::EngError::Internal(e.to_string()))?,
            tags: row.get(7).unwrap_or(None),
            confidence: row.get(8).unwrap_or(None),
            decay_score: row.get(9).unwrap_or(None),
            status: row.get(10).map_err(|e| crate::EngError::Internal(e.to_string()))?,
            model: row.get(11).unwrap_or(None),
        });
    }
    Ok(result)
}

pub async fn count_pending(db: &Database, user_id: i64) -> Result<i64> {
    let mut rows = db.conn.query(
        "SELECT COUNT(*) FROM memories WHERE status = 'pending' AND is_forgotten = 0 AND user_id = ?1",
        libsql::params![user_id],
    ).await?;
    let row = rows.next().await?.ok_or_else(|| crate::EngError::Internal("count query empty".into()))?;
    Ok(row.get::<i64>(0).unwrap_or(0))
}

pub async fn approve_memory(db: &Database, id: i64, user_id: i64) -> Result<()> {
    db.conn.execute(
        "UPDATE memories SET status = 'approved', updated_at = datetime('now') WHERE id = ?1 AND user_id = ?2",
        libsql::params![id, user_id],
    ).await?;
    Ok(())
}

pub async fn reject_memory(db: &Database, id: i64, user_id: i64) -> Result<()> {
    db.conn.execute(
        "UPDATE memories SET status = 'rejected', is_archived = 1, updated_at = datetime('now') WHERE id = ?1 AND user_id = ?2",
        libsql::params![id, user_id],
    ).await?;
    Ok(())
}

pub async fn set_forget_reason(db: &Database, id: i64, reason: &str) -> Result<()> {
    db.conn.execute(
        "UPDATE memories SET forget_reason = ?1 WHERE id = ?2",
        libsql::params![reason.to_string(), id],
    ).await?;
    Ok(())
}

pub async fn edit_and_approve(db: &Database, id: i64, content: Option<&str>, category: Option<&str>, importance: Option<i64>, tags: Option<&str>) -> Result<()> {
    let mut sets = vec!["status = 'approved'".to_string(), "updated_at = datetime('now')".to_string()];
    let mut vals: Vec<libsql::Value> = Vec::new();
    let mut idx = 1;
    if let Some(c) = content {
        sets.push(format!("content = ?{}", idx)); vals.push(c.to_string().into()); idx += 1;
    }
    if let Some(c) = category {
        sets.push(format!("category = ?{}", idx)); vals.push(c.to_string().into()); idx += 1;
    }
    if let Some(i) = importance {
        sets.push(format!("importance = ?{}", idx)); vals.push(i.into()); idx += 1;
    }
    if let Some(t) = tags {
        sets.push(format!("tags = ?{}", idx)); vals.push(t.to_string().into()); idx += 1;
    }
    vals.push(id.into());
    let sql = format!("UPDATE memories SET {} WHERE id = ?{}", sets.join(", "), idx);
    db.conn.execute(&sql, vals).await?;
    Ok(())
}
