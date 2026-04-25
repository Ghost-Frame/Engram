use std::collections::HashMap;
use std::time::Instant;

use crate::brain::hopfield::edges::{self, EdgeType};
use crate::brain::hopfield::interference::{resolve_interference, PatternState};
use crate::brain::hopfield::network::HopfieldNetwork;
use crate::brain::hopfield::pattern;
use crate::brain::hopfield::recall::parse_datetime_approx;
use crate::db::Database;
use crate::{EngError, Result};
use tracing::warn;

use super::StageReport;

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

/// Resolve contradictions using full interference resolution.
///
/// Scans all `contradiction` edges. For each pair, computes effective
/// strength factoring in activation, decay, importance, and recency,
/// then boosts the winner and suppresses the loser.
///
/// Budget limits the number of contradiction edges processed per cycle.
#[tracing::instrument(skip(db, network), fields(user_id, budget))]
pub async fn resolve(
    db: &Database,
    network: &mut HopfieldNetwork,
    user_id: i64,
    budget: u32,
) -> Result<StageReport> {
    let start = Instant::now();

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

    if items_processed == 0 {
        return Ok(StageReport {
            stage: "resolve".to_string(),
            items_processed: 0,
            items_changed: 0,
            duration_ms: start.elapsed().as_millis() as u64,
        });
    }

    let db_patterns = pattern::list_patterns(db, user_id).await?;
    let pattern_map: HashMap<i64, &_> = db_patterns.iter().map(|p| (p.id, p)).collect();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();

    for (src, tgt) in contradiction_pairs.iter().take(budget as usize) {
        let s_src = match network.strength(*src) {
            Some(s) => s,
            None => continue,
        };
        let s_tgt = match network.strength(*tgt) {
            Some(s) => s,
            None => continue,
        };

        let src_pat = pattern_map.get(src);
        let tgt_pat = pattern_map.get(tgt);

        let src_importance = src_pat.map(|p| p.importance).unwrap_or(5);
        let tgt_importance = tgt_pat.map(|p| p.importance).unwrap_or(5);
        let src_decay = src_pat.map(|p| p.strength).unwrap_or(1.0);
        let tgt_decay = tgt_pat.map(|p| p.strength).unwrap_or(1.0);
        let src_age = src_pat
            .map(|p| ((now - parse_datetime_approx(&p.created_at)) / 86400.0) as f32)
            .unwrap_or(30.0);
        let tgt_age = tgt_pat
            .map(|p| ((now - parse_datetime_approx(&p.created_at)) / 86400.0) as f32)
            .unwrap_or(30.0);

        let src_state = PatternState {
            activation: s_src,
            decay_factor: src_decay,
            importance: src_importance,
            age_days: src_age,
        };
        let tgt_state = PatternState {
            activation: s_tgt,
            decay_factor: tgt_decay,
            importance: tgt_importance,
            age_days: tgt_age,
        };
        let (new_src, new_tgt, src_won) = resolve_interference(&src_state, &tgt_state);

        let (winner, loser) = if src_won {
            (*src, *tgt)
        } else {
            (*tgt, *src)
        };

        network.update_strength(*src, new_src);
        if let Err(e) = pattern::update_strength(db, *src, user_id, new_src).await {
            warn!(pattern_id = *src, user_id, error = %e, "resolve: failed to persist strength");
        }

        network.update_strength(*tgt, new_tgt);
        if let Err(e) = pattern::update_strength(db, *tgt, user_id, new_tgt).await {
            warn!(pattern_id = *tgt, user_id, error = %e, "resolve: failed to persist strength");
        }

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
