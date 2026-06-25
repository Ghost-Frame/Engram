use crate::memory::types::SearchResult;

use super::retrievability_with_w20;
use super::FSRS6_WEIGHTS;

/// One memory ranked for spaced-repetition review, pairing its search relevance with its
/// current FSRS retrievability so fading-but-relevant memories rise to the top.
pub struct RecallDueEntry {
    /// Row id of the memory.
    pub memory_id: i64,
    /// Memory content (copied for the recall-due response).
    pub content: String,
    /// Current FSRS retrievability in [0, 1] (probability of recall right now).
    pub retrievability: f32,
    /// The memory's original hybrid-search score.
    pub original_score: f64,
    /// Combined score: relevance weighted by how close the memory is to fading.
    pub recall_due_score: f64,
}

/// Re-rank search results for the recall-due surface, boosting memories that are both
/// relevant and close to fading (low retrievability), so review effort targets the
/// memories most at risk of being forgotten.
pub fn rerank_by_retrievability(results: &[SearchResult], w20: Option<f32>) -> Vec<RecallDueEntry> {
    let w20 = w20.unwrap_or(FSRS6_WEIGHTS[20]);
    let now_ms = chrono::Utc::now().timestamp_millis();

    let mut entries: Vec<RecallDueEntry> = results
        .iter()
        .map(|r| {
            // recall-due-fallback: a memory with no recorded FSRS stability is exactly the
            // one most in need of reinforcement, so it must not be dropped. Fall back to
            // the same default_stability the decay path uses (derived from access and
            // source counts) instead of skipping the row.
            let stability = r
                .memory
                .fsrs_stability
                .map(|s| s as f32)
                .unwrap_or_else(|| {
                    crate::fsrs::decay::default_stability(
                        r.memory.access_count,
                        r.memory.source_count,
                    )
                });
            // recall-due-fallback: a memory that was never reviewed (no fsrs_last_review_at)
            // is maximally due, not freshly created. Using created_at here would make a
            // brand-new memory look just-reviewed (retrievability ~ 1) and hide it from the
            // recall-due surface on the day it was stored, so treat never-reviewed as long
            // past instead, consistent with the default_stability fallback above.
            let elapsed_days = match r.memory.fsrs_last_review_at.as_deref() {
                Some(ts) => elapsed_days_from(ts, now_ms),
                None => NEVER_REVIEWED_ELAPSED_DAYS,
            };
            let ret = retrievability_with_w20(stability, elapsed_days, w20);

            // recall-due score: boost memories that are (a) relevant (high search
            // score) and (b) about to fade (low retrievability). The product
            // balances both: a memory with 0.01 search score isn't useful even if
            // it's fading, and a memory at R=0.99 doesn't need reinforcement.
            //
            // Invert retrievability so lower R -> higher boost.
            let fade_boost = 1.0 - ret as f64;
            let recall_due_score = r.score * (0.3 + 0.7 * fade_boost);

            RecallDueEntry {
                memory_id: r.memory.id,
                content: r.memory.content.clone(),
                retrievability: ret,
                original_score: r.score,
                recall_due_score,
            }
        })
        .collect();

    entries.sort_by(|a, b| {
        b.recall_due_score
            .partial_cmp(&a.recall_due_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    entries
}

/// Elapsed days for an unparseable timestamp. An unparseable date must not look freshly
/// reviewed (elapsed 0 = high retrievability = never surfaced as due); treat it as long
/// past so the memory is flagged for reinforcement.
const UNPARSEABLE_ELAPSED_DAYS: f32 = 365.0;

/// Elapsed days for a memory that has never been reviewed. A never-reviewed memory is the one
/// most in need of reinforcement, so it must surface as due rather than look freshly created;
/// treat it as long past, matching the unparseable-timestamp fallback.
const NEVER_REVIEWED_ELAPSED_DAYS: f32 = 365.0;

/// Days between a review timestamp and now, clamped to be non-negative.
fn elapsed_days_from(date_str: &str, now_ms: i64) -> f32 {
    let normalized = if date_str.contains('Z') {
        date_str.to_string()
    } else {
        format!("{}Z", date_str.replace(' ', "T"))
    };
    let ref_ms = match normalized.parse::<chrono::DateTime<chrono::Utc>>() {
        Ok(dt) => dt.timestamp_millis(),
        Err(e) => {
            tracing::warn!(
                "recall-due: unparseable fsrs timestamp {date_str:?}: {e}; treating as long past"
            );
            return UNPARSEABLE_ELAPSED_DAYS;
        }
    };
    // Clamp at zero: chrono Utc::now and SQLite datetime('now') can differ slightly, and a
    // future-dated review must not yield a negative elapsed (which would push
    // retrievability above 1).
    (((now_ms - ref_ms) as f32) / (1000.0 * 60.0 * 60.0 * 24.0)).max(0.0)
}
