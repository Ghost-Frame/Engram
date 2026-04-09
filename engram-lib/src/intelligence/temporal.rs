// ============================================================================
// BI-TEMPORAL FACT TRACKING + CONTRADICTION DETECTION
// Facts have valid_at/invalid_at windows. Old facts are never deleted, just
// invalidated. New contradicting facts auto-invalidate predecessors on the
// same subject+verb.
// ============================================================================

use crate::db::Database;
use crate::Result;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemporalPattern {
    pub label: String,
    pub detail: String,
}

pub async fn detect_patterns(_db: &Database, _user_id: i64) -> Result<Vec<TemporalPattern>> {
    Ok(Vec::new())
}

pub async fn list_patterns(
    _db: &Database,
    _user_id: i64,
    _limit: usize,
) -> Result<Vec<TemporalPattern>> {
    Ok(Vec::new())
}

pub async fn store_pattern(_db: &Database, _pattern: &TemporalPattern) -> Result<()> {
    Ok(())
}

// ============================================================================
// VALID_AT POPULATION
// ============================================================================

/// Set valid_at on a newly inserted structured_fact.
/// Priority: date_approx > date_ref resolved > created_at of memory
pub async fn set_fact_validity(db: &Database, fact_id: i64, memory_created_at: &str) -> Result<()> {
    let mut rows = db.conn.query(
        "SELECT date_approx, date_ref FROM structured_facts WHERE id = ?1",
        libsql::params![fact_id],
    ).await?;

    let row = match rows.next().await? {
        Some(r) => r,
        None => return Ok(()),
    };

    let date_approx: Option<String> = row.get(0)?;
    let date_ref: Option<String> = row.get(1)?;

    let valid_at = if let Some(ref approx) = date_approx {
        // Absolute date provided, use it directly
        approx.clone()
    } else if let Some(ref dref) = date_ref {
        // Resolve relative dates against memory creation time
        resolve_relative_date(dref, memory_created_at)
            .unwrap_or_else(|| memory_created_at.to_string())
    } else {
        memory_created_at.to_string()
    };

    db.conn.execute(
        "UPDATE structured_facts SET valid_at = ?1 WHERE id = ?2",
        libsql::params![valid_at.clone(), fact_id],
    ).await?;

    Ok(())
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
                NaiveDate::from_ymd_opt(y, m as u32, base.day().min(28))
                    .unwrap_or(base)
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
                if m <= 0 { m += 12; y -= 1; }
                NaiveDate::from_ymd_opt(y, m as u32, base.day().min(28))
                    .unwrap_or(base)
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
            if m <= 0 { m += 12; y -= 1; }
            let d = NaiveDate::from_ymd_opt(y, m as u32, base.day().min(28))
                .unwrap_or(base);
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
            if diff <= 0 { diff += 7; }
            let d = base - chrono::Duration::days(diff);
            return Some(d.format("%Y-%m-%d").to_string());
        }
    }

    None
}

// ============================================================================
// CONTRADICTION DETECTION ON STRUCTURED FACTS
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactContradiction {
    pub new_fact_id: i64,
    pub old_fact_id: i64,
    pub old_memory_id: i64,
    pub subject: String,
    pub verb: String,
    pub new_object: Option<String>,
    pub old_object: Option<String>,
}

/// State-type verbs where only one value can be true at a time.
const STATE_VERBS: &[&str] = &[
    "is", "has", "lives", "works", "became", "started", "moved",
    "lives in", "works at", "works as",
];

