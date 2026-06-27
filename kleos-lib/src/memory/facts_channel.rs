//! L5 facts retrieval channel: `structured_facts` as an RRF channel in hybrid search.
//!
//! Extracted SVO facts already exist (enrichment-only today). This channel lets a query match
//! the fact text (subject/predicate/object/verb via the `facts_fts` FTS5 index) and feed the
//! parent memories into the same RRF fusion as the vector/FTS/graph channels, so a fact-shaped
//! query surfaces the memory the fact was distilled from even when the surface wording differs.
//!
//! ## Tenant isolation
//!
//! `structured_facts.user_id` was re-added with `DEFAULT 1` and is correctly owned on tenant
//! shards (backfilled at v67) but could be mis-attributed on a legacy multi-user monolith. The
//! roadmap calls for a "refuse to enable while mis-scoped rows remain" guard. We do something
//! stronger and always-on: every query JOINs the parent memory and requires BOTH the fact and
//! the memory to belong to the caller (`sf.user_id = ? AND m.user_id = ?`). A mis-scoped fact
//! (`sf.user_id != m.user_id`) then matches *neither* user and can never leak -- per-row
//! fail-closed isolation rather than a global precondition. [`misscoped_facts_count`] remains
//! for ops/observability.
//!
//! ## Current truth
//!
//! Only current facts (`invalid_at IS NULL`) over visible memories (`is_latest = 1`, not
//! forgotten/archived) participate, and results are resolved to one hit per
//! `(subject, predicate)` in relevance order. So "where does X live now" rides the current
//! fact, not a superseded one.

use crate::db::Database;
use crate::Result;
use rusqlite::params;
use std::collections::HashSet;

/// One current fact matched for a query, mapped back to the memory it was extracted from.
#[derive(Debug, Clone)]
pub struct FactHit {
    /// The parent memory the fact was distilled from (the RRF candidate key).
    pub memory_id: i64,
    /// The stored fact confidence in [0, 1], used to scale the channel's RRF contribution.
    pub confidence: f64,
}

/// FTS-search current `structured_facts` and resolve to the best current fact per
/// `(subject, predicate)`, returning parent memory ids in relevance (BM25) order.
///
/// `match_query` is an already-sanitized FTS5 MATCH expression (e.g. from
/// `memory::fts::fts_or_match_query`). Empty input yields no hits (not an error).
pub async fn search_facts_fts(
    db: &Database,
    match_query: &str,
    user_id: i64,
    limit: usize,
) -> Result<Vec<FactHit>> {
    if match_query.is_empty() || limit == 0 {
        return Ok(vec![]);
    }
    let mq = match_query.to_string();
    // Over-fetch so the per-(subject,predicate) and per-memory dedup below can still yield up
    // to `limit` distinct memories; bounded so a hot query cannot scan unboundedly.
    let fetch = limit.saturating_mul(3).clamp(limit, 200) as i64;
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT m.id, sf.subject, sf.predicate, sf.confidence \
             FROM facts_fts f \
             JOIN structured_facts sf ON sf.id = f.rowid \
             JOIN memories m ON m.id = sf.memory_id \
             WHERE facts_fts MATCH ?1 \
               AND sf.user_id = ?2 AND m.user_id = ?2 \
               AND sf.invalid_at IS NULL \
               AND m.is_latest = 1 AND m.is_forgotten = 0 AND m.is_archived = 0 \
             ORDER BY bm25(facts_fts) \
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![mq, user_id, fetch], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, f64>(3)?,
            ))
        })?;

        // Current-truth resolution: walk in relevance order, keep the first (best-ranked) fact
        // per (subject, predicate) and the first hit per memory, so near-duplicate facts about
        // the same attribute collapse to one and a memory is not double-counted.
        let mut seen_sp: HashSet<(String, String)> = HashSet::new();
        let mut seen_mem: HashSet<i64> = HashSet::new();
        let mut hits: Vec<FactHit> = Vec::new();
        for r in rows {
            let (memory_id, subject, predicate, confidence) = r?;
            if !seen_sp.insert((subject.to_lowercase(), predicate.to_lowercase())) {
                continue;
            }
            if !seen_mem.insert(memory_id) {
                continue;
            }
            hits.push(FactHit {
                memory_id,
                confidence: confidence.clamp(0.0, 1.0),
            });
            if hits.len() >= limit {
                break;
            }
        }
        Ok(hits)
    })
    .await
}

/// Count `structured_facts` rows whose owner disagrees with their parent memory's owner.
///
/// The query-time isolation in [`search_facts_fts`] already makes such rows invisible, so a
/// non-zero result is a data-hygiene signal (e.g. legacy monolith `DEFAULT 1` rows), not a
/// leak. Exposed for ops dashboards, a startup warning, and tests.
pub async fn misscoped_facts_count(db: &Database) -> Result<i64> {
    db.read(|conn| {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM structured_facts sf JOIN memories m ON m.id = sf.memory_id \
             WHERE sf.user_id != m.user_id",
            [],
            |row| row.get(0),
        )?;
        Ok(n)
    })
    .await
}
