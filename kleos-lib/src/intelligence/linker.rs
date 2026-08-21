//! Associative auto-linker -- reconnects each memory to its nearest semantic
//! neighbours by inserting `similarity`-typed rows into `memory_links`.
//!
//! ## Why this exists
//!
//! An earlier `auto_link` pass ran on every store and produced the bulk of the
//! graph's edges (the `cite`/`similarity` links). Its call site was stripped in
//! a refactor and the now-orphaned function was later deleted as "dead code"
//! (commit 75905ca0). The effect was silent: from that point on, newly stored
//! memories accrued zero associative links, so the memory graph degraded into a
//! disconnected dust field and the dedup/consolidation passes -- which READ
//! `type = 'similarity'` links -- went inert because nothing wrote them anymore.
//!
//! This module restores that behaviour, but OFF the write path. Instead of
//! linking synchronously inside `store`, [`link_unlinked_batch`] processes a
//! throttled batch of as-yet-unlinked memories. The background dreamer pipeline
//! calls it each cycle (forward-fill for new memories) and the admin
//! `backfill-links` command calls it in a loop (one-shot backfill of the
//! historical unlinked set). Both share the same code path, so behaviour can't
//! drift between them.
//!
//! ## How a memory is linked
//!
//! 1. ANN search for the memory's `ANN_K` nearest neighbours (LanceDB primary,
//!    sqlite-vec fallback).
//! 2. Convert distance to cosine similarity, drop the self-hit, keep neighbours
//!    at or above [`AUTO_LINK_THRESHOLD`].
//! 3. Take the strongest [`AUTO_LINK_MAX`] and insert a bidirectional
//!    `similarity` link to each. Ownership is enforced by `insert_link`
//!    (`user_id` + existence check), so a stray cross-tenant neighbour from a
//!    shared index is silently rejected rather than linked.

use crate::db::Database;
use crate::vector::VectorHit;
use crate::Result;
use serde::Serialize;
use std::collections::HashSet;
use tracing::warn;

/// Minimum cosine similarity for an auto-link. Matches the pre-regression value.
pub const AUTO_LINK_THRESHOLD: f64 = 0.55;

/// Maximum neighbours linked per memory. Matches the pre-regression value.
pub const AUTO_LINK_MAX: usize = 6;

/// Nearest-neighbour fetch width before threshold/truncation are applied.
const ANN_K: usize = 50;

/// Over-fetch factor that prevents stale Lance rows from starving valid targets.
const ANN_POOL_INFLATION: usize = 8;

/// Outcome of a batch linking pass over one tenant.
#[derive(Debug, Default, Clone, Serialize)]
pub struct LinkBatchReport {
    /// Unlinked memories examined this pass.
    pub scanned: usize,
    /// Memories that gained at least one link (or would, when `dry_run`).
    pub memories_linked: usize,
    /// Total `similarity` links created (counts each neighbour once, not both
    /// directions). With `dry_run` this is the count that WOULD be created.
    pub links_created: usize,
    /// Memories skipped because their stored embedding was missing/malformed.
    pub skipped_no_embedding: usize,
    /// Whether this was a dry run (no rows written).
    pub dry_run: bool,
}