/// Check if a new structured fact contradicts existing facts.
/// Two facts contradict when: same subject + same verb + different object (for state verbs),
/// or same subject + verb + different quantity.
#[allow(clippy::too_many_arguments)]
pub async fn detect_fact_contradictions(
    db: &Database,
    new_fact_id: i64,
    _memory_id: i64,
    subject: &str,
    verb: &str,
    object: Option<&str>,
    quantity: Option<f64>,
    user_id: i64,
) -> Result<Vec<FactContradiction>> {
    let mut contradictions = Vec::new();

    let mut rows = db.conn.query(
        "SELECT id, memory_id, object, quantity, unit
         FROM structured_facts
         WHERE subject = ?1 COLLATE NOCASE
           AND verb = ?2 COLLATE NOCASE
           AND user_id = ?3
           AND id != ?4
           AND invalid_at IS NULL
         ORDER BY created_at DESC
         LIMIT 20",
        libsql::params![subject.to_string(), verb.to_string(), user_id, new_fact_id],
    ).await?;

    let verb_lower = verb.to_lowercase();
    let verb_lower = verb_lower.trim();
    let is_state_verb = STATE_VERBS.contains(&verb_lower);

    while let Some(row) = rows.next().await? {
        let old_id: i64 = row.get(0)?;
        let old_memory_id: i64 = row.get(1)?;
        let old_object: Option<String> = row.get(2)?;
        let old_quantity: Option<f64> = row.get(3)?;

        let mut is_contradiction = false;

        // State-type verbs: "user is X" vs "user is Y"
        if is_state_verb {
            if let (Some(new_obj), Some(ref old_obj)) = (object, &old_object) {
                if new_obj.to_lowercase() != old_obj.to_lowercase() {
                    is_contradiction = true;
                }
            }
        }

        // Quantity contradiction: same thing, different quantity
        if let (Some(new_q), Some(old_q)) = (quantity, old_quantity) {
            if (new_q - old_q).abs() > f64::EPSILON
                && object.map(|o| o.to_lowercase()) == old_object.as_ref().map(|o| o.to_lowercase())
            {
                is_contradiction = true;
            }
        }

        if is_contradiction {
            contradictions.push(FactContradiction {
                new_fact_id,
                old_fact_id: old_id,
                old_memory_id,
                subject: subject.to_string(),
                verb: verb.to_string(),
                new_object: object.map(|s| s.to_string()),
                old_object,
            });
        }
    }

    Ok(contradictions)
}

/// Invalidate old facts that have been contradicted by a newer fact.
pub async fn invalidate_contradicted_facts(
    db: &Database,
    contradictions: &[FactContradiction],
) -> Result<i32> {
    if contradictions.is_empty() {
        return Ok(0);
    }

    let mut invalidated = 0i32;
    for c in contradictions {
        let affected = db.conn.execute(
            "UPDATE structured_facts SET invalid_at = datetime('now'), invalidated_by = ?1 WHERE id = ?2 AND invalid_at IS NULL",
            libsql::params![c.new_fact_id, c.old_fact_id],
        ).await?;
        if affected > 0 {
            invalidated += 1;
        }
    }

    if invalidated > 0 {
        let subjects: Vec<String> = contradictions
            .iter()
            .map(|c| format!("{}.{}", c.subject, c.verb))
            .collect();
        info!(msg = "facts_invalidated_by_contradiction", count = invalidated, ?subjects);
    }

    Ok(invalidated)
}

/// Post-process newly inserted facts for a memory:
/// 1. Set valid_at based on date info
/// 2. Detect and invalidate contradictions
pub async fn post_process_new_facts(
    db: &Database,
    memory_id: i64,
    user_id: i64,
) -> Result<()> {
    // Get the memory created_at for date resolution
    let mut mem_rows = db.conn.query(
        "SELECT created_at FROM memories WHERE id = ?1",
        libsql::params![memory_id],
    ).await?;

    let created_at = match mem_rows.next().await? {
        Some(row) => {
            let s: String = row.get(0)?;
            s
        }
        None => return Ok(()),
    };

    // Get all facts just inserted for this memory (those without valid_at)
    let mut fact_rows = db.conn.query(
        "SELECT id, subject, verb, object, quantity
         FROM structured_facts WHERE memory_id = ?1 AND user_id = ?2 AND valid_at IS NULL",
        libsql::params![memory_id, user_id],
    ).await?;

    let mut facts = Vec::new();
    while let Some(row) = fact_rows.next().await? {
        let id: i64 = row.get(0)?;
        let subject: String = row.get(1)?;
        let verb: String = row.get(2)?;
        let object: Option<String> = row.get(3)?;
        let quantity: Option<f64> = row.get(4)?;
        facts.push((id, subject, verb, object, quantity));
    }

    for (fact_id, subject, verb, object, quantity) in &facts {
        // Set temporal validity
        if let Err(e) = set_fact_validity(db, *fact_id, &created_at).await {
            warn!(msg = "set_fact_validity_failed", fact_id, error = %e);
        }

        // Check for contradictions
        match detect_fact_contradictions(
            db, *fact_id, memory_id, subject, verb,
            object.as_deref(), *quantity, user_id,
        ).await {
            Ok(contradictions) if !contradictions.is_empty() => {
                if let Err(e) = invalidate_contradicted_facts(db, &contradictions).await {
                    warn!(msg = "fact_invalidation_failed", error = %e);
                }
            }
            Err(e) => {
                warn!(msg = "fact_contradiction_check_failed", error = %e);
            }
            _ => {}
        }
    }

    Ok(())
}

