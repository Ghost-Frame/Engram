use std::collections::HashSet;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::brain::hopfield::edges::{self, EdgeType};
use crate::brain::hopfield::network::{self, HopfieldNetwork};
use crate::brain::hopfield::pattern;
use crate::db::Database;
use crate::Result;

use super::StageReport;

/// Co-activation similarity threshold for creating new association edges.
/// Pairs above this threshold are connected if no edge yet exists.
const DISCOVER_SIM_THRESHOLD: f32 = 0.65;

/// Initial weight for newly discovered association edges.
const DISCOVER_EDGE_WEIGHT: f32 = 0.3;

/// Maximum candidate comparisons permitted for each possible new edge.
const DISCOVER_PAIR_CHECKS_PER_EDGE: usize = 64;

/// Return an order-independent key for an association between two patterns.
fn canonical_pair(id_a: i64, id_b: i64) -> (i64, i64) {
    if id_a <= id_b {
        (id_a, id_b)
    } else {
        (id_b, id_a)
    }
}

/// Mix a pattern id with the cycle seed to vary bounded scan coverage.
fn discovery_rank(pattern_id: i64, seed: u64) -> u64 {
    let mut value = (pattern_id as u64) ^ seed;
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

/// Find new cross-pattern connections by co-activation similarity.
///
/// Scans a bounded, per-cycle permutation of pattern pairs. When two patterns
/// have cosine similarity above DISCOVER_SIM_THRESHOLD and no existing
/// association edge, a new weak association edge is created. This models the
/// brain forming new associations during sleep consolidation based on shared
/// representational content.
///
/// Budget limits the maximum number of new edges created per cycle. Candidate
/// comparisons are separately capped at `budget * DISCOVER_PAIR_CHECKS_PER_EDGE`
/// so a tenant with many patterns cannot monopolize the dream cycle.
#[tracing::instrument(skip(db, network), fields(user_id, budget))]
pub async fn discover(
    db: &Database,
    network: &mut HopfieldNetwork,
    user_id: i64,
    budget: u32,
) -> Result<StageReport> {
    let start = Instant::now();

    let db_patterns = pattern::list_patterns(db, user_id).await?;
    if db_patterns.len() < 2 {
        return Ok(StageReport {
            stage: "discover".to_string(),
            items_processed: 0,
            items_changed: 0,
            duration_ms: start.elapsed().as_millis() as u64,
        });
    }

    let max_pair_checks = (budget as usize).saturating_mul(DISCOVER_PAIR_CHECKS_PER_EDGE);
    if max_pair_checks == 0 {
        return Ok(StageReport {
            stage: "discover".to_string(),
            items_processed: 0,
            items_changed: 0,
            duration_ms: start.elapsed().as_millis() as u64,
        });
    }

    // Build normalized vectors and vary their order so bounded scans cover
    // different pattern regions across repeated dream cycles.
    let cycle_seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
        ^ (user_id as u64).rotate_left(17);
    let mut normalized: Vec<(i64, Vec<f32>)> = db_patterns
        .iter()
        .map(|p| (p.id, network::l2_normalize(&p.pattern)))
        .collect();
    normalized.sort_unstable_by_key(|(pattern_id, _)| discovery_rank(*pattern_id, cycle_seed));

    // Load every existing association once. Canonical keys prevent a reverse
    // edge from being duplicated when the per-cycle permutation changes.
    let association_type = EdgeType::Association.to_string();
    let mut existing_pairs: HashSet<(i64, i64)> = db
        .read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT source_id, target_id FROM brain_edges \
                 WHERE user_id = ?1 AND edge_type = ?2",
            )?;
            let rows = stmt.query_map(rusqlite::params![user_id, association_type], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })?;
            let mut pairs = HashSet::new();
            for row in rows {
                let (source_id, target_id) = row?;
                pairs.insert(canonical_pair(source_id, target_id));
            }
            Ok(pairs)
        })
        .await?;

    let mut pairs_checked = 0usize;
    let mut items_changed = 0usize;
    let mut budget_remaining = budget as usize;

    // Pairwise scan with a strict comparison ceiling independent of n².
    'outer: for i in 0..normalized.len() {
        for j in (i + 1)..normalized.len() {
            if budget_remaining == 0 || pairs_checked == max_pair_checks {
                break 'outer;
            }

            let id_a = normalized[i].0;
            let id_b = normalized[j].0;
            pairs_checked += 1;

            // Check whether either pattern is still alive in the network.
            if network.strength(id_a).is_none() || network.strength(id_b).is_none() {
                continue;
            }

            let sim = network::cosine_similarity(&normalized[i].1, &normalized[j].1);
            if sim < DISCOVER_SIM_THRESHOLD {
                continue;
            }

            let pair = canonical_pair(id_a, id_b);
            if existing_pairs.contains(&pair) {
                continue;
            }

            edges::store_edge(
                db,
                pair.0,
                pair.1,
                DISCOVER_EDGE_WEIGHT,
                EdgeType::Association,
                user_id,
            )
            .await?;
            existing_pairs.insert(pair);
            items_changed += 1;
            budget_remaining -= 1;
        }
    }

    Ok(StageReport {
        stage: "discover".to_string(),
        items_processed: pairs_checked,
        items_changed,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}