/// Reduce raw ANN hits to the link targets for `self_id`: drop the self-hit,
/// convert distance to similarity (rank-based fallback when distance is absent),
/// keep hits at or above `threshold`, sort strongest-first, and cap at `max`.
///
/// Pure and DB-free so the ranking rules can be unit-tested directly.
fn rank_candidates(
    hits: &[VectorHit],
    self_id: i64,
    threshold: f64,
    max: usize,
) -> Vec<(i64, f64)> {
    let mut scored: Vec<(i64, f64)> = Vec::new();
    for hit in hits {
        if hit.memory_id == self_id {
            continue;
        }
        // LanceDB cosine distance -> similarity. When the index reports no
        // distance, approximate from rank so we still produce some links.
        // Clamp like search.rs::semantic_score_from_distance: PQ-quantized
        // distances can come back marginally negative for near-identical
        // vectors, and an unclamped sim > 1.0 would be rejected by
        // insert_link's (0.0, 1.0] guard -- silently dropping exactly the
        // strongest (near-duplicate) candidate in both directions.
        let sim = match hit.distance {
            Some(d) => (1.0 - d as f64).clamp(0.0, 1.0),
            None => 1.0 - (hit.rank as f64 / ANN_K as f64),
        };
        if sim >= threshold {
            scored.push((hit.memory_id, sim));
        }
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(max);
    scored
}

/// Keep ANN hits that still resolve to active, graph-visible memories owned by
/// `user_id`, preserving the index's distance order.
///
/// Lance is eventually consistent with SQLite. Forgotten, archived, superseded,
/// or pending rows can therefore remain in its nearest-neighbour window after
/// SQLite stops treating them as linkable. Filtering before ranking prevents
/// those stale rows from consuming every auto-link slot.
async fn filter_linkable_hits(
    db: &Database,
    hits: Vec<VectorHit>,
    user_id: i64,
) -> Result<Vec<VectorHit>> {
    if hits.is_empty() {
        return Ok(Vec::new());
    }

    let ids: Vec<i64> = hits.iter().map(|hit| hit.memory_id).collect();
    let linkable: HashSet<i64> = db
        .read(move |conn| {
            let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT id FROM memories \
                 WHERE id IN ({placeholders}) AND user_id = ? \
                   AND is_forgotten = 0 AND is_archived = 0 AND is_latest = 1 \
                   AND status != 'pending'"
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut params: Vec<&dyn rusqlite::types::ToSql> = Vec::with_capacity(ids.len() + 1);
            for id in &ids {
                params.push(id);
            }
            params.push(&user_id);

            let mut rows = stmt.query(params.as_slice())?;
            let mut out = HashSet::with_capacity(ids.len());
            while let Some(row) = rows.next()? {
                out.insert(row.get::<_, i64>(0)?);
            }
            Ok(out)
        })
        .await?;

    let mut filtered: Vec<VectorHit> = hits
        .into_iter()
        .filter(|hit| linkable.contains(&hit.memory_id))
        .collect();
    for (rank, hit) in filtered.iter_mut().enumerate() {
        // SQLite-vec hits carry no distance, so their similarity fallback uses
        // rank. Re-densify after filtering or stale rows would still depress the
        // surviving candidates below the threshold.
        hit.rank = rank;
    }
    Ok(filtered)
}

/// Link a single memory to its nearest neighbours. Returns the number of
/// neighbour links created (each neighbour counted once). When `dry_run` is set,
/// nothing is written and the return value is the count that WOULD be created.
pub async fn auto_link(
    db: &Database,
    memory_id: i64,
    embedding: &[f32],
    user_id: i64,
    dry_run: bool,
) -> Result<usize> {
    // Primary path: the tenant's ANN index (cosine distance available). Search
    // a wider pool, then validate every hit against SQLite before ranking:
    // Lance may retain forgotten or superseded rows until its sync catches up.
    // Falls back to sqlite-vec when no index is loaded.
    let ranked_hits = if let Some(index) = db.vector_index.as_ref() {
        let pool = ANN_K.saturating_mul(ANN_POOL_INFLATION);
        let hits = index.search(embedding, pool).await.unwrap_or_default();
        filter_linkable_hits(db, hits, user_id).await?
    } else {
        let pool = ANN_K.saturating_mul(ANN_POOL_INFLATION);
        let hits = crate::memory::vector::vector_search(db, embedding, pool, user_id).await?;
        // The fallback's hit type carries the same distance/rank information;
        // adapt it before applying the stricter graph-visibility filter.
        let adapted: Vec<VectorHit> = hits
            .iter()
            .map(|h| VectorHit {
                memory_id: h.memory_id,
                distance: h.distance,
                rank: h.rank,
            })
            .collect();
        filter_linkable_hits(db, adapted, user_id).await?
    };
    let targets = rank_candidates(&ranked_hits, memory_id, AUTO_LINK_THRESHOLD, AUTO_LINK_MAX);

    if dry_run {
        return Ok(targets.len());
    }

    let mut linked = 0usize;
    for (target_id, similarity) in &targets {
        // Bidirectional, matching the historical edge shape. insert_link
        // validates ownership of both endpoints, so a foreign neighbour is
        // rejected here rather than producing a cross-tenant link.
        match crate::memory::insert_link(
            db,
            memory_id,
            *target_id,
            *similarity,
            "similarity",
            user_id,
        )
        .await
        {
            Ok(()) => linked += 1,
            Err(e) => {
                warn!(
                    memory_id,
                    target_id, "auto_link forward insert skipped: {}", e
                );
                continue;
            }
        }
        let _ = crate::memory::insert_link(
            db,
            *target_id,
            memory_id,
            *similarity,
            "similarity",
            user_id,
        )
        .await;
    }
    Ok(linked)
}

/// Decode a little-endian f32 blob (the `embedding_vec_1024` column format).
/// Returns `None` if the blob length is not a whole number of f32s or is empty.
fn decode_embedding(blob: &[u8]) -> Option<Vec<f32>> {
    if blob.is_empty() || !blob.len().is_multiple_of(4) {
        return None;
    }
    Some(
        blob.chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
    )
}

/// Select a throttled batch of graph-visible memories missing an outgoing
/// associative link.
///
/// A memory with only a cite, hierarchy, or generalization edge still needs a
/// semantic link or it remains trapped inside a legacy graph island. Selecting
/// specifically on `type = 'similarity'` lets the backfill bridge those islands
/// while remaining resumable: once a pass creates the bidirectional link, both
/// endpoints disappear from future batches.
async fn select_link_candidates(
    db: &Database,
    user_id: i64,
    limit: usize,
) -> Result<Vec<(i64, Vec<u8>)>> {
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT id, embedding_vec_1024 \
                 FROM memories \
                 WHERE user_id = ?1 \
                   AND is_forgotten = 0 AND is_archived = 0 AND is_latest = 1 \
                   AND status != 'pending' \
                   AND embedding_vec_1024 IS NOT NULL \
                   AND NOT EXISTS ( \
                       SELECT 1 \
                       FROM memory_links ml \
                       JOIN memories target ON target.id = ml.target_id \
                       WHERE ml.source_id = memories.id \
                         AND ml.type = 'similarity' \
                         AND target.user_id = ?1 \
                         AND target.is_forgotten = 0 \
                         AND target.is_archived = 0 \
                         AND target.is_latest = 1 \
                         AND target.status != 'pending' \
                   ) \
                 ORDER BY id DESC \
                 LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![user_id, limit as i64], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    })
    .await
}

