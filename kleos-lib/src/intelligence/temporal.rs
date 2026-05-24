// ============================================================================
// BI-TEMPORAL FACT TRACKING + CONTRADICTION DETECTION
// Facts have valid_at/invalid_at windows. Old facts are never deleted, just
// invalidated. New contradicting facts auto-invalidate predecessors on the
// same subject+verb.
// ============================================================================

use super::types::{FactContradiction, TemporalPattern, TimeTravelResult};
use crate::db::Database;
use crate::{EngError, Result};
use rusqlite::{params, OptionalExtension};
use tracing::{info, warn};

/// Convert a rusqlite error into the crate's canonical error type.
fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

// ============================================================================
// PATTERN DETECTION CONSTANTS
// ============================================================================

/// Maximum number of memories scanned per `detect_patterns` call, to bound
/// the dreamer hot path.
const DETECT_SCAN_LIMIT: i64 = 5_000;

/// Minimum number of memories in a category before pattern detection runs.
const MIN_SAMPLE_SIZE: usize = 5;

/// Maximum number of memory ids stored per pattern.
const MAX_MEMORY_IDS: usize = 50;

/// Confidence threshold: `stddev / mean` must be below this value to emit a
/// pattern. Values at or above this threshold are filtered as noise.
const STDDEV_RATIO_THRESHOLD: f64 = 0.3;

/// Tolerance window (in seconds) around the daily bucket centre (86 400 s).
const DAILY_TOLERANCE_SECS: f64 = 7_200.0;

/// Tolerance window (in seconds) around the weekly bucket centre (604 800 s).
const WEEKLY_TOLERANCE_SECS: f64 = 86_400.0;

/// Tolerance window (in seconds) around the monthly bucket centre (2 592 000 s).
const MONTHLY_TOLERANCE_SECS: f64 = 432_000.0;

// ============================================================================
// TEMPORAL PATTERN DETECTION
// ============================================================================

/// Detect recurring temporal patterns across all non-forgotten memories for
/// the given user, then persist each pattern via `store_pattern`.
///
/// Algorithm (category-grouped inter-arrival stddev):
/// 1. Read `created_at` timestamps + ids for up to `DETECT_SCAN_LIMIT`
///    non-forgotten memories, grouped by `category`.
/// 2. For each category with >= `MIN_SAMPLE_SIZE` memories, sort timestamps,
///    compute inter-arrival deltas in seconds, then mean and stddev.
/// 3. If `stddev / mean < STDDEV_RATIO_THRESHOLD` and the mean falls within a
///    recognisable bucket (daily / weekly / monthly), emit one `TemporalPattern`.
/// 4. Each emitted pattern is persisted via `store_pattern` before returning,
///    so the dreamer's call materialises rows in `temporal_patterns`.
///
/// Returns only PRECISE patterns (high confidence) -- noisy categories are
/// silently skipped to avoid hallucinated recurrence claims.
#[tracing::instrument(skip(db), fields(user_id))]
pub async fn detect_patterns(db: &Database, user_id: i64) -> Result<Vec<TemporalPattern>> {
    // --- 1. Load timestamps grouped by category ---
    let rows: Vec<(i64, String, String)> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, category, created_at \
                     FROM memories \
                     WHERE is_forgotten = 0 \
                     ORDER BY category, created_at \
                     LIMIT ?1",
                )
                .map_err(rusqlite_to_eng_error)?;

            let iter = stmt
                .query_map(params![DETECT_SCAN_LIMIT], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(rusqlite_to_eng_error)?;

            iter.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(rusqlite_to_eng_error)
        })
        .await?;

    // --- 2. Group by category ---
    use std::collections::HashMap;
    let mut by_category: HashMap<String, Vec<(i64, i64)>> = HashMap::new();
    for (id, category, created_at_str) in rows {
        let ts = parse_sqlite_timestamp(&created_at_str);
        by_category.entry(category).or_default().push((id, ts));
    }

    // --- 3. Analyse each category ---
    let mut patterns: Vec<TemporalPattern> = Vec::new();

    for (category, mut entries) in by_category {
        if entries.len() < MIN_SAMPLE_SIZE {
            continue;
        }

        // Sort by timestamp (should already be ordered by the query, but guarantee it)
        entries.sort_by_key(|&(_, ts)| ts);

        let ids: Vec<i64> = entries.iter().map(|(id, _)| *id).collect();
        let timestamps: Vec<i64> = entries.iter().map(|(_, ts)| *ts).collect();

        // Compute inter-arrival deltas in seconds
        let deltas: Vec<f64> = timestamps
            .windows(2)
            .map(|w| (w[1] - w[0]) as f64)
            .filter(|&d| d >= 0.0)
            .collect();

        if deltas.is_empty() {
            continue;
        }

        let mean = deltas.iter().sum::<f64>() / deltas.len() as f64;
        if mean <= 0.0 {
            // Degenerate: all same timestamp. Division guard.
            continue;
        }

        let variance =
            deltas.iter().map(|&d| (d - mean).powi(2)).sum::<f64>() / deltas.len() as f64;
        let stddev = variance.sqrt();
        let ratio = stddev / mean;

        if ratio >= STDDEV_RATIO_THRESHOLD {
            // Too noisy -- skip
            continue;
        }

        // Classify into a bucket
        let (pattern_type, recurrence, bucket_centre) = classify_bucket(mean);
        let pattern_type = match pattern_type {
            Some(pt) => pt,
            None => continue, // Mean doesn't fit a recognised bucket
        };

        let mean_hours = mean / 3600.0;
        let confidence = (1.0 - ratio).clamp(0.0, 1.0) as f32;

        let memory_ids: Vec<i64> = ids.into_iter().take(MAX_MEMORY_IDS).collect();

        let description = format!(
            "Recurring '{}' memories ~every {:.1}h ({})",
            category,
            mean_hours,
            bucket_label(bucket_centre),
        );

        patterns.push(TemporalPattern {
            id: None,
            pattern_type: pattern_type.to_string(),
            description,
            memory_ids,
            confidence,
            recurrence: Some(recurrence.to_string()),
            created_at: None,
        });
    }

    // --- 4. Persist each pattern (scoped to the detecting user) ---
    for pattern in &patterns {
        if let Err(e) = store_pattern(db, pattern, user_id).await {
            warn!(msg = "temporal_pattern_store_failed", error = %e);
        }
    }

    info!(msg = "temporal_patterns_detected", count = patterns.len());
    Ok(patterns)
}

