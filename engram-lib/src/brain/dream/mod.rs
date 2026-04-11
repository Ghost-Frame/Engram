/// Dream cycle -- consolidation processing for the Hopfield substrate.
///
/// During a dream cycle, the network runs through 6 sequential stages that
/// mirror sleep-phase memory consolidation in biological systems:
///
/// 1. **replay** -- Strengthen recently-activated patterns by replaying them.
/// 2. **merge** -- Collapse highly similar patterns into a single stronger one.
/// 3. **prune** -- Remove patterns whose strength has fallen below threshold.
/// 4. **discover** -- Find new cross-pattern connections via co-activation.
/// 5. **decorrelate** -- Reduce redundant edge weights between similar patterns.
/// 6. **resolve** -- Resolve contradictions by boosting the winner.
///
/// Each stage returns a `StageReport` summarising what it did.
/// The driver `run_dream_cycle` runs all 6 stages and persists a run record
/// to the `brain_dream_runs` table.

pub mod decorrelate;
pub mod discover;
pub mod merge;
pub mod prune;
pub mod replay;
pub mod resolve;

#[cfg(test)]
mod tests;

use crate::brain::hopfield::network::HopfieldNetwork;
use crate::db::Database;
use crate::{EngError, Result};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Report types
// ---------------------------------------------------------------------------

/// Per-stage summary returned by each dream-cycle stage function.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageReport {
    pub stage: String,
    pub items_processed: usize,
    pub items_changed: usize,
    pub duration_ms: u64,
}

/// Combined result of a full dream cycle run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamCycleResult {
    pub user_id: i64,
    pub run_id: i64,
    pub stages: Vec<StageReport>,
    pub total_duration_ms: u64,
}

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

/// Run a complete dream cycle for a user's Hopfield network.
///
/// Stages run in order: replay -> merge -> prune -> discover ->
/// decorrelate -> resolve. A run record is inserted into
/// `brain_dream_runs` before the stages begin and updated with counts
/// and finish time when all stages complete.
pub async fn run_dream_cycle(
    db: &Database,
    network: &mut HopfieldNetwork,
    user_id: i64,
    budget: u32,
) -> Result<DreamCycleResult> {
    let cycle_start = std::time::Instant::now();

    // Insert run record to get the run_id
    let run_id = insert_dream_run(db, user_id).await?;

    let mut stages = Vec::with_capacity(6);

    // Stage 1: replay
    let report = replay::replay(db, network, user_id, budget).await?;
    stages.push(report);

    // Stage 2: merge
    let report = merge::merge(db, network, user_id, budget).await?;
    stages.push(report);

    // Stage 3: prune
    let report = prune::prune(db, network, user_id, budget).await?;
    stages.push(report);

    // Stage 4: discover
    let report = discover::discover(db, network, user_id, budget).await?;
    stages.push(report);

    // Stage 5: decorrelate
    let report = decorrelate::decorrelate(db, network, user_id, budget).await?;
    stages.push(report);

    // Stage 6: resolve
    let report = resolve::resolve(db, network, user_id, budget).await?;
    stages.push(report);

    let total_duration_ms = cycle_start.elapsed().as_millis() as u64;

    // Extract counts from each stage by name for the summary row
    let counts = StageCounts::from_reports(&stages);

    // Update the run record with finish time and counts
    finish_dream_run(db, run_id, user_id, &counts).await?;

    Ok(DreamCycleResult {
        user_id,
        run_id,
        stages,
        total_duration_ms,
    })
}

// ---------------------------------------------------------------------------
// Persistence helpers
// ---------------------------------------------------------------------------

struct StageCounts {
    replay_count: usize,
    merge_count: usize,
    prune_count: usize,
    discover_count: usize,
    decorrelate_count: usize,
    resolve_count: usize,
}

impl StageCounts {
    fn from_reports(reports: &[StageReport]) -> Self {
        let get = |name: &str| {
            reports
                .iter()
                .find(|r| r.stage == name)
                .map(|r| r.items_changed)
                .unwrap_or(0)
        };
        StageCounts {
            replay_count: get("replay"),
            merge_count: get("merge"),
            prune_count: get("prune"),
            discover_count: get("discover"),
            decorrelate_count: get("decorrelate"),
            resolve_count: get("resolve"),
        }
    }
}

/// Insert a new brain_dream_runs row, returning the new run_id.
async fn insert_dream_run(db: &Database, user_id: i64) -> Result<i64> {
    db.conn
        .execute(
            "INSERT INTO brain_dream_runs \
             (user_id, started_at, replay_count, merge_count, prune_count, \
              discover_count, decorrelate_count, resolve_count) \
             VALUES (?1, datetime('now'), 0, 0, 0, 0, 0, 0)",
            libsql::params![user_id],
        )
        .await?;

    let mut rows = db
        .conn
        .query("SELECT last_insert_rowid()", ())
        .await?;

    match rows.next().await? {
        Some(row) => {
            let id: i64 = row.get(0)?;
            Ok(id)
        }
        None => Err(EngError::Internal(
            "failed to retrieve dream run id".to_string(),
        )),
    }
}

/// Update an existing brain_dream_runs row with final counts and finish time.
async fn finish_dream_run(
    db: &Database,
    run_id: i64,
    user_id: i64,
    counts: &StageCounts,
) -> Result<()> {
    db.conn
        .execute(
            "UPDATE brain_dream_runs SET \
             finished_at = datetime('now'), \
             replay_count = ?1, \
             merge_count = ?2, \
             prune_count = ?3, \
             discover_count = ?4, \
             decorrelate_count = ?5, \
             resolve_count = ?6 \
             WHERE id = ?7 AND user_id = ?8",
            libsql::params![
                counts.replay_count as i64,
                counts.merge_count as i64,
                counts.prune_count as i64,
                counts.discover_count as i64,
                counts.decorrelate_count as i64,
                counts.resolve_count as i64,
                run_id,
                user_id
            ],
        )
        .await?;
    Ok(())
}