/// Link a throttled batch of the user's memories still missing associative
/// similarity links.
pub async fn link_unlinked_batch(
    db: &Database,
    user_id: i64,
    limit: usize,
    dry_run: bool,
) -> Result<LinkBatchReport> {
    if limit == 0 {
        return Ok(LinkBatchReport {
            dry_run,
            ..Default::default()
        });
    }

    let candidates = select_link_candidates(db, user_id, limit).await?;

    let mut report = LinkBatchReport {
        dry_run,
        ..Default::default()
    };

    for (memory_id, blob) in &candidates {
        report.scanned += 1;
        let Some(embedding) = decode_embedding(blob) else {
            report.skipped_no_embedding += 1;
            continue;
        };
        match auto_link(db, *memory_id, &embedding, user_id, dry_run).await {
            Ok(0) => {}
            Ok(n) => {
                report.memories_linked += 1;
                report.links_created += n;
            }
            Err(e) => {
                warn!(memory_id, "link_unlinked_batch: auto_link failed: {}", e);
            }
        }
    }

    Ok(report)
}

/// Unit tests for the DB-free ranking and embedding-decode helpers.
#[cfg(test)]
mod tests {
    use super::*;

    /// Build a VectorHit for the ranking tests.
    fn hit(memory_id: i64, distance: Option<f32>, rank: usize) -> VectorHit {
        VectorHit {
            memory_id,
            distance,
            rank,
        }
    }

