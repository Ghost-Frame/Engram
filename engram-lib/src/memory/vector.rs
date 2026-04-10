use crate::db::Database;
use crate::Result;
use tracing::warn;

/// Result from vector ANN search -- id and its rank position (0-based, ascending similarity)
#[derive(Debug, Clone)]
pub struct VectorHit {
    pub memory_id: i64,
    pub rank: usize,
}

/// Search for similar memories using libsql's native vector index.
/// Returns up to `limit` results ordered by vector similarity (most similar first).
pub async fn vector_search(
    db: &Database,
    embedding: &[f32],
    limit: usize,
    user_id: i64,
) -> Result<Vec<VectorHit>> {
    let conn = db.connection();
    let embedding_json = format!(
        "[{}]",
        embedding
            .iter()
            .map(|f| f.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );

    // vector_top_k returns rowids ordered by distance (ascending = most similar first).
    // We JOIN on memories.rowid = id to get the full row filters applied.
    let sql = "
        SELECT memories.id
        FROM vector_top_k('memories_vec_1024_idx', vector(?1), ?2)
        JOIN memories ON memories.rowid = id
        WHERE memories.is_forgotten = 0
          AND memories.is_latest = 1
          AND memories.user_id = ?3
    ";

    let mut rows = match conn
        .query(sql, libsql::params![embedding_json, limit as i64, user_id])
        .await
    {
        Ok(r) => r,
        Err(e) => {
            warn!("vector search failed: {}", e);
            return Ok(vec![]);
        }
    };

    let mut hits = Vec::new();
    let mut rank: usize = 0;
    while let Some(row) = rows.next().await? {
        let memory_id: i64 = row.get(0)?;
        hits.push(VectorHit { memory_id, rank });
        rank += 1;
    }

    Ok(hits)
}
