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
use tracing::warn;

/// Minimum cosine similarity for an auto-link. Matches the pre-regression value.
pub const AUTO_LINK_THRESHOLD: f64 = 0.55;

/// Maximum neighbours linked per memory. Matches the pre-regression value.
pub const AUTO_LINK_MAX: usize = 6;

/// Nearest-neighbour fetch width before threshold/truncation are applied.
const ANN_K: usize = 50;

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
        let sim = match hit.distance {
            Some(d) => 1.0 - d as f64,
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
    // Primary path: the tenant's ANN index (cosine distance available). Falls
    // back to sqlite-vec, which is already user-scoped, when no index is loaded.
    let targets = if let Some(index) = db.vector_index.as_ref() {
        let hits = index.search(embedding, ANN_K).await.unwrap_or_default();
        rank_candidates(&hits, memory_id, AUTO_LINK_THRESHOLD, AUTO_LINK_MAX)
    } else {
        let hits = crate::memory::vector::vector_search(db, embedding, ANN_K, user_id).await?;
        // The fallback's VectorHit carries no distance, so rank approximation is
        // used; reuse the same ranking rules via a lightweight adapter.
        let adapted: Vec<VectorHit> = hits
            .iter()
            .map(|h| VectorHit {
                memory_id: h.memory_id,
                distance: h.distance,
                rank: h.rank,
            })
            .collect();
        rank_candidates(&adapted, memory_id, AUTO_LINK_THRESHOLD, AUTO_LINK_MAX)
    };

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

/// Link a throttled batch of the user's still-unlinked memories.
///
/// Selects up to `limit` active memories that have an embedding but appear in no
/// `memory_links` row, newest first (so freshly stored memories link promptly
/// and the historical backlog drains over subsequent calls), and links each.
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

    // Unlinked = present in neither endpoint of any link. memory_links only ever
    // connects same-user memories (enforced at insert), so the un-scoped subquery
    // is safe; the candidate set itself is user-scoped.
    let candidates: Vec<(i64, Vec<u8>)> = db
        .read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, embedding_vec_1024 \
                 FROM memories \
                 WHERE user_id = ?1 \
                   AND is_forgotten = 0 AND is_archived = 0 AND is_latest = 1 \
                   AND embedding_vec_1024 IS NOT NULL \
                   AND id NOT IN ( \
                       SELECT source_id FROM memory_links \
                       UNION \
                       SELECT target_id FROM memory_links \
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
        .await?;

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
}