/// Classify a mean inter-arrival time (in seconds) into a pattern bucket.
///
/// Returns `(pattern_type, iso_duration, bucket_centre_secs)` or `None` if
/// the mean does not fit a recognised bucket.
fn classify_bucket(mean_secs: f64) -> (Option<&'static str>, &'static str, f64) {
    const DAILY_CENTRE: f64 = 86_400.0;
    const WEEKLY_CENTRE: f64 = 604_800.0;
    const MONTHLY_CENTRE: f64 = 2_592_000.0;

    if (mean_secs - DAILY_CENTRE).abs() <= DAILY_TOLERANCE_SECS {
        (Some("daily"), "P1D", DAILY_CENTRE)
    } else if (mean_secs - WEEKLY_CENTRE).abs() <= WEEKLY_TOLERANCE_SECS {
        (Some("weekly"), "P1W", WEEKLY_CENTRE)
    } else if (mean_secs - MONTHLY_CENTRE).abs() <= MONTHLY_TOLERANCE_SECS {
        (Some("monthly"), "P30D", MONTHLY_CENTRE)
    } else {
        (None, "", 0.0)
    }
}

/// Return a human-readable label for a bucket identified by its centre in seconds.
fn bucket_label(centre_secs: f64) -> &'static str {
    match centre_secs as i64 {
        86_400 => "daily",
        604_800 => "weekly",
        2_592_000 => "monthly",
        _ => "interval",
    }
}

/// Parse an SQLite datetime string ("YYYY-MM-DD HH:MM:SS" or ISO 8601 variants)
/// into a Unix timestamp in seconds. Returns 0 on parse failure so entries
/// sort to the epoch rather than panicking.
fn parse_sqlite_timestamp(s: &str) -> i64 {
    // Try full datetime formats first.
    if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return ndt.and_utc().timestamp();
    }
    if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.fZ") {
        return ndt.and_utc().timestamp();
    }
    if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%SZ") {
        return ndt.and_utc().timestamp();
    }
    // Fall back to date-only (midnight UTC).
    if let Some(ndt) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
    {
        return ndt.and_utc().timestamp();
    }
    0
}

// ============================================================================
// PERSISTENCE
// ============================================================================

