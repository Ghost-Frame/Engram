use crate::db::Database;
use crate::Result;
use libsql::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressedEpisode {
    pub summary: String,
    pub source_memory_ids: Vec<i64>,
    pub result_memory_id: i64,
    pub period_start: String,
    pub period_end: String,
}

/// Compress memories from a weekly period into summary memories.
/// Groups memories by week, scores sentences heuristically, creates summaries.
pub async fn compress_weekly(db: &Database, user_id: i64) -> Result<Vec<CompressedEpisode>> {
    let conn = db.connection();
    let mut compressed = Vec::new();

    // Find weeks with enough memories to compress (7+ memories older than 7 days)
    let mut rows = conn
        .query(
            "SELECT strftime('%Y-%W', created_at) as week, \
         GROUP_CONCAT(id) as ids, COUNT(*) as cnt, \
         MIN(created_at) as period_start, MAX(created_at) as period_end \
         FROM memories \
         WHERE user_id = ?1 AND is_forgotten = 0 AND is_latest = 1 \
         AND created_at < datetime('now', '-7 days') \
         GROUP BY week HAVING cnt >= 7 \
         ORDER BY week DESC LIMIT 4",
            params![user_id],
        )
        .await?;

    let mut weeks: Vec<(String, Vec<i64>, String, String)> = Vec::new();
    while let Some(row) = rows.next().await? {
        let _week: String = row.get(0)?;
        let ids_str: String = row.get(1)?;
        let ids: Vec<i64> = ids_str
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        let start: String = row.get(3)?;
        let end: String = row.get(4)?;
        weeks.push((_week, ids, start, end));
    }

    for (_week, memory_ids, period_start, period_end) in weeks {
        // Fetch contents and score sentences
        let mut contents: Vec<(String, i32)> = Vec::new();
        for &id in memory_ids.iter().take(50) {
            let mut mrows = conn
                .query(
                    "SELECT content, importance FROM memories WHERE id = ?1 AND user_id = ?2",
                    params![id, user_id],
                )
                .await?;
            if let Some(row) = mrows.next().await? {
                let content: String = row.get(0)?;
                let importance: i32 = row.get(1)?;
                contents.push((content, importance));
            }
        }

        if contents.is_empty() {
            continue;
        }

        // Heuristic: take the most important sentences
        let summary = build_summary(&contents, 500);

        // Store as a new compressed memory
        conn.execute(
            "INSERT INTO memories (content, category, source, importance, is_latest, status, user_id) \
             VALUES (?1, 'general', 'compression', 7, 1, 'approved', ?2)",
            params![summary.clone(), user_id],
        ).await?;

        let mut id_rows = conn.query("SELECT last_insert_rowid()", ()).await?;
        let result_id: i64 = if let Some(row) = id_rows.next().await? {
            row.get(0)?
        } else {
            continue;
        };

        // Link compressed memory to originals via memory_links
        for &source_id in memory_ids.iter().take(50) {
            let _ = conn
                .execute(
                    "INSERT OR IGNORE INTO memory_links (source_id, target_id, similarity, type) \
                 VALUES (?1, ?2, 1.0, 'generalizes')",
                    params![result_id, source_id],
                )
                .await;
        }

        compressed.push(CompressedEpisode {
            summary: summary.clone(),
            source_memory_ids: memory_ids,
            result_memory_id: result_id,
            period_start,
            period_end,
        });
    }

    Ok(compressed)
}

/// Build a summary from content+importance pairs, capped at max_chars.
fn build_summary(contents: &[(String, i32)], max_chars: usize) -> String {
    let mut scored: Vec<(&str, i32)> = Vec::new();

    for (content, importance) in contents {
        for sentence in content.split(['.', '\n']) {
            let trimmed = sentence.trim();
            if trimmed.len() >= 15 {
                scored.push((trimmed, *importance));
            }
        }
    }

    // Sort by importance descending
    scored.sort_by(|a, b| b.1.cmp(&a.1));

    let mut summary = String::new();
    for (sentence, _) in scored {
        if summary.len() + sentence.len() + 2 > max_chars {
            break;
        }
        if !summary.is_empty() {
            summary.push_str(". ");
        }
        summary.push_str(sentence);
    }

    summary
}
