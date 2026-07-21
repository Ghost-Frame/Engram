//! Inbox -- pending memory management.
//!
//! Ports: inbox/db.ts, inbox/routes.ts (logic)

use crate::db::Database;
use crate::Result;
use serde::{Deserialize, Serialize};

/// A memory row awaiting user approval/rejection in the inbox.
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

/// Lists pending, non-forgotten memories for a user, newest first, paginated
/// by `limit`/`offset`.
#[tracing::instrument(skip(db), fields(user_id, limit, offset))]
pub async fn list_pending(
    db: &Database,
    user_id: i64,
    limit: i64,
    offset: i64,
) -> Result<Vec<PendingMemory>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, content, category, source, session_id, importance, created_at, tags, confidence, decay_score, status, model \
                 FROM memories \
                 WHERE status = 'pending' AND is_forgotten = 0 AND user_id = ?1 \
                 ORDER BY created_at DESC LIMIT ?2 OFFSET ?3",
            )
            ?;

        let rows = stmt
            .query_map(rusqlite::params![user_id, limit, offset], |row| {
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
            ?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    })
    .await
}

/// Counts pending, non-forgotten memories for a user.
#[tracing::instrument(skip(db))]
pub async fn count_pending(db: &Database, user_id: i64) -> Result<i64> {
    db.read(move |conn| {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM memories \
             WHERE status = 'pending' AND is_forgotten = 0 AND user_id = ?1",
            rusqlite::params![user_id],
            |row| row.get::<_, i64>(0),
        )?)
    })
    .await
}

/// Approve a pending memory. The `status = 'pending'` predicate makes the
/// transition state-checked and the affected-row count observable: approving a
/// missing, foreign, or already-decided row returns NotFound instead of a
/// silent no-op Ok (finding [73]).
#[tracing::instrument(skip(db))]
pub async fn approve_memory(db: &Database, id: i64, user_id: i64) -> Result<()> {
    let affected = db
        .write(move |conn| {
            Ok(conn.execute(
                "UPDATE memories SET status = 'approved', updated_at = datetime('now') \
                 WHERE id = ?1 AND user_id = ?2 AND status = 'pending'",
                rusqlite::params![id, user_id],
            )?)
        })
        .await?;
    if affected == 0 {
        return Err(crate::EngError::NotFound(format!(
            "no pending memory {} for this user",
            id
        )));
    }
    Ok(())
}

/// Reject a pending memory (archives it). Same state-checked contract as
/// [`approve_memory`]: only pending rows are eligible, and a zero-row update
/// surfaces as NotFound rather than silent success (finding [73]).
#[tracing::instrument(skip(db), fields(memory_id = id, user_id))]
pub async fn reject_memory(db: &Database, id: i64, user_id: i64) -> Result<()> {
    let affected = db
        .write(move |conn| {
            Ok(conn.execute(
                "UPDATE memories SET status = 'rejected', is_archived = 1, updated_at = datetime('now') \
                 WHERE id = ?1 AND user_id = ?2 AND status = 'pending'",
                rusqlite::params![id, user_id],
            )?)
        })
        .await?;
    if affected == 0 {
        return Err(crate::EngError::NotFound(format!(
            "no pending memory {} for this user",
            id
        )));
    }
    Ok(())
}

/// Records the reason a memory was forgotten/rejected, scoped to the owning user.
#[tracing::instrument(skip(db, reason), fields(memory_id = id, user_id))]
pub async fn set_forget_reason(db: &Database, id: i64, user_id: i64, reason: &str) -> Result<()> {
    let reason = reason.to_string();
    db.write(move |conn| {
        conn.execute(
            "UPDATE memories SET forget_reason = ?1 WHERE id = ?2 AND user_id = ?3",
            rusqlite::params![reason, id, user_id],
        )?;
        Ok(())
    })
    .await
}

/// Edit fields of a pending memory and approve it in one write.
///
/// Every supplied field passes through the same validation the generic
/// memory-write paths use (finding [72]): content via
/// `validation::validate_content`, importance clamped to the supported range,
/// and tags parsed + normalized into the canonical JSON-array form so the
/// column never holds raw non-JSON text. Only pending rows are eligible and a
/// zero-row update returns NotFound (finding [73]).
#[tracing::instrument(skip(db, content, tags), fields(memory_id = id, user_id, category = ?category, importance = ?importance))]
pub async fn edit_and_approve(
    db: &Database,
    id: i64,
    user_id: i64,
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
        crate::validation::validate_content(c)?;
        sets.push(format!("content = ?{}", idx));
        vals.push(rusqlite::types::Value::Text(c.trim().to_string()));
        idx += 1;
    }
    if let Some(c) = category {
        let trimmed = c.trim();
        if trimmed.is_empty() {
            return Err(crate::EngError::InvalidInput(
                "category must not be empty".to_string(),
            ));
        }
        sets.push(format!("category = ?{}", idx));
        vals.push(rusqlite::types::Value::Text(trimmed.to_string()));
        idx += 1;
    }
    if let Some(i) = importance {
        sets.push(format!("importance = ?{}", idx));
        vals.push(rusqlite::types::Value::Integer(
            crate::validation::clamp_importance_i64(i),
        ));
        idx += 1;
    }
    if let Some(t) = tags {
        // NULL out the column when normalization leaves no usable tags,
        // matching memory::store's normalize_tags behavior.
        match crate::validation::normalize_tags_json(t)? {
            Some(json) => {
                sets.push(format!("tags = ?{}", idx));
                vals.push(rusqlite::types::Value::Text(json));
                idx += 1;
            }
            None => {
                sets.push("tags = NULL".to_string());
            }
        }
    }
    let id_idx = idx;
    vals.push(rusqlite::types::Value::Integer(id));
    let user_idx = idx + 1;
    vals.push(rusqlite::types::Value::Integer(user_id));
    let sql = format!(
        "UPDATE memories SET {} WHERE id = ?{} AND user_id = ?{} AND status = 'pending'",
        sets.join(", "),
        id_idx,
        user_idx
    );
    let affected = db
        .write(
            move |conn| Ok(conn.execute(&sql, rusqlite::params_from_iter(vals.iter().cloned()))?),
        )
        .await?;
    if affected == 0 {
        return Err(crate::EngError::NotFound(format!(
            "no pending memory {} for this user",
            id
        )));
    }
    Ok(())
}
