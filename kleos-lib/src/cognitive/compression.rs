use super::types::CompressedEpisode;
use crate::db::Database;
use crate::{EngError, Result};
use rusqlite::{params, OptionalExtension};

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

/// Compress memories from a weekly period into summary memories.
/// Groups memories by week, scores sentences heuristically, creates summaries.
#[tracing::instrument(skip(db), fields(user_id))]
pub async fn compress_weekly(db: &Database, user_id: i64) -> Result<Vec<CompressedEpisode>> {
    let mut compressed = Vec::new();

    // Find weeks with enough memories to compress (7+ memories older than 7 days)
    let weeks: Vec<(String, Vec<i64>, String, String)> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT strftime('%Y-%W', created_at) as week, \
                     GROUP_CONCAT(id) as ids, COUNT(*) as cnt, \
                     MIN(created_at) as period_start, MAX(created_at) as period_end \
                     FROM memories \
                     WHERE user_id = ?1 AND is_forgotten = 0 AND is_latest = 1 \
                     AND created_at < datetime('now', '-7 days') \
                     GROUP BY week HAVING cnt >= 7 \
                     ORDER BY week DESC LIMIT 4",
                )
                .map_err(rusqlite_to_eng_error)?;

            let rows = stmt
                .query_map(params![user_id], |row| {
                    let week: String = row.get(0)?;
                    let ids_str: String = row.get(1)?;
                    let start: String = row.get(3)?;
                    let end: String = row.get(4)?;
                    Ok((week, ids_str, start, end))
                })
                .map_err(rusqlite_to_eng_error)?;

            let mut weeks = Vec::new();
            for row in rows {
                let (week, ids_str, start, end) = row.map_err(rusqlite_to_eng_error)?;
                let ids: Vec<i64> = ids_str
                    .split(',')
                    .filter_map(|s| s.trim().parse().ok())
                    .collect();
                weeks.push((week, ids, start, end));
            }
            Ok(weeks)
        })
        .await?;

    for (_week, memory_ids, period_start, period_end) in weeks {
        // Fetch contents and score sentences
        let ids_to_fetch: Vec<i64> = memory_ids.iter().take(50).copied().collect();
        let contents: Vec<(String, i32)> = db
            .read(move |conn| {
                let mut results = Vec::new();
                for id in &ids_to_fetch {
                    let mut stmt = conn
                        .prepare(
                            "SELECT content, importance FROM memories WHERE id = ?1 AND user_id = ?2",
                        )
                        .map_err(rusqlite_to_eng_error)?;
                    let row = stmt
                        .query_row(params![id, user_id], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, i32>(1)?))
                        })
                        .optional()
                        .map_err(rusqlite_to_eng_error)?;
                    if let Some(pair) = row {
                        results.push(pair);
                    }
                }
                Ok(results)
            })
            .await?;

        if contents.is_empty() {
            continue;
        }

        // Heuristic: take the most important sentences
        let summary = build_summary(&contents, 500);

        // Store as a new compressed memory
        let summary_clone = summary.clone();
        let result_id: i64 = db
            .write(move |conn| {
                conn.execute(
                    "INSERT INTO memories (content, category, source, importance, is_latest, status, user_id) \
                     VALUES (?1, 'general', 'compression', 7, 1, 'approved', ?2)",
                    params![summary_clone, user_id],
                )
                .map_err(rusqlite_to_eng_error)?;
                Ok(conn.last_insert_rowid())
            })
            .await?;

        // Link compressed memory to originals via memory_links
        let source_ids: Vec<i64> = memory_ids.iter().take(50).copied().collect();
        let _ = db
            .write(move |conn| {
                for source_id in &source_ids {
                    let _ = conn.execute(
                        "INSERT OR IGNORE INTO memory_links (source_id, target_id, similarity, type) \
                         VALUES (?1, ?2, 1.0, 'generalizes')",
                        params![result_id, source_id],
                    );
                }
                Ok(())
            })
            .await;

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
    scored.sort_by_key(|b| std::cmp::Reverse(b.1));

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