/// Persist a `TemporalPattern` to the `temporal_patterns` table, scoped to
/// the given user so that single-DB mode isolates pattern rows per owner.
/// `memory_ids` is serialised to a JSON array for the TEXT column.
#[tracing::instrument(skip(db, pattern), fields(user_id))]
pub async fn store_pattern(db: &Database, pattern: &TemporalPattern, user_id: i64) -> Result<()> {
    let pattern_type = pattern.pattern_type.clone();
    let description = pattern.description.clone();
    let memory_ids_json = serde_json::to_string(&pattern.memory_ids)
        .map_err(|e| EngError::DatabaseMessage(format!("failed to serialise memory_ids: {e}")))?;
    let confidence = f64::from(pattern.confidence);
    let recurrence = pattern.recurrence.clone();

    db.write(move |conn| {
        conn.execute(
            "INSERT INTO temporal_patterns \
             (pattern_type, description, memory_ids, confidence, recurrence, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                pattern_type,
                description,
                memory_ids_json,
                confidence,
                recurrence,
                user_id,
            ],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

/// List persisted temporal patterns for the given user, newest first, up to
/// `limit` rows. The WHERE user_id = ?1 predicate enforces single-DB isolation.
/// `memory_ids` JSON text is deserialised back into `Vec<i64>`; a NULL or
/// malformed column yields an empty vec rather than an error.
#[tracing::instrument(skip(db), fields(user_id, limit))]
pub async fn list_patterns(
    db: &Database,
    user_id: i64,
    limit: i64,
) -> Result<Vec<TemporalPattern>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, pattern_type, description, memory_ids, confidence, \
                        recurrence, created_at \
                 FROM temporal_patterns \
                 WHERE user_id = ?1 \
                 ORDER BY created_at DESC \
                 LIMIT ?2",
            )
            .map_err(rusqlite_to_eng_error)?;

        let iter = stmt
            .query_map(params![user_id, limit], |row| {
                let id: i64 = row.get(0)?;
                let pattern_type: String = row.get(1)?;
                let description: String = row.get(2)?;
                let memory_ids_json: Option<String> = row.get(3)?;
                let confidence: f64 = row.get(4)?;
                let recurrence: Option<String> = row.get(5)?;
                let created_at: Option<String> = row.get(6)?;
                Ok((
                    id,
                    pattern_type,
                    description,
                    memory_ids_json,
                    confidence,
                    recurrence,
                    created_at,
                ))
            })
            .map_err(rusqlite_to_eng_error)?;

        let mut patterns = Vec::new();
        for row in iter {
            let (
                id,
                pattern_type,
                description,
                memory_ids_json,
                confidence,
                recurrence,
                created_at,
            ) = row.map_err(rusqlite_to_eng_error)?;

            // Deserialise memory_ids JSON; treat NULL or bad JSON as empty vec.
            let memory_ids: Vec<i64> = memory_ids_json
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();

            patterns.push(TemporalPattern {
                id: Some(id),
                pattern_type,
                description,
                memory_ids,
                confidence: confidence as f32,
                recurrence,
                created_at,
            });
        }
        Ok(patterns)
    })
    .await
}

// ============================================================================
// VALID_AT POPULATION
// ============================================================================

