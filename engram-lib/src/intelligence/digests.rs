use crate::db::Database;
use crate::Result;
use libsql::params;
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
    let conn = db.connection();

    let interval = match period {
        "daily" => "-1 day",
        "weekly" => "-7 days",
        "monthly" => "-30 days",
        _ => "-1 day",
    };

    // Fetch recent memories in the period
    let mut rows = conn.query(
        "SELECT id, content, category, importance FROM memories \
         WHERE user_id = ?1 AND is_forgotten = 0 AND created_at >= datetime('now', ?2) \
         ORDER BY importance DESC LIMIT 50",
        params![user_id, interval],
    ).await?;

    let mut summaries: Vec<String> = Vec::new();
    let mut count = 0i32;
    while let Some(row) = rows.next().await? {
        let content: String = row.get(1)?;
        let category: String = row.get(2)?;
        let importance: i32 = row.get(3)?;
        // Take first 100 chars as summary line
        let truncated = if content.len() > 100 { &content[..100] } else { &content };
        summaries.push(format!("[{}] (importance:{}) {}", category, importance, truncated));
        count += 1;
    }

    let digest_content = if summaries.is_empty() {
        format!("No activity during this {} period.", period)
    } else {
        format!("{} period summary ({} memories):\n{}", period, count, summaries.join("\n"))
    };

    conn.execute(
        "INSERT INTO digests (period, content, memory_count, user_id, started_at, ended_at) \
         VALUES (?1, ?2, ?3, ?4, datetime('now', ?5), datetime('now'))",
        params![period, digest_content.clone(), count, user_id, interval],
    ).await?;

    let mut id_rows = conn.query("SELECT last_insert_rowid()", ()).await?;
    let id: i64 = if let Some(row) = id_rows.next().await? { row.get(0)? } else { 0 };

    Ok(Digest {
        id,
        period: period.into(),
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
    let conn = db.connection();
    let mut rows = conn.query(
        "SELECT id, period, content, memory_count, user_id, started_at, ended_at, created_at \
         FROM digests WHERE user_id = ?1 ORDER BY id DESC LIMIT ?2",
        params![user_id, limit as i64],
    ).await?;

    let mut results = Vec::new();
    while let Some(row) = rows.next().await? {
        results.push(Digest {
            id: row.get(0)?,
            period: row.get(1)?,
            content: row.get(2)?,
            memory_count: row.get(3)?,
            user_id: row.get(4)?,
            started_at: row.get(5)?,
            ended_at: row.get(6)?,
            created_at: row.get(7)?,
        });
    }
    Ok(results)
}
