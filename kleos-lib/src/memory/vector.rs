use super::types::VectorHit;
use crate::db::Database;
use crate::EngError;
use crate::Result;
use std::collections::HashSet;
use std::fmt::Write as _;
use tracing::warn;

/// Serialize a query embedding to the `[f1,f2,...]` string form that
/// libsql's `vector()` parser accepts. Single pre-allocated String +
/// write! loop -- previously `format!("[{}]", v.iter().map(to_string)
/// .collect::<Vec<_>>().join(","))` allocated a String per float
/// (1024x) plus a Vec<String> plus the final join buffer (R8 P-001).
fn embedding_to_json_array(embedding: &[f32]) -> String {
    // f32 Display output is 7-13 chars; 14 is a safe upper bound for
    // the bge-m3 1024-dim vectors we ship. 2 extra for brackets.
    let mut out = String::with_capacity(embedding.len() * 14 + 2);
    out.push('[');
    for (i, f) in embedding.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(&mut out, "{f}");
    }
    out.push(']');
    out
}

/// Search for similar memories using SQLite's native vector index.
/// Returns up to `limit` results ordered by vector similarity (most similar first).
/// Note: This uses vector_top_k which requires the sqlite-vec extension.
/// If the extension is not available, the query will fail gracefully and return empty results.
/// The primary vector search path uses LanceDB; this is a fallback for embedded deployments.
#[tracing::instrument(skip(db, embedding), fields(embedding_dim = embedding.len(), limit, user_id))]
pub async fn vector_search(
    db: &Database,
    embedding: &[f32],
    limit: usize,
    user_id: i64,
) -> Result<Vec<VectorHit>> {
    let embedding_json = embedding_to_json_array(embedding);

    // vector_top_k returns rowids ordered by distance (ascending = most similar first).
    // We JOIN on memories.rowid = id to get the full row filters applied.
    // Note: vector_top_k requires sqlite-vec extension.
    // user_id is accepted for API compatibility but not used as a SQL filter:
    // tenant isolation is enforced at the database level (one DB per tenant).
    #[allow(unused_variables)]
    let _user_id_for_compat = user_id;
    let sql = "
        SELECT memories.id
        FROM vector_top_k('memories_vec_1024_idx', vector(?1), ?2)
        JOIN memories ON memories.rowid = id
        WHERE memories.is_forgotten = 0
          AND memories.is_latest = 1
          AND memories.is_consolidated = 0
    ";

    match db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(sql)
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            let mut rows = stmt
                .query(rusqlite::params![embedding_json, limit as i64])
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

            // 6.9 capacity hint: LIMIT bounds the row count.
            let mut hits = Vec::with_capacity(limit);
            let mut rank: usize = 0;
            while let Some(row) = rows
                .next()
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
            {
                let memory_id: i64 = row
                    .get(0)
                    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
                hits.push(VectorHit {
                    memory_id,
                    distance: None,
                    rank,
                });
                rank += 1;
            }

            Ok(hits)
        })
        .await
    {
        Ok(hits) => Ok(hits),
        Err(e) => {
            warn!("vector search failed (sqlite-vec may not be loaded): {}", e);
            Ok(vec![])
        }
    }
}

/// Chunk-level vector search. Hits the per-chunk LanceDB table, decodes
/// each result key back to its parent memory_id, and returns one hit per
/// memory ranked by best (minimum) chunk distance. Falls back to an empty
/// result if `db.chunk_vector_index` is absent.
///
/// `over_fetch_factor` should match `embedding_chunk_max_chunks` so that
/// even when every memory has the maximum number of chunks we still see
/// `limit` distinct memories.
#[tracing::instrument(skip(db, embedding), fields(embedding_dim = embedding.len(), limit))]
pub async fn chunk_vector_search(
    db: &Database,
    embedding: &[f32],
    limit: usize,
) -> Result<Vec<VectorHit>> {
    let Some(index) = db.chunk_vector_index.as_ref() else {
        return Ok(Vec::new());
    };

    let over_fetch_factor = db.embedding_chunk_max_chunks.max(1);
    let raw_hits = index.search(embedding, limit * over_fetch_factor).await?;

    let mut seen: HashSet<i64> = HashSet::with_capacity(limit);
    let mut out: Vec<VectorHit> = Vec::with_capacity(limit);
    let mut rank: usize = 0;
    for hit in raw_hits {
        let memory_id = super::lance_key_to_memory_id(hit.memory_id);
        if seen.insert(memory_id) {
            out.push(VectorHit {
                memory_id,
                distance: hit.distance,
                rank,
            });
            rank += 1;
            if out.len() >= limit {
                break;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::embedding_to_json_array;

    fn old_format(embedding: &[f32]) -> String {
        format!(
            "[{}]",
            embedding
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
                .join(",")
        )
    }

    #[test]
    fn empty() {
        assert_eq!(embedding_to_json_array(&[]), "[]");
    }

    #[test]
    fn single() {
        assert_eq!(embedding_to_json_array(&[1.5]), old_format(&[1.5]));
    }

    #[test]
    fn matches_old_format_on_full_vector() {
        let v: Vec<f32> = (0..1024).map(|i| (i as f32).sin() * 0.25).collect();
        assert_eq!(embedding_to_json_array(&v), old_format(&v));
    }

    #[test]
    fn matches_old_format_with_specials() {
        let v = [0.0_f32, -0.0, 1.0, -1.0, f32::MIN, f32::MAX, 1e-10, 1e10];
        assert_eq!(embedding_to_json_array(&v), old_format(&v));
    }
}
