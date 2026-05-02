use std::time::Instant;

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

/// Find new cross-pattern connections by co-activation similarity.
///
/// Scans all pattern pairs. When two patterns have cosine similarity above
/// DISCOVER_SIM_THRESHOLD and no existing association edge, a new weak
/// association edge is created. This models the brain forming new
/// associations during sleep consolidation based on shared representational
/// content.
///
/// Budget limits the maximum number of new edges created per cycle.
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
            items_processed: db_patterns.len(),
            items_changed: 0,
            duration_ms: start.elapsed().as_millis() as u64,
        });
    }

    // Build normalized vectors for all patterns
    let normalized: Vec<(i64, Vec<f32>)> = db_patterns
        .iter()
        .map(|p| (p.id, network::l2_normalize(&p.pattern)))
        .collect();

    let items_processed = normalized.len();
    let mut items_changed = 0usize;
    let mut budget_remaining = budget as usize;

    // Pairwise scan -- O(n^2) but budget-bounded
    'outer: for i in 0..normalized.len() {
        for j in (i + 1)..normalized.len() {
            if budget_remaining == 0 {
                break 'outer;
            }

            let id_a = normalized[i].0;
            let id_b = normalized[j].0;

            let sim = network::cosine_similarity(&normalized[i].1, &normalized[j].1);
            if sim < DISCOVER_SIM_THRESHOLD {
                continue;
            }

            // Check whether either pattern is still alive in the network
            if network.strength(id_a).is_none() || network.strength(id_b).is_none() {
                continue;
            }

            // Look up existing edges to avoid duplicating
            let existing = edges::get_edges_from(db, id_a, user_id).await?;
            let already_connected = existing
                .iter()
                .any(|e| e.target_id == id_b && e.edge_type == EdgeType::Association);

            if !already_connected {
                edges::store_edge(
                    db,
                    id_a,
                    id_b,
                    DISCOVER_EDGE_WEIGHT,
                    EdgeType::Association,
                    user_id,
                )
                .await?;
                items_changed += 1;
                budget_remaining -= 1;
            }
        }
    }

    Ok(StageReport {
        stage: "discover".to_string(),
        items_processed,
        items_changed,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}
