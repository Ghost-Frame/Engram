use std::time::Instant;

use crate::brain::hopfield::network::HopfieldNetwork;
use crate::brain::hopfield::recall;
use crate::db::Database;
use crate::Result;

use super::StageReport;

/// Remove patterns whose strength has fallen below the death threshold.
///
/// Wraps `recall::prune_weak` with the dream-cycle StageReport interface.
/// Dead patterns (strength < DEATH_THRESHOLD) are removed from both the
/// in-memory network and the database, along with any edges that reference
/// them.
#[tracing::instrument(skip(db, network), fields(user_id))]
pub async fn prune(
    db: &Database,
    network: &mut HopfieldNetwork,
    user_id: i64,
    _budget: u32,
) -> Result<StageReport> {
    let start = Instant::now();

    let before_count = network.pattern_count();
    let removed = recall::prune_weak(db, network, user_id, recall::DEATH_THRESHOLD).await?;

    Ok(StageReport {
        stage: "prune".to_string(),
        items_processed: before_count,
        items_changed: removed,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}