/// Set valid_at on a newly inserted structured_fact (tenant-scoped).
/// Priority: date_approx > date_ref resolved > created_at of memory
#[tracing::instrument(skip(db, memory_created_at))]
pub async fn set_fact_validity(
    db: &Database,
    fact_id: i64,
    memory_created_at: &str,
    user_id: i64,
) -> Result<()> {
    let memory_created_at = memory_created_at.to_string();

    let row = db
        .read(move |conn| {
            conn.query_row(
                "SELECT date_approx, date_ref FROM structured_facts WHERE id = ?1",
                params![fact_id],
                |row| {
                    let date_approx: Option<String> = row.get(0)?;
                    let date_ref: Option<String> = row.get(1)?;
                    Ok((date_approx, date_ref))
                },
            )
            .optional()
            .map_err(rusqlite_to_eng_error)
        })
        .await?;

    let (date_approx, date_ref) = match row {
        Some(r) => r,
        None => return Ok(()),
    };

    let valid_at = if let Some(ref approx) = date_approx {
        approx.clone()
    } else if let Some(ref dref) = date_ref {
        resolve_relative_date(dref, &memory_created_at).unwrap_or_else(|| memory_created_at.clone())
    } else {
        memory_created_at.clone()
    };

    db.write(move |conn| {
        conn.execute(
            "UPDATE structured_facts SET valid_at = ?1 WHERE id = ?2",
            params![valid_at, fact_id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

// ============================================================================
// DATE RESOLUTION
// ============================================================================

/// Parse a word-form number ("one", "two", etc.) or numeric string to i64.
fn parse_word_number(word: &str) -> i64 {
    match word.to_lowercase().as_str() {
        "a" | "an" | "one" => 1,
        "two" => 2,
        "three" => 3,
        "four" => 4,
        "five" => 5,
        "six" => 6,
        "seven" => 7,
        "eight" => 8,
        "nine" => 9,
        "ten" => 10,
        other => other.parse::<i64>().unwrap_or(0),
    }
}

/// Resolve relative date references to YYYY-MM-DD strings.
/// Handles: "today", "yesterday", "N days/weeks/months ago",
/// "last monday", "last week", "a week ago", etc.
pub fn resolve_relative_date(reference: &str, base_date: &str) -> Option<String> {
    use chrono::{Datelike, NaiveDate, NaiveDateTime, Weekday};

    // Parse base date -- try datetime first, then date-only
    let base = if let Ok(dt) = NaiveDateTime::parse_from_str(base_date, "%Y-%m-%dT%H:%M:%S%.fZ") {
        dt.date()
    } else if let Ok(dt) = NaiveDateTime::parse_from_str(base_date, "%Y-%m-%d %H:%M:%S") {
        dt.date()
    } else if let Ok(d) = NaiveDate::parse_from_str(base_date, "%Y-%m-%d") {
        d
    } else {
        return None;
    };

    let lower = reference.to_lowercase();
    let lower = lower.trim();

    // Simple relative dates
    if lower == "today" {
        return Some(base.format("%Y-%m-%d").to_string());
    }
    if lower == "yesterday" {
        let d = base - chrono::Duration::days(1);
        return Some(d.format("%Y-%m-%d").to_string());
    }
    if lower == "this morning" || lower == "this afternoon" || lower == "this evening" {
        return Some(base.format("%Y-%m-%d").to_string());
    }
    if lower == "last morning" || lower == "last afternoon" || lower == "last evening" {
        let d = base - chrono::Duration::days(1);
        return Some(d.format("%Y-%m-%d").to_string());
    }

    // "N days/weeks/months ago"
    let parts: Vec<&str> = lower.split_whitespace().collect();
    if parts.len() == 3 && parts[2] == "ago" {
        let num = parse_word_number(parts[0]);
        if num > 0 {
            let unit = parts[1];
            let d = if unit.starts_with("day") {
                base - chrono::Duration::days(num)
            } else if unit.starts_with("week") {
                base - chrono::Duration::weeks(num)
            } else if unit.starts_with("month") {
                // Approximate month subtraction
                let mut y = base.year();
                let mut m = base.month() as i32 - num as i32;
                while m <= 0 {
                    m += 12;
                    y -= 1;
                }
                NaiveDate::from_ymd_opt(y, m as u32, base.day().min(28)).unwrap_or(base)
            } else {
                return None;
            };
            return Some(d.format("%Y-%m-%d").to_string());
        }
    }

    // "a week/month ago"
    if parts.len() == 3 && parts[0] == "a" && parts[2] == "ago" {
        let d = match parts[1] {
            "week" => base - chrono::Duration::weeks(1),
            "month" => {
                let mut y = base.year();
                let mut m = base.month() as i32 - 1;
                if m <= 0 {
                    m += 12;
                    y -= 1;
                }
                NaiveDate::from_ymd_opt(y, m as u32, base.day().min(28)).unwrap_or(base)
            }
            _ => return None,
        };
        return Some(d.format("%Y-%m-%d").to_string());
    }

    // "last week/month/monday/tuesday/..."
    if parts.len() == 2 && parts[0] == "last" {
        let unit = parts[1];
        if unit == "week" {
            let d = base - chrono::Duration::weeks(1);
            return Some(d.format("%Y-%m-%d").to_string());
        }
        if unit == "month" {
            let mut y = base.year();
            let mut m = base.month() as i32 - 1;
            if m <= 0 {
                m += 12;
                y -= 1;
            }
            let d = NaiveDate::from_ymd_opt(y, m as u32, base.day().min(28)).unwrap_or(base);
            return Some(d.format("%Y-%m-%d").to_string());
        }
        // Day of week
        let target_weekday = match unit {
            "monday" => Some(Weekday::Mon),
            "tuesday" => Some(Weekday::Tue),
            "wednesday" => Some(Weekday::Wed),
            "thursday" => Some(Weekday::Thu),
            "friday" => Some(Weekday::Fri),
            "saturday" => Some(Weekday::Sat),
            "sunday" => Some(Weekday::Sun),
            _ => None,
        };
        if let Some(target) = target_weekday {
            let current = base.weekday().num_days_from_sunday() as i64;
            let target_num = target.num_days_from_sunday() as i64;
            let mut diff = current - target_num;
            if diff <= 0 {
                diff += 7;
            }
            let d = base - chrono::Duration::days(diff);
            return Some(d.format("%Y-%m-%d").to_string());
        }
    }

    None
}

// ============================================================================
// CONTRADICTION DETECTION ON STRUCTURED FACTS
// ============================================================================

/// State-type verbs where only one value can be true at a time.
const STATE_VERBS: &[&str] = &[
    "is", "has", "lives", "works", "became", "started", "moved", "lives in", "works at", "works as",
];

/// Check if a new structured fact contradicts existing facts.
/// Two facts contradict when: same subject + same verb + different object (for state verbs),
/// or same subject + verb + different quantity.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip(db, subject, verb, object))]
pub async fn detect_fact_contradictions(
    db: &Database,
    new_fact_id: i64,
    _memory_id: i64,
    subject: &str,
    verb: &str,
    object: Option<&str>,
    quantity: Option<f64>,
    _user_id: i64,
) -> Result<Vec<FactContradiction>> {
    let subject = subject.to_string();
    let verb = verb.to_string();
    let object = object.map(|s| s.to_string());

    // Compute these before the closure consumes subject/verb
    let verb_lower_owned = verb.to_lowercase();
    let is_state_verb = STATE_VERBS.contains(&verb_lower_owned.trim());

    /// Ephemeral row type for candidate contradicting facts returned from the DB query.
    struct CandidateRow {
        old_id: i64,
        old_memory_id: i64,
        old_object: Option<String>,
        old_quantity: Option<f64>,
    }

    let subject_c = subject.clone();
    let verb_c = verb.clone();
    let candidates = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, memory_id, object, quantity
                     FROM structured_facts
                     WHERE subject = ?1 COLLATE NOCASE
                       AND verb = ?2 COLLATE NOCASE
                       AND id != ?3
                       AND invalid_at IS NULL
                     ORDER BY created_at DESC
                     LIMIT 20",
                )
                .map_err(rusqlite_to_eng_error)?;

            let rows = stmt
                .query_map(params![subject_c, verb_c, new_fact_id], |row| {
                    Ok(CandidateRow {
                        old_id: row.get(0)?,
                        old_memory_id: row.get(1)?,
                        old_object: row.get(2)?,
                        old_quantity: row.get(3)?,
                    })
                })
                .map_err(rusqlite_to_eng_error)?;

            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(rusqlite_to_eng_error)
        })
        .await?;

    let mut contradictions = Vec::new();
    for c in candidates {
        let mut is_contradiction = false;

        // State-type verbs: "user is X" vs "user is Y"
        if is_state_verb {
            if let (Some(ref new_obj), Some(ref old_obj)) = (&object, &c.old_object) {
                if new_obj.to_lowercase() != old_obj.to_lowercase() {
                    is_contradiction = true;
                }
            }
        }

        // Quantity contradiction: same thing, different quantity
        if let (Some(new_q), Some(old_q)) = (quantity, c.old_quantity) {
            if (new_q - old_q).abs() > f64::EPSILON
                && object.as_ref().map(|o| o.to_lowercase())
                    == c.old_object.as_ref().map(|o| o.to_lowercase())
            {
                is_contradiction = true;
            }
        }

        if is_contradiction {
            contradictions.push(FactContradiction {
                new_fact_id,
                old_fact_id: c.old_id,
                old_memory_id: c.old_memory_id,
                subject: subject.clone(),
                verb: verb.clone(),
                new_object: object.clone(),
                old_object: c.old_object,
            });
        }
    }

    Ok(contradictions)
}

/// Invalidate old facts that have been contradicted by a newer fact (tenant-scoped).
#[tracing::instrument(skip(db, contradictions))]
pub async fn invalidate_contradicted_facts(
    db: &Database,
    contradictions: &[FactContradiction],
    user_id: i64,
) -> Result<i32> {
    if contradictions.is_empty() {
        return Ok(0);
    }

    // Clone the data we need to move into the closure
    let contradiction_ids: Vec<(i64, i64)> = contradictions
        .iter()
        .map(|c| (c.new_fact_id, c.old_fact_id))
        .collect();

    let invalidated = db
        .write(move |conn| {
            let mut count = 0i32;
            for (new_fact_id, old_fact_id) in &contradiction_ids {
                let affected = conn
                    .execute(
                        "UPDATE structured_facts SET invalid_at = datetime('now'), invalidated_by = ?1 WHERE id = ?2 AND invalid_at IS NULL",
                        params![new_fact_id, old_fact_id],
                    )
                    .map_err(rusqlite_to_eng_error)?;
                if affected > 0 {
                    count += 1;
                }
            }
            Ok(count)
        })
        .await?;

    if invalidated > 0 {
        let subjects: Vec<String> = contradictions
            .iter()
            .map(|c| format!("{}.{}", c.subject, c.verb))
            .collect();
        info!(
            msg = "facts_invalidated_by_contradiction",
            count = invalidated,
            user_id,
            ?subjects
        );
    }

    Ok(invalidated)
}

/// Post-process newly inserted facts for a memory:
/// 1. Set valid_at based on date info
/// 2. Detect and invalidate contradictions
#[tracing::instrument(skip(db))]
pub async fn post_process_new_facts(db: &Database, memory_id: i64, user_id: i64) -> Result<()> {
    // Get the memory created_at for date resolution (tenant-scoped)
    let created_at = db
        .read(move |conn| {
            conn.query_row(
                "SELECT created_at FROM memories WHERE id = ?1",
                params![memory_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(rusqlite_to_eng_error)
        })
        .await?;

    let created_at = match created_at {
        Some(s) => s,
        None => return Ok(()),
    };

    // Get all facts just inserted for this memory (those without valid_at)
    let facts = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, subject, verb, object, quantity
                     FROM structured_facts WHERE memory_id = ?1 AND valid_at IS NULL",
                )
                .map_err(rusqlite_to_eng_error)?;

            let rows = stmt
                .query_map(params![memory_id], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<f64>>(4)?,
                    ))
                })
                .map_err(rusqlite_to_eng_error)?;

            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(rusqlite_to_eng_error)
        })
        .await?;

    for (fact_id, subject, verb, object, quantity) in &facts {
        // Set temporal validity (tenant-scoped)
        if let Err(e) = set_fact_validity(db, *fact_id, &created_at, user_id).await {
            warn!(msg = "set_fact_validity_failed", fact_id, user_id, error = %e);
        }

        // Check for contradictions
        match detect_fact_contradictions(
            db,
            *fact_id,
            memory_id,
            subject,
            verb,
            object.as_deref(),
            *quantity,
            user_id,
        )
        .await
        {
            Ok(contradictions) if !contradictions.is_empty() => {
                if let Err(e) = invalidate_contradicted_facts(db, &contradictions, user_id).await {
                    warn!(msg = "fact_invalidation_failed", user_id, error = %e);
                }
            }
            Err(e) => {
                warn!(msg = "fact_contradiction_check_failed", user_id, error = %e);
            }
            _ => {}
        }
    }

    Ok(())
}

