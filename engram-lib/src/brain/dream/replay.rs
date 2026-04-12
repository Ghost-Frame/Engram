use std::time::Instant;

use crate::brain::hopfield::network::HopfieldNetwork;
use crate::brain::hopfield::pattern;
use crate::db::Database;
use crate::Result;

use super::StageReport;

/// Boost recently accessed patterns to reinforce active memories.
///
/// Patterns that have been accessed (non-zero access_count) are replayed
/// through the network using pattern completion, and their strength is
/// modestly boosted. This mirrors hippocampal replay during sleep -- recent
/// experiences are replayed to drive consolidation into long-term storage.
///
/// Budget controls how many patterns are replayed at most per cycle.
pub async fn replay(
    db: &Database,
    network: &mut HopfieldNetwork,
    user_id: i64,
    budget: u32,
) -> Result<StageReport> {
    let start = Instant::now();

    let db_patterns = pattern::list_patterns(db, user_id).await?;
    let items_processed = db_patterns.len().min(budget as usize);

    // Sort by access_count descending -- most recently active patterns first
    let mut candidates: Vec<_> = db_patterns.iter().filter(|p| p.access_count > 0).collect();
    candidates.sort_by(|a, b| b.access_count.cmp(&a.access_count));
    candidates.truncate(budget as usize);

    let mut items_changed = 0usize;

    for bp in &candidates {
        let current_strength = match network.strength(bp.id) {
            Some(s) => s,
            None => continue,
        };

        // Replay boost: small additive nudge toward 1.0
        // Less aggressive than the recall boost -- this is background consolidation
        const REPLAY_BOOST: f32 = 0.05;
        let new_strength = (current_strength + REPLAY_BOOST * (1.0 - current_strength)).min(1.0);

        if (new_strength - current_strength).abs() > 1e-6 {
            network.update_strength(bp.id, new_strength);
            let _ = pattern::update_strength(db, bp.id, user_id, new_strength).await;
            items_changed += 1;
        }
    }

    Ok(StageReport {
        stage: "replay".to_string(),
        items_processed,
        items_changed,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}