    /// Insert a memory row with a minimal valid embedding for linker selection
    /// tests and return its generated id.
    async fn insert_memory(
        db: &crate::db::Database,
        user_id: i64,
        status: &str,
        is_archived: bool,
        is_latest: bool,
        is_forgotten: bool,
    ) -> i64 {
        let status = status.to_string();
        db.write(move |conn| {
            Ok(conn.query_row(
                "INSERT INTO memories \
                 (content, user_id, status, is_archived, is_latest, is_forgotten, \
                  embedding_vec_1024) \
                 VALUES ('linker test', ?1, ?2, ?3, ?4, ?5, ?6) \
                 RETURNING id",
                rusqlite::params![
                    user_id,
                    status,
                    is_archived,
                    is_latest,
                    is_forgotten,
                    vec![0_u8; 4],
                ],
                |row| row.get(0),
            )?)
        })
        .await
        .expect("insert linker test memory")
    }

    /// A marginally negative PQ-quantized distance (near-duplicate noise)
    /// must clamp to similarity 1.0 and stay linkable, not overflow past 1.0
    /// into insert_link's rejection range -- that would silently drop exactly
    /// the strongest candidate in both directions.
    #[test]
    fn rank_candidates_clamps_negative_distance_to_valid_similarity() {
        let hits = vec![
            hit(2, Some(-0.02), 0), // PQ noise: raw sim would be 1.02
            hit(3, Some(0.10), 1),  // ordinary neighbour, sim 0.9
        ];
        let out = rank_candidates(&hits, 1, AUTO_LINK_THRESHOLD, AUTO_LINK_MAX);
        assert_eq!(out.len(), 2, "the near-duplicate must survive ranking");
        assert_eq!(out[0].0, 2, "near-duplicate ranks first");
        assert!(
            out[0].1 <= 1.0 && out[0].1 > 0.0,
            "similarity must land in insert_link's accepted (0.0, 1.0], got {}",
            out[0].1
        );
    }

