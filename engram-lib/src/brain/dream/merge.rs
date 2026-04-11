use std::time::Instant;

use crate::brain::hopfield::network::HopfieldNetwork;
use crate::brain::hopfield::recall;
use crate::db::Database;
use crate::Result;

use super::StageReport;

/// Merge highly similar patterns to reduce redundancy.
///
/// Wraps `recall::merge_similar` with the dream-cycle StageReport interface.
/// Pairs whose cosine similarity exceeds the merge threshold are collapsed --
/// the weaker pattern is removed and the stronger is boosted to the maximum
/// strength of the pair.
pub async fn merge(
    db: &Database,
    network: &mut HopfieldNetwork,
    user_id: i64,
    _budget: u32,
) -> Result<StageReport> {
    let start = Instant::now();

    let before_count = network.pattern_count();

    // 0.0 threshold defers to recall's built-in default (MERGE_SIMILARITY_THRESHOLD = 0.92)
    let merged_pairs = recall::merge_similar(db, network, user_id, 0.0).await?;

    let items_changed = merged_pairs.len();
    let items_processed = before_count;

    Ok(StageReport {
        stage: "merge".to_string(),
        items_processed,
        items_changed,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}