/// Backfill valid_at for existing facts that do not have it yet.
/// SECURITY: user_id is required so writes stay tenant-scoped. Admin-level
/// backfill should iterate users rather than passing a wildcard.
#[tracing::instrument(skip(db))]
pub async fn backfill_fact_validity(db: &Database, user_id: i64) -> Result<i32> {
    let pending = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT sf.id, sf.date_approx, sf.date_ref, m.created_at as memory_created_at
                     FROM structured_facts sf
                     JOIN memories m ON m.id = sf.memory_id
                     WHERE sf.valid_at IS NULL",
                )
                .map_err(rusqlite_to_eng_error)?;

            let rows = stmt
                .query_map(params![], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })
                .map_err(rusqlite_to_eng_error)?;

            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(rusqlite_to_eng_error)
        })
        .await?;

    let updates: Vec<(i64, String)> = pending
        .iter()
        .map(|(id, date_approx, date_ref, memory_created_at)| {
            let valid_at = if let Some(approx) = date_approx {
                approx.clone()
            } else if let Some(dref) = date_ref {
                resolve_relative_date(dref, memory_created_at)
                    .unwrap_or_else(|| memory_created_at.clone())
            } else {
                memory_created_at.clone()
            };
            (*id, valid_at)
        })
        .collect();

    let filled = db
        .write(move |conn| {
            let mut count = 0i32;
            for (id, valid_at) in &updates {
                conn.execute(
                    "UPDATE structured_facts SET valid_at = ?1 WHERE id = ?2",
                    params![valid_at, id],
                )
                .map_err(rusqlite_to_eng_error)?;
                count += 1;
            }
            Ok(count)
        })
        .await?;

    if filled > 0 {
        info!(msg = "fact_validity_backfilled", count = filled, user_id);
    }

    Ok(filled)
}

