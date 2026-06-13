use super::types::VectorHit;
use crate::db::Database;
use crate::Result;
use std::collections::HashSet;
use std::fmt::Write as _;
use tracing::warn;

/// Over-fetch multiplier applied before the owner/visibility filter on ANN
/// results, so foreign or superseded neighbours returned by a global top-k do
/// not starve the owned result set below the requested `limit`.
const POOL_INFLATION: usize = 8;

/// Minimum extra candidates to fetch on top of `limit` (covers small limits
/// where the multiplier alone is a thin buffer).
const MIN_POOL_BUFFER: usize = 32;

/// Extra distinct memories collected beyond `limit` in chunk search before the
/// owner/visibility filter, bounding the size of the follow-up IN query.
const COLLECT_BUFFER: usize = 128;

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
    //
    // The owner/visibility predicates (?3 + is_forgotten/is_latest) keep
    // single-DB (shared) mode from returning another user's nearest neighbours
    // and exclude superseded/forgotten rows. Because vector_top_k applies a HARD
    // top-k GLOBALLY before these filters run, asking for exactly `limit` rows
    // would let foreign or stale neighbours consume the slots, returning fewer
    // than `limit` owned hits (recall starvation). Over-fetch a larger pool,
    // filter, then truncate to `limit` in distance order.
    let pool = limit
        .saturating_mul(POOL_INFLATION)
        .max(limit.saturating_add(MIN_POOL_BUFFER));

    let sql = "
        SELECT memories.id
        FROM vector_top_k('memories_vec_1024_idx', vector(?1), ?2)
        JOIN memories ON memories.rowid = id
        WHERE memories.user_id = ?3
          AND memories.is_forgotten = 0
          AND memories.is_latest = 1
    ";

    match db
        .read(move |conn| {
            let mut stmt = conn.prepare(sql)?;
            let mut rows = stmt.query(rusqlite::params![embedding_json, pool as i64, user_id])?;

            let mut hits = Vec::with_capacity(limit);
            let mut rank: usize = 0;
            while let Some(row) = rows.next()? {
                let memory_id: i64 = row.get(0)?;
                hits.push(VectorHit {
                    memory_id,
                    distance: None,
                    rank,
                    matching_chunk_text: None,
                });
                rank += 1;
                // Truncate to the requested count once enough owned hits survive.
                if hits.len() >= limit {
                    break;
                }
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
#[tracing::instrument(skip(db, embedding), fields(embedding_dim = embedding.len(), limit, user_id))]
pub async fn chunk_vector_search(
    db: &Database,
    embedding: &[f32],
    limit: usize,
    user_id: i64,
) -> Result<Vec<VectorHit>> {
    let Some(index) = db.chunk_vector_index.as_ref() else {
        return Ok(Vec::new());
    };

    let over_fetch_factor = db.embedding_chunk_max_chunks.max(1);
    let raw_hits = index.search(embedding, limit * over_fetch_factor).await?;

    // Dedup chunk hits down to distinct parent memories in distance order. The
    // chunk index carries no tenant identity, so we OVER-COLLECT candidates
    // (beyond `limit`) and then drop any that are not owned by the caller and
    // currently visible (is_latest, not forgotten). Without this filter foreign
    // or superseded memories occupy the `limit` slots and are only discarded
    // later at hydration -- recall starvation in monolith mode.
    let collect_cap = limit.saturating_add(COLLECT_BUFFER);
    let mut seen: HashSet<i64> = HashSet::with_capacity(collect_cap);
    // (memory_id, chunk_idx, distance) preserving distance order.
    let mut candidates: Vec<(i64, usize, Option<f32>)> = Vec::with_capacity(collect_cap);
    for hit in raw_hits {
        let memory_id = super::lance_key_to_memory_id(hit.memory_id);
        let chunk_idx = (hit.memory_id % 1000) as usize;
        if seen.insert(memory_id) {
            candidates.push((memory_id, chunk_idx, hit.distance));
            if candidates.len() >= collect_cap {
                break;
            }
        }
    }

    let candidate_ids: Vec<i64> = candidates.iter().map(|(id, _, _)| *id).collect();
    let owned = filter_owned_visible(db, &candidate_ids, user_id).await?;

    let mut out: Vec<VectorHit> = Vec::with_capacity(limit);
    let mut winners: Vec<(i64, usize)> = Vec::with_capacity(limit);
    let mut rank: usize = 0;
    for (memory_id, chunk_idx, distance) in candidates {
        if !owned.contains(&memory_id) {
            continue;
        }
        out.push(VectorHit {
            memory_id,
            distance,
            rank,
            matching_chunk_text: None,
        });
        winners.push((memory_id, chunk_idx));
        rank += 1;
        if out.len() >= limit {
            break;
        }
    }

    if let Ok(texts) = fetch_chunk_texts_batch(db, &winners).await {
        for hit in &mut out {
            if let Some(text) = texts.get(&hit.memory_id) {
                hit.matching_chunk_text = Some(text.clone());
            }
        }
    }

    Ok(out)
}

/// Return the subset of `ids` that belong to `user_id` and are currently
/// visible (is_latest, not forgotten). One batched query; preserves nothing
/// about order (caller re-orders against its candidate list).
async fn filter_owned_visible(db: &Database, ids: &[i64], user_id: i64) -> Result<HashSet<i64>> {
    if ids.is_empty() {
        return Ok(HashSet::new());
    }
    let ids_owned: Vec<i64> = ids.to_vec();
    db.read(move |conn| {
        let placeholders = ids_owned.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT id FROM memories \
             WHERE id IN ({placeholders}) AND user_id = ? \
               AND is_forgotten = 0 AND is_latest = 1"
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut params: Vec<&dyn rusqlite::types::ToSql> = Vec::with_capacity(ids_owned.len() + 1);
        for id in &ids_owned {
            params.push(id);
        }
        params.push(&user_id);
        let mut owned = HashSet::with_capacity(ids_owned.len());
        let mut rows = stmt.query(params.as_slice())?;
        while let Some(row) = rows.next()? {
            owned.insert(row.get::<_, i64>(0)?);
        }
        Ok(owned)
    })
    .await
}

async fn fetch_chunk_texts_batch(
    db: &Database,
    winners: &[(i64, usize)],
) -> Result<std::collections::HashMap<i64, String>> {
    if winners.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let winners_owned: Vec<(i64, usize)> = winners.to_vec();
    db.read(move |conn| {
        let mut map = std::collections::HashMap::with_capacity(winners_owned.len());
        let mut stmt = conn.prepare(
            "SELECT content FROM memory_chunks \
                 WHERE memory_id = ?1 AND chunk_idx = ?2",
        )?;
        for (memory_id, chunk_idx) in &winners_owned {
            if let Ok(text) = stmt
                .query_row(rusqlite::params![memory_id, *chunk_idx as i64], |row| {
                    row.get::<_, String>(0)
                })
            {
                map.insert(*memory_id, text);
            }
        }
        Ok(map)
    })
    .await
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

    // The owner/visibility filter underpinning chunk-search recall: only the
    // caller's currently-visible memories survive, so foreign or superseded
    // candidates can no longer occupy result slots.
    #[tokio::test]
    async fn filter_owned_visible_excludes_foreign_and_stale() {
        use crate::db::Database;
        let db = Database::connect_memory().await.expect("db");
        let ids: Vec<i64> = db
            .write(|conn| {
                let mut ids = Vec::new();
                // (user_id, is_latest, is_forgotten)
                for (uid, latest, forgotten) in
                    [(1i64, 1i64, 0i64), (2, 1, 0), (1, 0, 0), (1, 1, 1)]
                {
                    let id: i64 = conn.query_row(
                        "INSERT INTO memories (content, user_id, is_latest, is_forgotten) \
                         VALUES ('x', ?1, ?2, ?3) RETURNING id",
                        rusqlite::params![uid, latest, forgotten],
                        |r| r.get(0),
                    )?;
                    ids.push(id);
                }
                Ok(ids)
            })
            .await
            .unwrap();

        let owned = super::filter_owned_visible(&db, &ids, 1).await.unwrap();
        assert!(owned.contains(&ids[0]), "owned + visible must be kept");
        assert!(!owned.contains(&ids[1]), "foreign user excluded");
        assert!(
            !owned.contains(&ids[2]),
            "superseded (is_latest=0) excluded"
        );
        assert!(!owned.contains(&ids[3]), "forgotten excluded");
        assert_eq!(owned.len(), 1);
    }
}
