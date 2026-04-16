use std::time::Instant;

use crate::brain::hopfield::edges::{self, EdgeType};
use crate::brain::hopfield::network::HopfieldNetwork;
use crate::brain::hopfield::pattern;
use crate::db::Database;
use crate::{EngError, Result};
use tracing::warn;

use super::StageReport;

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

/// Strength boost applied to the winner of a contradiction pair.
const WINNER_BOOST: f32 = 0.05;

/// Strength reduction applied to the loser of a contradiction pair.
const LOSER_PENALTY: f32 = 0.05;

/// Resolve contradictions by boosting the winner and weakening the loser.
///
/// Scans all `contradiction` edges. For each pair, the pattern with higher
/// current strength is the winner -- it receives a small boost. The loser
/// receives a small penalty. This gradually resolves conflicting memories
/// by reinforcing the dominant version.
///
/// Budget limits the number of contradiction edges processed per cycle.
pub async fn resolve(
    db: &Database,
    network: &mut HopfieldNetwork,
    user_id: i64,
    budget: u32,
) -> Result<StageReport> {
    let start = Instant::now();

    // Load all contradiction edges for this user
    let edge_type_str = EdgeType::Contradiction.to_string();
    let contradiction_pairs: Vec<(i64, i64)> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT source_id, target_id FROM brain_edges \
                     WHERE user_id = ?1 AND edge_type = ?2 \
                     ORDER BY weight DESC",
                )
                .map_err(rusqlite_to_eng_error)?;

            let pairs = stmt
                .query_map(rusqlite::params![user_id, edge_type_str], |row| {
                    let src: i64 = row.get(0)?;
                    let tgt: i64 = row.get(1)?;
                    Ok((src, tgt))
                })
                .map_err(rusqlite_to_eng_error)?
                .map(|r| r.map_err(rusqlite_to_eng_error))
                .collect::<Result<Vec<(i64, i64)>>>()?;

            Ok(pairs)
        })
        .await?;

    let items_processed = contradiction_pairs.len().min(budget as usize);
    let mut items_changed = 0usize;

    for (src, tgt) in contradiction_pairs.iter().take(budget as usize) {
        let s_src = match network.strength(*src) {
            Some(s) => s,
            None => continue,
        };
        let s_tgt = match network.strength(*tgt) {
            Some(s) => s,
            None => continue,
        };

        let (winner, loser, winner_strength, loser_strength) = if s_src >= s_tgt {
            (*src, *tgt, s_src, s_tgt)
        } else {
            (*tgt, *src, s_tgt, s_src)
        };

        // Boost winner
        let new_winner_strength =
            (winner_strength + WINNER_BOOST * (1.0 - winner_strength)).min(1.0);
        network.update_strength(winner, new_winner_strength);
        if let Err(e) = pattern::update_strength(db, winner, user_id, new_winner_strength).await {
            warn!(pattern_id = winner, user_id, error = %e, "resolve: failed to persist winner strength");
        }

        // Penalise loser
        let new_loser_strength = (loser_strength - LOSER_PENALTY).max(0.0);
        network.update_strength(loser, new_loser_strength);
        if let Err(e) = pattern::update_strength(db, loser, user_id, new_loser_strength).await {
            warn!(pattern_id = loser, user_id, error = %e, "resolve: failed to persist loser strength");
        }

        // Strengthen the contradiction edge itself -- it becomes more certain
        if let Err(e) =
            edges::strengthen_edge(db, winner, loser, EdgeType::Contradiction, 0.02, user_id).await
        {
            warn!(winner, loser, user_id, error = %e, "resolve: failed to strengthen contradiction edge");
        }

        items_changed += 1;
    }

    Ok(StageReport {
        stage: "resolve".to_string(),
        items_processed,
        items_changed,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}
