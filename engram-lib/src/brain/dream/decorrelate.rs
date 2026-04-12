use std::time::Instant;

use crate::brain::hopfield::network::{self, HopfieldNetwork};
use crate::brain::hopfield::pattern;
use crate::db::Database;
use crate::{EngError, Result};

use super::StageReport;

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

/// Similarity threshold above which two patterns are considered redundantly
/// correlated and eligible for edge weight reduction.
const DECORRELATE_SIM_THRESHOLD: f32 = 0.80;

/// Fractional reduction applied to edge weights between highly-correlated
/// patterns. weight *= (1 - DECORRELATE_RATE).
const DECORRELATE_RATE: f32 = 0.10;

/// Reduce edge weights between highly similar (redundant) patterns.
///
/// When two patterns share high cosine similarity, any edges between them
/// carry redundant information -- the patterns already encode the same
/// content. Reducing those edge weights frees capacity for more
/// informative, lower-similarity connections.
///
/// Budget limits the number of pattern pairs examined.
pub async fn decorrelate(
    db: &Database,
    network: &mut HopfieldNetwork,
    user_id: i64,
    budget: u32,
) -> Result<StageReport> {
    let start = Instant::now();

    let db_patterns = pattern::list_patterns(db, user_id).await?;
    if db_patterns.len() < 2 {
        return Ok(StageReport {
            stage: "decorrelate".to_string(),
            items_processed: db_patterns.len(),
            items_changed: 0,
            duration_ms: start.elapsed().as_millis() as u64,
        });
    }

    let normalized: Vec<(i64, Vec<f32>)> = db_patterns
        .iter()
        .map(|p| (p.id, network::l2_normalize(&p.pattern)))
        .collect();

    let items_processed = normalized.len();
    let mut items_changed = 0usize;
    let mut budget_remaining = budget as usize;

    'outer: for i in 0..normalized.len() {
        for j in (i + 1)..normalized.len() {
            if budget_remaining == 0 {
                break 'outer;
            }

            let id_a = normalized[i].0;
            let id_b = normalized[j].0;

            // Only process patterns still alive in the network
            if network.strength(id_a).is_none() || network.strength(id_b).is_none() {
                continue;
            }

            let sim = network::cosine_similarity(&normalized[i].1, &normalized[j].1);
            if sim < DECORRELATE_SIM_THRESHOLD {
                continue;
            }

            // Apply a fractional decay to edges between this pair in both directions
            let decay_rate = 1.0 - DECORRELATE_RATE;
            let affected = db
                .write(move |conn| {
                    let affected_ab = conn
                        .execute(
                            "UPDATE brain_edges \
                             SET weight = weight * ?1 \
                             WHERE source_id = ?2 AND target_id = ?3 AND user_id = ?4",
                            rusqlite::params![decay_rate as f64, id_a, id_b, user_id],
                        )
                        .map_err(rusqlite_to_eng_error)?;
                    let affected_ba = conn
                        .execute(
                            "UPDATE brain_edges \
                             SET weight = weight * ?1 \
                             WHERE source_id = ?2 AND target_id = ?3 AND user_id = ?4",
                            rusqlite::params![decay_rate as f64, id_b, id_a, user_id],
                        )
                        .map_err(rusqlite_to_eng_error)?;
                    Ok(affected_ab + affected_ba)
                })
                .await?;

            if affected > 0 {
                items_changed += 1;
            }

            budget_remaining -= 1;
        }
    }

    Ok(StageReport {
        stage: "decorrelate".to_string(),
        items_processed,
        items_changed,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}