// ============================================================================
// TIME TRAVEL -- query memories as they existed at a given timestamp
// ============================================================================

/// Retrieve memories as they existed at or before a given timestamp.
/// Optionally filter by content substring.
#[tracing::instrument(skip(db, query, timestamp))]
pub async fn time_travel(
    db: &Database,
    _user_id: i64,
    query: Option<&str>,
    timestamp: &str,
    limit: i64,
) -> Result<Vec<TimeTravelResult>> {
    let timestamp = timestamp.to_string();

    if let Some(q) = query {
        let pattern = format!("%{}%", q);
        db.read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, content, category, importance, created_at \
                     FROM memories \
                     WHERE created_at <= ?1 AND is_forgotten = 0 \
                       AND content LIKE ?2 \
                     ORDER BY created_at DESC LIMIT ?3",
                )
                .map_err(rusqlite_to_eng_error)?;

            let rows = stmt
                .query_map(params![timestamp, pattern, limit], |row| {
                    Ok(TimeTravelResult {
                        id: row.get(0)?,
                        content: row.get(1)?,
                        category: row.get(2)?,
                        importance: row.get(3)?,
                        created_at: row.get(4)?,
                    })
                })
                .map_err(rusqlite_to_eng_error)?;

            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(rusqlite_to_eng_error)
        })
        .await
    } else {
        db.read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, content, category, importance, created_at \
                     FROM memories \
                     WHERE created_at <= ?1 AND is_forgotten = 0 \
                     ORDER BY created_at DESC LIMIT ?2",
                )
                .map_err(rusqlite_to_eng_error)?;

            let rows = stmt
                .query_map(params![timestamp, limit], |row| {
                    Ok(TimeTravelResult {
                        id: row.get(0)?,
                        content: row.get(1)?,
                        category: row.get(2)?,
                        importance: row.get(3)?,
                        created_at: row.get(4)?,
                    })
                })
                .map_err(rusqlite_to_eng_error)?;

            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(rusqlite_to_eng_error)
        })
        .await
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that `parse_word_number` handles English words and numeric strings.
    #[test]
    fn test_parse_word_number() {
        assert_eq!(parse_word_number("one"), 1);
        assert_eq!(parse_word_number("two"), 2);
        assert_eq!(parse_word_number("three"), 3);
        assert_eq!(parse_word_number("ten"), 10);
        assert_eq!(parse_word_number("a"), 1);
        assert_eq!(parse_word_number("an"), 1);
        assert_eq!(parse_word_number("5"), 5);
        assert_eq!(parse_word_number("42"), 42);
        assert_eq!(parse_word_number("xyz"), 0);
    }

    /// "today" resolves to the base date.
    #[test]
    fn test_resolve_today() {
        let r = resolve_relative_date("today", "2024-06-15T12:00:00.000Z");
        assert_eq!(r, Some("2024-06-15".to_string()));
    }

    /// "yesterday" resolves to base minus one day.
    #[test]
    fn test_resolve_yesterday() {
        let r = resolve_relative_date("yesterday", "2024-06-15T12:00:00.000Z");
        assert_eq!(r, Some("2024-06-14".to_string()));
    }

    /// "N days ago" resolves to base minus N days.
    #[test]
    fn test_resolve_days_ago() {
        let r = resolve_relative_date("3 days ago", "2024-06-15T12:00:00.000Z");
        assert_eq!(r, Some("2024-06-12".to_string()));
    }

    /// English word for N ("two days ago") resolves identically to the numeric form.
    #[test]
    fn test_resolve_word_days_ago() {
        let r = resolve_relative_date("two days ago", "2024-06-15T12:00:00.000Z");
        assert_eq!(r, Some("2024-06-13".to_string()));
    }

    /// "a week ago" resolves to base minus 7 days.
    #[test]
    fn test_resolve_a_week_ago() {
        let r = resolve_relative_date("a week ago", "2024-06-15T12:00:00.000Z");
        assert_eq!(r, Some("2024-06-08".to_string()));
    }

    /// "last week" resolves to base minus 7 days.
    #[test]
    fn test_resolve_last_week() {
        let r = resolve_relative_date("last week", "2024-06-15T12:00:00.000Z");
        assert_eq!(r, Some("2024-06-08".to_string()));
    }

    /// "last monday" resolves correctly relative to a known weekday (Saturday base).
    #[test]
    fn test_resolve_last_monday() {
        // 2024-06-15 is a Saturday
        let r = resolve_relative_date("last monday", "2024-06-15T12:00:00.000Z");
        assert_eq!(r, Some("2024-06-10".to_string()));
    }

    /// "this morning" resolves to the same base date.
    #[test]
    fn test_resolve_this_morning() {
        let r = resolve_relative_date("this morning", "2024-06-15T12:00:00.000Z");
        assert_eq!(r, Some("2024-06-15".to_string()));
    }

    /// An unparseable base date returns `None` without panicking.
    #[test]
    fn test_resolve_invalid_base() {
        let r = resolve_relative_date("yesterday", "not-a-date");
        assert_eq!(r, None);
    }

    /// An unrecognised relative reference returns `None`.
    #[test]
    fn test_resolve_unknown_reference() {
        let r = resolve_relative_date("next century", "2024-06-15T12:00:00.000Z");
        assert_eq!(r, None);
    }

    /// A date-only base string (no time component) parses correctly.
    #[test]
    fn test_resolve_date_only_base() {
        let r = resolve_relative_date("yesterday", "2024-06-15");
        assert_eq!(r, Some("2024-06-14".to_string()));
    }

    /// An SQLite-format datetime base string ("YYYY-MM-DD HH:MM:SS") parses correctly.
    #[test]
    fn test_resolve_sqlite_datetime_base() {
        let r = resolve_relative_date("yesterday", "2024-06-15 12:00:00");
        assert_eq!(r, Some("2024-06-14".to_string()));
    }

    // -----------------------------------------------------------------------
    // detect_patterns / store_pattern / list_patterns integration tests
    // -----------------------------------------------------------------------

    /// Build a SQLite datetime string offset from a fixed base by `hours` hours.
    /// Base is 2026-01-01 00:00:00.
    fn ts(hours: i64) -> String {
        let base =
            chrono::NaiveDateTime::parse_from_str("2026-01-01 00:00:00", "%Y-%m-%d %H:%M:%S")
                .unwrap();
        let t = base + chrono::Duration::hours(hours);
        t.format("%Y-%m-%d %H:%M:%S").to_string()
    }

    /// Insert a single memory row with the given category and created_at.
    async fn insert_memory(db: &crate::db::Database, category: &str, created_at: &str) {
        let cat = category.to_string();
        let cat2 = cat.clone();
        let ts = created_at.to_string();
        db.write(move |conn| {
            conn.execute(
                "INSERT INTO memories (content, category, created_at, is_forgotten) \
                 VALUES (?1, ?2, ?3, 0)",
                rusqlite::params![format!("Memory at {} in {}", ts, cat2), cat2, ts,],
            )
            .map_err(rusqlite_to_eng_error)?;
            Ok(())
        })
        .await
        .unwrap();
    }

    /// 20 memories spaced exactly 24 h apart must produce a daily pattern.
    #[tokio::test]
    async fn test_detect_daily_pattern() {
        let db = crate::db::Database::open_tenant_memory().await.unwrap();

        // Insert 20 memories spaced 24 h apart
        for i in 0..20i64 {
            insert_memory(&db, "morning_routine", &ts(i * 24)).await;
        }

        let patterns = detect_patterns(&db, 1).await.unwrap();

        let daily: Vec<_> = patterns
            .iter()
            .filter(|p| p.pattern_type == "daily")
            .collect();

        assert!(
            !daily.is_empty(),
            "expected at least one daily pattern, got none"
        );
        let p = &daily[0];
        assert!(
            p.memory_ids.len() >= 5,
            "expected memory_ids.len >= 5, got {}",
            p.memory_ids.len()
        );
        assert!(
            p.confidence > 0.5,
            "expected confidence > 0.5, got {}",
            p.confidence
        );
        assert_eq!(p.recurrence.as_deref(), Some("P1D"));
    }

    /// 3 memories at random (far-apart) intervals must produce no pattern.
    #[tokio::test]
    async fn test_no_pattern_for_sparse_category() {
        let db = crate::db::Database::open_tenant_memory().await.unwrap();

        // 3 memories -- below MIN_SAMPLE_SIZE=5
        insert_memory(&db, "ad_hoc", &ts(0)).await;
        insert_memory(&db, "ad_hoc", &ts(100)).await;
        insert_memory(&db, "ad_hoc", &ts(5000)).await;

        let patterns = detect_patterns(&db, 1).await.unwrap();
        let for_cat: Vec<_> = patterns
            .iter()
            .filter(|p| p.description.contains("ad_hoc"))
            .collect();
        assert!(
            for_cat.is_empty(),
            "expected no pattern for sparse ad_hoc category, got {:?}",
            for_cat
        );
    }

    /// `store_pattern` must INSERT a row; `list_patterns` must return it.
    #[tokio::test]
    async fn test_store_and_list_round_trip() {
        let db = crate::db::Database::open_tenant_memory().await.unwrap();

        let pattern = TemporalPattern {
            id: None,
            pattern_type: "daily".to_string(),
            description: "Test pattern".to_string(),
            memory_ids: vec![1, 2, 3],
            confidence: 0.85,
            recurrence: Some("P1D".to_string()),
            created_at: None,
        };

        store_pattern(&db, &pattern, 1).await.unwrap();

        let listed = list_patterns(&db, 1, 10).await.unwrap();
        assert_eq!(listed.len(), 1, "expected exactly 1 pattern");
        let got = &listed[0];
        assert_eq!(got.pattern_type, "daily");
        assert_eq!(got.memory_ids, vec![1i64, 2, 3]);
        assert_eq!(got.recurrence.as_deref(), Some("P1D"));
        assert!(got.id.is_some(), "persisted pattern must have an id");
        assert!((got.confidence - 0.85).abs() < 0.001);
    }

    /// `list_patterns` with limit=1 must return only 1 row when 2 exist.
    #[tokio::test]
    async fn test_list_patterns_respects_limit() {
        let db = crate::db::Database::open_tenant_memory().await.unwrap();

        for i in 0..2i64 {
            let p = TemporalPattern {
                id: None,
                pattern_type: "weekly".to_string(),
                description: format!("Pattern {i}"),
                memory_ids: vec![i],
                confidence: 0.9,
                recurrence: Some("P1W".to_string()),
                created_at: None,
            };
            store_pattern(&db, &p, 1).await.unwrap();
        }

        let listed = list_patterns(&db, 1, 1).await.unwrap();
        assert_eq!(listed.len(), 1, "limit=1 must return exactly 1 row");
        // NOTE: first arg is user_id=1, second is limit=1
    }

    /// `detect_patterns` followed by `list_patterns` must show persisted rows.
    #[tokio::test]
    async fn test_detect_then_list_persists() {
        let db = crate::db::Database::open_tenant_memory().await.unwrap();

        for i in 0..20i64 {
            insert_memory(&db, "daily_standup", &ts(i * 24)).await;
        }

        let detected = detect_patterns(&db, 1).await.unwrap();
        assert!(
            !detected.is_empty(),
            "expected detect to find at least one pattern"
        );

        let listed = list_patterns(&db, 1, 50).await.unwrap();
        assert!(
            !listed.is_empty(),
            "expected list to return the persisted patterns"
        );
    }
}
