// ============================================================================
// BI-TEMPORAL FACT TRACKING + CONTRADICTION DETECTION
// Facts have valid_at/invalid_at windows. Old facts are never deleted, just
// invalidated. New contradicting facts auto-invalidate predecessors on the
// same subject+verb.
// ============================================================================

use super::types::{TemporalPattern, TimeTravelResult};
use crate::db::Database;
use crate::{EngError, Result};
use rusqlite::params;
use tracing::{info, warn};

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
            let mut stmt = conn.prepare(
                "SELECT id, category, created_at \
                     FROM memories \
                     WHERE is_forgotten = 0 AND user_id = ?2 \
                     ORDER BY category, created_at \
                     LIMIT ?1",
            )?;

            let iter = stmt.query_map(params![DETECT_SCAN_LIMIT, user_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;

            Ok(iter.collect::<std::result::Result<Vec<_>, _>>()?)
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
        )?;
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
        let mut stmt = conn.prepare(
            "SELECT id, pattern_type, description, memory_ids, confidence, \
                        recurrence, created_at \
                 FROM temporal_patterns \
                 WHERE user_id = ?1 \
                 ORDER BY created_at DESC \
                 LIMIT ?2",
        )?;

        let iter = stmt.query_map(params![user_id, limit], |row| {
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
        })?;

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
            ) = row?;

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
// TIME TRAVEL -- query memories as they existed at a given timestamp
// ============================================================================

/// Retrieve memories as they existed at or before a given timestamp.
/// Optionally filter by content substring.
#[tracing::instrument(skip(db, query, timestamp))]
pub async fn time_travel(
    db: &Database,
    user_id: i64,
    query: Option<&str>,
    timestamp: &str,
    limit: i64,
) -> Result<Vec<TimeTravelResult>> {
    let timestamp = timestamp.to_string();

    if let Some(q) = query {
        let pattern = format!("%{}%", q);
        db.read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, content, category, importance, created_at \
                     FROM memories \
                     WHERE created_at <= ?1 AND is_forgotten = 0 \
                       AND content LIKE ?2 AND user_id = ?4 \
                     ORDER BY created_at DESC LIMIT ?3",
            )?;

            let rows = stmt.query_map(params![timestamp, pattern, limit, user_id], |row| {
                Ok(TimeTravelResult {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    category: row.get(2)?,
                    importance: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })?;

            Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
        })
        .await
    } else {
        db.read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, content, category, importance, created_at \
                     FROM memories \
                     WHERE created_at <= ?1 AND is_forgotten = 0 AND user_id = ?3 \
                     ORDER BY created_at DESC LIMIT ?2",
            )?;

            let rows = stmt.query_map(params![timestamp, limit, user_id], |row| {
                Ok(TimeTravelResult {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    category: row.get(2)?,
                    importance: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })?;

            Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
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
            )?;
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