    /// The self-hit is removed and neighbours below the threshold are dropped.
    #[test]
    fn rank_candidates_drops_self_and_below_threshold() {
        let hits = vec![
            hit(1, Some(0.0), 0), // self -> dropped
            hit(2, Some(0.1), 1), // sim 0.9 -> keep
            hit(3, Some(0.5), 2), // sim 0.5 -> below 0.55, drop
            hit(4, Some(0.4), 3), // sim 0.6 -> keep
        ];
        let out = rank_candidates(&hits, 1, AUTO_LINK_THRESHOLD, AUTO_LINK_MAX);
        let ids: Vec<i64> = out.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![2, 4]); // strongest first, threshold applied
    }

    /// Above-threshold neighbours are sorted strongest-first and capped at max.
    #[test]
    fn rank_candidates_sorts_desc_and_truncates_to_max() {
        // 8 neighbours all above threshold; only the strongest AUTO_LINK_MAX kept.
        let hits: Vec<VectorHit> = (2..10)
            .map(|i| hit(i, Some(0.1 + (i as f32) * 0.01), i as usize))
            .collect();
        let out = rank_candidates(&hits, 1, AUTO_LINK_THRESHOLD, AUTO_LINK_MAX);
        assert_eq!(out.len(), AUTO_LINK_MAX);
        // Descending similarity order.
        for w in out.windows(2) {
            assert!(w[0].1 >= w[1].1, "not sorted desc: {out:?}");
        }
        // The strongest neighbour (smallest distance = id 2) must be first.
        assert_eq!(out[0].0, 2);
    }

    /// With no distance, similarity falls back to a rank-based approximation.
    #[test]
    fn rank_candidates_rank_fallback_when_distance_absent() {
        // No distance -> sim = 1 - rank/ANN_K. rank 0 -> 1.0 keep; rank 49 -> ~0.02 drop.
        let hits = vec![hit(2, None, 0), hit(3, None, 49)];
        let out = rank_candidates(&hits, 1, AUTO_LINK_THRESHOLD, AUTO_LINK_MAX);
        let ids: Vec<i64> = out.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![2]);
    }

    /// Valid blobs round-trip; empty or non-multiple-of-4 blobs are rejected.
    #[test]
    fn decode_embedding_roundtrips_and_rejects_garbage() {
        let v = [1.0f32, -2.5, 3.25];
        let mut blob = Vec::new();
        for f in v {
            blob.extend_from_slice(&f.to_le_bytes());
        }
        assert_eq!(decode_embedding(&blob), Some(v.to_vec()));
        assert_eq!(decode_embedding(&[]), None); // empty
        assert_eq!(decode_embedding(&[1, 2, 3]), None); // not a multiple of 4
    }

    /// ANN candidates must resolve to active graph-visible rows in the caller's
    /// database before they can consume an auto-link slot.
    #[tokio::test]
    async fn filter_linkable_hits_drops_stale_hidden_and_foreign_rows() {
        let db = crate::db::Database::connect_memory().await.expect("db");
        let visible = insert_memory(&db, 1, "approved", false, true, false).await;
        let archived = insert_memory(&db, 1, "approved", true, true, false).await;
        let superseded = insert_memory(&db, 1, "approved", false, false, false).await;
        let forgotten = insert_memory(&db, 1, "approved", false, true, true).await;
        let pending = insert_memory(&db, 1, "pending", false, true, false).await;
        let foreign = insert_memory(&db, 2, "approved", false, true, false).await;
        let missing = foreign + 10_000;

        let hits = vec![
            hit(archived, Some(0.01), 0),
            hit(superseded, Some(0.02), 1),
            hit(forgotten, Some(0.03), 2),
            hit(pending, Some(0.04), 3),
            hit(foreign, Some(0.05), 4),
            hit(missing, Some(0.06), 5),
            hit(visible, Some(0.07), 6),
        ];

        let filtered = filter_linkable_hits(&db, hits, 1)
            .await
            .expect("filter hits");
        assert_eq!(
            filtered
                .iter()
                .map(|entry| entry.memory_id)
                .collect::<Vec<_>>(),
            vec![visible]
        );
    }

    /// Existing structural edges do not satisfy associative connectivity:
    /// cite-only memories remain eligible, while a source with an outgoing
    /// similarity edge is omitted from the resumable batch.
    #[tokio::test]
    async fn select_link_candidates_targets_missing_similarity_not_missing_any_edge() {
        let db = crate::db::Database::connect_memory().await.expect("db");
        let cite_source = insert_memory(&db, 1, "approved", false, true, false).await;
        let cite_target = insert_memory(&db, 1, "approved", false, true, false).await;
        let similarity_source = insert_memory(&db, 1, "approved", false, true, false).await;
        let similarity_target = insert_memory(&db, 1, "approved", false, true, false).await;
        let stale_similarity_source = insert_memory(&db, 1, "approved", false, true, false).await;
        let pending = insert_memory(&db, 1, "pending", false, true, false).await;
        let archived = insert_memory(&db, 1, "approved", true, true, false).await;

        db.write(move |conn| {
            conn.execute(
                "INSERT INTO memory_links (source_id, target_id, similarity, type) \
                 VALUES (?1, ?2, 0.8, 'cite')",
                rusqlite::params![cite_source, cite_target],
            )?;
            conn.execute(
                "INSERT INTO memory_links (source_id, target_id, similarity, type) \
                 VALUES (?1, ?2, 0.9, 'similarity')",
                rusqlite::params![similarity_source, similarity_target],
            )?;
            conn.execute(
                "INSERT INTO memory_links (source_id, target_id, similarity, type) \
                 VALUES (?1, ?2, 0.9, 'similarity')",
                rusqlite::params![stale_similarity_source, archived],
            )?;
            Ok(())
        })
        .await
        .expect("insert test links");

        let candidates = select_link_candidates(&db, 1, 20)
            .await
            .expect("select candidates");
        let ids: HashSet<i64> = candidates.into_iter().map(|(id, _)| id).collect();

        assert!(
            ids.contains(&cite_source),
            "cite-only source remains eligible"
        );
        assert!(
            ids.contains(&cite_target),
            "cite-only target remains eligible"
        );
        assert!(
            !ids.contains(&similarity_source),
            "outgoing similarity source is already linked"
        );
        assert!(
            ids.contains(&similarity_target),
            "incoming-only rows are repaired into the normal bidirectional shape"
        );
        assert!(
            ids.contains(&stale_similarity_source),
            "a similarity edge to a hidden target does not satisfy visible connectivity"
        );
        assert!(!ids.contains(&pending), "pending memory stays review-gated");
        assert!(!ids.contains(&archived), "archived memory stays hidden");
    }
}
