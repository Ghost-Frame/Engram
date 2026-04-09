use chrono::Utc;

use super::{initial_stability, retrievability, Rating, FSRS_MIN_STABILITY};

/// Parse a date string (with or without trailing Z) into milliseconds since epoch.
/// Accepts formats like "2025-01-01 00:00:00" or "2025-01-01T00:00:00Z".
fn parse_ms(s: &str) -> Option<i64> {
    // Normalize: replace space with T, ensure Z suffix for UTC parse
    let normalized = if s.contains('Z') {
        s.to_string()
    } else {
        // Assume UTC if no timezone marker
        format!("{}Z", s.replace(' ', "T"))
    };
    normalized
        .parse::<chrono::DateTime<chrono::Utc>>()
        .ok()
        .map(|dt| dt.timestamp_millis())
}

/// Compute a decayed relevance score matching TypeScript calculateDecayScore.
pub fn calculate_decay_score(
    importance: f32,
    created_at: &str,
    access_count: i32,
    last_accessed_at: Option<&str>,
    is_static: bool,
    source_count: i32,
    stability: Option<f32>,
) -> f32 {
    if is_static {
        return importance;
    }

    let now_ms = Utc::now().timestamp_millis();
    let ref_str = last_accessed_at.unwrap_or(created_at);

    let ref_ms = match parse_ms(ref_str) {
        Some(ms) => ms,
        None => return importance * 0.5,
    };

    let elapsed_days = (now_ms - ref_ms) as f32 / (1000.0 * 60.0 * 60.0 * 24.0);

    let effective_stability = if let Some(s) = stability {
        if s > 0.0 {
            s
        } else {
            default_stability(access_count, source_count)
        }
    } else {
        default_stability(access_count, source_count)
    };

    let r = retrievability(effective_stability, elapsed_days);
    let result = importance * r;
    if result.is_finite() {
        result
    } else {
        importance * 0.5
    }
}

fn default_stability(access_count: i32, source_count: i32) -> f32 {
    let base = initial_stability(Rating::Good);
    let access_bonus = f32::min(access_count as f32 * 0.3, 3.0);
    let source_bonus = f32::min((source_count - 1) as f32 * 0.2, 1.0);
    f32::max(FSRS_MIN_STABILITY, base * (1.0 + access_bonus + source_bonus))
}