/// Backfill valid_at for existing facts that do not have it yet.
pub async fn backfill_fact_validity(db: &Database, user_id: Option<i64>) -> Result<i32> {
    let (query, rows_result) = if let Some(uid) = user_id {
        let q = "SELECT sf.id, sf.date_approx, sf.date_ref, m.created_at as memory_created_at
                  FROM structured_facts sf
                  JOIN memories m ON m.id = sf.memory_id
                  WHERE sf.valid_at IS NULL AND sf.user_id = ?1";
        (q, db.conn.query(q, libsql::params![uid]).await)
    } else {
        let q = "SELECT sf.id, sf.date_approx, sf.date_ref, m.created_at as memory_created_at
                  FROM structured_facts sf
                  JOIN memories m ON m.id = sf.memory_id
                  WHERE sf.valid_at IS NULL";
        (q, db.conn.query(q, libsql::params![]).await)
    };

    let _ = query; // suppress unused warning
    let mut rows = rows_result?;

    let mut pending = Vec::new();
    while let Some(row) = rows.next().await? {
        let id: i64 = row.get(0)?;
        let date_approx: Option<String> = row.get(1)?;
        let date_ref: Option<String> = row.get(2)?;
        let memory_created_at: String = row.get(3)?;
        pending.push((id, date_approx, date_ref, memory_created_at));
    }

    let mut filled = 0i32;
    for (id, date_approx, date_ref, memory_created_at) in &pending {
        let valid_at = if let Some(approx) = date_approx {
            approx.clone()
        } else if let Some(dref) = date_ref {
            resolve_relative_date(dref, memory_created_at)
                .unwrap_or_else(|| memory_created_at.clone())
        } else {
            memory_created_at.clone()
        };

        db.conn.execute(
            "UPDATE structured_facts SET valid_at = ?1 WHERE id = ?2",
            libsql::params![valid_at.clone(), *id],
        ).await?;
        filled += 1;
    }

    if filled > 0 {
        info!(msg = "fact_validity_backfilled", count = filled);
    }

    Ok(filled)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn test_resolve_today() {
        let r = resolve_relative_date("today", "2024-06-15T12:00:00.000Z");
        assert_eq!(r, Some("2024-06-15".to_string()));
    }

    #[test]
    fn test_resolve_yesterday() {
        let r = resolve_relative_date("yesterday", "2024-06-15T12:00:00.000Z");
        assert_eq!(r, Some("2024-06-14".to_string()));
    }

    #[test]
    fn test_resolve_days_ago() {
        let r = resolve_relative_date("3 days ago", "2024-06-15T12:00:00.000Z");
        assert_eq!(r, Some("2024-06-12".to_string()));
    }

    #[test]
    fn test_resolve_word_days_ago() {
        let r = resolve_relative_date("two days ago", "2024-06-15T12:00:00.000Z");
        assert_eq!(r, Some("2024-06-13".to_string()));
    }

    #[test]
    fn test_resolve_a_week_ago() {
        let r = resolve_relative_date("a week ago", "2024-06-15T12:00:00.000Z");
        assert_eq!(r, Some("2024-06-08".to_string()));
    }

    #[test]
    fn test_resolve_last_week() {
        let r = resolve_relative_date("last week", "2024-06-15T12:00:00.000Z");
        assert_eq!(r, Some("2024-06-08".to_string()));
    }

    #[test]
    fn test_resolve_last_monday() {
        // 2024-06-15 is a Saturday
        let r = resolve_relative_date("last monday", "2024-06-15T12:00:00.000Z");
        assert_eq!(r, Some("2024-06-10".to_string()));
    }

    #[test]
    fn test_resolve_this_morning() {
        let r = resolve_relative_date("this morning", "2024-06-15T12:00:00.000Z");
        assert_eq!(r, Some("2024-06-15".to_string()));
    }

    #[test]
    fn test_resolve_invalid_base() {
        let r = resolve_relative_date("yesterday", "not-a-date");
        assert_eq!(r, None);
    }

    #[test]
    fn test_resolve_unknown_reference() {
        let r = resolve_relative_date("next century", "2024-06-15T12:00:00.000Z");
        assert_eq!(r, None);
    }

    #[test]
    fn test_resolve_date_only_base() {
        let r = resolve_relative_date("yesterday", "2024-06-15");
        assert_eq!(r, Some("2024-06-14".to_string()));
    }

    #[test]
    fn test_resolve_sqlite_datetime_base() {
        let r = resolve_relative_date("yesterday", "2024-06-15 12:00:00");
        assert_eq!(r, Some("2024-06-14".to_string()));
    }
}
