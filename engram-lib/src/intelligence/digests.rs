use crate::db::Database;
use crate::{EngError, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Digest {
    pub id: i64,
    pub period: String,
    pub content: String,
    pub memory_count: i32,
    pub user_id: i64,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub created_at: String,
}

/// Generate a digest summarizing recent memory activity.
pub async fn generate_digest(db: &Database, user_id: i64, period: &str) -> Result<Digest> {
    let interval = match period {
        "daily" => "-1 day",
        "weekly" => "-7 days",
        "monthly" => "-30 days",
        _ => "-1 day",
    };

    let interval_owned = interval.to_string();
    let period_owned = period.to_string();

    // Fetch recent memories in the period
    let (summaries, count) = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, content, category, importance FROM memories \
                     WHERE user_id = ?1 AND is_forgotten = 0 AND created_at >= datetime('now', ?2) \
                     ORDER BY importance DESC LIMIT 50",
                )
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            let rows = stmt
                .query_map(params![user_id, interval_owned], |row| {
                    let content: String = row.get(1)?;
                    let category: String = row.get(2)?;
                    let importance: i32 = row.get(3)?;
                    Ok((content, category, importance))
                })
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

            let mut summaries: Vec<String> = Vec::new();
            let mut count = 0i32;
            for row in rows {
                let (content, category, importance) =
                    row.map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
                let truncated = if content.len() > 100 {
                    content[..100].to_string()
                } else {
                    content.clone()
                };
                summaries.push(format!(
                    "[{}] (importance:{}) {}",
                    category, importance, truncated
                ));
                count += 1;
            }
            Ok((summaries, count))
        })
        .await?;

    let digest_content = if summaries.is_empty() {
        format!("No activity during this {} period.", period_owned)
    } else {
        format!(
            "{} period summary ({} memories):\n{}",
            period_owned,
            count,
            summaries.join("\n")
        )
    };

    let interval_owned2 = interval.to_string();
    let period_owned2 = period_owned.clone();
    let digest_content_clone = digest_content.clone();

    let id = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO digests (period, content, memory_count, user_id, started_at, ended_at) \
                 VALUES (?1, ?2, ?3, ?4, datetime('now', ?5), datetime('now'))",
                params![period_owned2, digest_content_clone, count, user_id, interval_owned2],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            Ok(conn.last_insert_rowid())
        })
        .await?;

    Ok(Digest {
        id,
        period: period_owned,
        content: digest_content,
        memory_count: count,
        user_id,
        started_at: None,
        ended_at: None,
        created_at: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    })
}

/// List existing digests.
pub async fn list_digests(db: &Database, user_id: i64, limit: usize) -> Result<Vec<Digest>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, period, content, memory_count, user_id, started_at, ended_at, created_at \
                 FROM digests WHERE user_id = ?1 ORDER BY id DESC LIMIT ?2",
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        let rows = stmt
            .query_map(params![user_id, limit as i64], |row| {
                Ok(Digest {
                    id: row.get(0)?,
                    period: row.get(1)?,
                    content: row.get(2)?,
                    memory_count: row.get(3)?,
                    user_id: row.get(4)?,
                    started_at: row.get(5)?,
                    ended_at: row.get(6)?,
                    created_at: row.get(7)?,
                })
            })
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))
    })
    .await
}
