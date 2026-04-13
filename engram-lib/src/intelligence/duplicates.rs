//! Duplicate detection and deduplication via memory_links similarity scores.

use crate::db::Database;
use crate::{EngError, Result};
use serde::{Deserialize, Serialize};

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicatePair {
    pub id_a: i64,
    pub id_b: i64,
    pub content_a: String,
    pub content_b: String,
    pub similarity: f64,
    pub importance_a: i32,
    pub importance_b: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeduplicateResult {
    pub pairs_found: i64,
    pub merged: i64,
    pub dry_run: bool,
}

/// Find duplicate memory pairs based on similarity links.
pub async fn find_duplicates(
    db: &Database,
    user_id: i64,
    threshold: f64,
    limit: i64,
) -> Result<Vec<DuplicatePair>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT ml.source_id, ml.target_id, ml.similarity, \
                        ms.content, mt.content, ms.importance, mt.importance \
                 FROM memory_links ml \
                 JOIN memories ms ON ms.id = ml.source_id \
                 JOIN memories mt ON mt.id = ml.target_id \
                 WHERE ml.similarity >= ?1 \
                   AND ml.type = 'similarity' \
                   AND ms.user_id = ?2 AND mt.user_id = ?2 \
                   AND ms.is_forgotten = 0 AND mt.is_forgotten = 0 \
                   AND ms.is_superseded = 0 AND mt.is_superseded = 0 \
                 ORDER BY ml.similarity DESC \
                 LIMIT ?3",
            )
            .map_err(rusqlite_to_eng_error)?;

        let pairs = stmt
            .query_map(
                rusqlite::params![threshold, user_id, limit],
                |row| {
                    Ok(DuplicatePair {
                        id_a: row.get(0)?,
                        id_b: row.get(1)?,
                        similarity: row.get(2)?,
                        content_a: row.get(3)?,
                        content_b: row.get(4)?,
                        importance_a: row.get(5)?,
                        importance_b: row.get(6)?,
                    })
                },
            )
            .map_err(rusqlite_to_eng_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(rusqlite_to_eng_error)?;

        Ok(pairs)
    })
    .await
}

/// Deduplicate memories by marking lower-importance duplicates as superseded.
/// If `dry_run` is true, returns stats without modifying data.
pub async fn deduplicate(
    db: &Database,
    user_id: i64,
    threshold: f64,
    dry_run: bool,
) -> Result<DeduplicateResult> {
    let pairs = find_duplicates(db, user_id, threshold, 500).await?;
    let pairs_found = pairs.len() as i64;

    if dry_run {
        return Ok(DeduplicateResult {
            pairs_found,
            merged: 0,
            dry_run: true,
        });
    }

    let mut merged = 0i64;
    for pair in &pairs {
        // Keep the one with higher importance; on tie, keep the older one (lower id)
        let (keep_id, supersede_id) = if pair.importance_a > pair.importance_b {
            (pair.id_a, pair.id_b)
        } else if pair.importance_b > pair.importance_a {
            (pair.id_b, pair.id_a)
        } else if pair.id_a < pair.id_b {
            (pair.id_a, pair.id_b)
        } else {
            (pair.id_b, pair.id_a)
        };

        let similarity = pair.similarity;

        let affected = db
            .write(move |conn| {
                let n = conn
                    .execute(
                        "UPDATE memories SET is_superseded = 1, updated_at = datetime('now') \
                         WHERE id = ?1 AND user_id = ?2 AND is_superseded = 0",
                        rusqlite::params![supersede_id, user_id],
                    )
                    .map_err(rusqlite_to_eng_error)?;
                Ok(n)
            })
            .await?;

        if affected > 0 {
            // Create a 'supersedes' link from keeper to superseded
            let _ = db
                .write(move |conn| {
                    conn.execute(
                        "INSERT OR IGNORE INTO memory_links (source_id, target_id, similarity, type) \
                         VALUES (?1, ?2, ?3, 'supersedes')",
                        rusqlite::params![keep_id, supersede_id, similarity],
                    )
                    .map_err(rusqlite_to_eng_error)?;
                    Ok(())
                })
                .await;
            merged += 1;
        }
    }

    Ok(DeduplicateResult {
        pairs_found,
        merged,
        dry_run: false,
    })
}
