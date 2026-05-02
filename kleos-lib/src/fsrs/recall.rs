use crate::memory::types::SearchResult;

use super::retrievability_with_w20;
use super::FSRS6_WEIGHTS;

pub struct RecallDueEntry {
    pub memory_id: i64,
    pub content: String,
    pub retrievability: f32,
    pub original_score: f64,
    pub recall_due_score: f64,
}

pub fn rerank_by_retrievability(results: &[SearchResult], w20: Option<f32>) -> Vec<RecallDueEntry> {
    let w20 = w20.unwrap_or(FSRS6_WEIGHTS[20]);
    let now_ms = chrono::Utc::now().timestamp_millis();

    let mut entries: Vec<RecallDueEntry> = results
        .iter()
        .filter_map(|r| {
            let stability = r.memory.fsrs_stability? as f32;
            let last_review = r
                .memory
                .fsrs_last_review_at
                .as_deref()
                .or(Some(&r.memory.created_at))?;

            let elapsed_days = elapsed_days_from(last_review, now_ms);
            let ret = retrievability_with_w20(stability, elapsed_days, w20);

            // recall-due score: boost memories that are (a) relevant (high search
            // score) and (b) about to fade (low retrievability). The product
            // balances both: a memory with 0.01 search score isn't useful even if
            // it's fading, and a memory at R=0.99 doesn't need reinforcement.
            //
            // Invert retrievability so lower R -> higher boost.
            let fade_boost = 1.0 - ret as f64;
            let recall_due_score = r.score * (0.3 + 0.7 * fade_boost);

            Some(RecallDueEntry {
                memory_id: r.memory.id,
                content: r.memory.content.clone(),
                retrievability: ret,
                original_score: r.score,
                recall_due_score,
            })
        })
        .collect();

    entries.sort_by(|a, b| {
        b.recall_due_score
            .partial_cmp(&a.recall_due_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    entries
}

fn elapsed_days_from(date_str: &str, now_ms: i64) -> f32 {
    let normalized = if date_str.contains('Z') {
        date_str.to_string()
    } else {
        format!("{}Z", date_str.replace(' ', "T"))
    };
    let ref_ms = normalized
        .parse::<chrono::DateTime<chrono::Utc>>()
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(now_ms);
    ((now_ms - ref_ms) as f32) / (1000.0 * 60.0 * 60.0 * 24.0)
}
