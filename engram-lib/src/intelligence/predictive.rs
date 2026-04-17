//! Predictive recall -- temporal pattern tracking and proactive memory surfacing.
//!
//! Surfaces memories BEFORE they are asked for based on time of day,
//! day of week, project context, and activity patterns.

use crate::db::Database;
use crate::intelligence::types::{
    PredictedProject, PredictiveContext, ProactiveMemory, SequencePattern,
};
use crate::{EngError, Result};
use rusqlite::params;
use std::collections::HashMap;
use tracing::info;

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

/// Track that a memory category was accessed at this time.
/// Builds the temporal pattern database over time.
#[tracing::instrument(skip(db), fields(user_id, category = %category, project_id = ?project_id))]
pub async fn track_temporal_access(
    db: &Database,
    user_id: i64,
    category: &str,
    project_id: Option<i64>,
) -> Result<()> {
    let now = chrono::Utc::now();
    let dow = now.format("%w").to_string().parse::<i32>().unwrap_or(0); // 0=Sun, 6=Sat
    let hour = now.format("%H").to_string().parse::<i32>().unwrap_or(0);
    let description = format!("dow:{},hour:{},category:{}", dow, hour, category);
    let project_str = project_id.map(|p| p.to_string()).unwrap_or_default();

    db.write(move |conn| {
        conn.execute(
            "INSERT INTO temporal_patterns (pattern_type, description, memory_ids, confidence, user_id, created_at) \
             VALUES ('access', ?1, ?2, 1.0, ?3, datetime('now'))",
            params![description, project_str, user_id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

/// Generate proactive context for the current moment.
/// Called at session start or periodically.
#[tracing::instrument(skip(db))]
pub async fn predictive_recall(db: &Database, user_id: i64) -> Result<PredictiveContext> {
    let now = chrono::Utc::now();
    let dow = now.format("%w").to_string().parse::<i32>().unwrap_or(0);
    let hour = now.format("%H").to_string().parse::<i32>().unwrap_or(0);

    // Time context string
    let day_names = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    let day_name = day_names.get(dow as usize).unwrap_or(&"Unknown");
    let time_period = if hour < 12 {
        "morning"
    } else if hour < 17 {
        "afternoon"
    } else {
        "evening"
    };
    let time_context = format!("{} {}", day_name, time_period);

    // Get temporal patterns for this time slot
    let pattern_query = format!("%dow:{},hour:{}%", dow, hour);
    let predicted_categories: Vec<String> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT description FROM temporal_patterns \
                     WHERE user_id = ?1 AND description LIKE ?2 \
                     ORDER BY created_at DESC LIMIT 20",
                )
                .map_err(rusqlite_to_eng_error)?;
            let rows = stmt
                .query_map(params![user_id, pattern_query], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(rusqlite_to_eng_error)?;
            let descs: Vec<String> = rows
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(rusqlite_to_eng_error)?;
            Ok(descs)
        })
        .await?
        .into_iter()
        .fold(Vec::new(), |mut acc, desc| {
            if let Some(cat_start) = desc.find("category:") {
                let cat = desc[cat_start + 9..].to_string();
                if !acc.contains(&cat) {
                    acc.push(cat);
                }
            }
            acc
        });

    // Get unfinished tasks (recent, not completed)
    struct MemRow {
        id: i64,
        content: String,
        category: String,
        importance: i32,
    }

    let task_rows: Vec<MemRow> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, content, category, importance \
                     FROM memories \
                     WHERE user_id = ?1 AND is_forgotten = 0 AND is_archived = 0 AND is_latest = 1 \
                       AND category = 'task' AND is_static = 0 \
                       AND created_at > datetime('now', '-3 days') \
                     ORDER BY importance DESC, created_at DESC LIMIT 5",
                )
                .map_err(rusqlite_to_eng_error)?;
            let rows = stmt
                .query_map(params![user_id], |row| {
                    Ok(MemRow {
                        id: row.get(0)?,
                        content: row.get(1)?,
                        category: row.get(2)?,
                        importance: row.get(3)?,
                    })
                })
                .map_err(rusqlite_to_eng_error)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(rusqlite_to_eng_error)
        })
        .await?;

    let mut proactive_memories: Vec<ProactiveMemory> = Vec::new();
    let mut suggested_actions: Vec<String> = Vec::new();

    for row in task_rows {
        if suggested_actions.is_empty() {
            let truncated = if row.content.len() > 80 {
                &row.content[..80]
            } else {
                &row.content
            };
            suggested_actions.push(format!("Continue: {}", truncated));
        }
        proactive_memories.push(ProactiveMemory {
            id: row.id,
            content: row.content,
            category: row.category,
            importance: row.importance,
            reason: "unfinished_task".to_string(),
            score: row.importance as f64 / 10.0 + 0.3,
        });
    }

    // Get active issues
    let issue_rows: Vec<MemRow> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, content, category, importance \
                     FROM memories \
                     WHERE user_id = ?1 AND is_forgotten = 0 AND is_archived = 0 AND is_latest = 1 \
                       AND category = 'issue' \
                       AND created_at > datetime('now', '-7 days') \
                     ORDER BY importance DESC LIMIT 3",
                )
                .map_err(rusqlite_to_eng_error)?;
            let rows = stmt
                .query_map(params![user_id], |row| {
                    Ok(MemRow {
                        id: row.get(0)?,
                        content: row.get(1)?,
                        category: row.get(2)?,
                        importance: row.get(3)?,
                    })
                })
                .map_err(rusqlite_to_eng_error)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(rusqlite_to_eng_error)
        })
        .await?;

    for row in issue_rows {
        if suggested_actions.len() < 3 {
            let truncated = if row.content.len() > 80 {
                &row.content[..80]
            } else {
                &row.content
            };
            suggested_actions.push(format!("Address issue: {}", truncated));
        }
        proactive_memories.push(ProactiveMemory {
            id: row.id,
            content: row.content,
            category: row.category,
            importance: row.importance,
            reason: "active_issue".to_string(),
            score: row.importance as f64 / 10.0 + 0.2,
        });
    }

    // Get recent memories for session continuity
    let recent_rows: Vec<MemRow> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, content, category, importance \
                     FROM memories \
                     WHERE user_id = ?1 AND is_forgotten = 0 AND is_archived = 0 AND is_latest = 1 \
                     ORDER BY created_at DESC LIMIT 3",
                )
                .map_err(rusqlite_to_eng_error)?;
            let rows = stmt
                .query_map(params![user_id], |row| {
                    Ok(MemRow {
                        id: row.get(0)?,
                        content: row.get(1)?,
                        category: row.get(2)?,
                        importance: row.get(3)?,
                    })
                })
                .map_err(rusqlite_to_eng_error)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(rusqlite_to_eng_error)
        })
        .await?;

    for row in recent_rows {
        // Avoid duplicates
        if !proactive_memories.iter().any(|m| m.id == row.id) {
            proactive_memories.push(ProactiveMemory {
                id: row.id,
                content: row.content,
                category: row.category,
                importance: row.importance,
                reason: "session_continuity".to_string(),
                score: row.importance as f64 / 10.0,
            });
        }
    }

    // Sort by composite score
    proactive_memories.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    proactive_memories.truncate(10);

    // Try to predict project from recent activity
    let predicted_project = predict_project(db, user_id).await?;
    if let Some(ref proj) = predicted_project {
        suggested_actions.push(format!("Expected project: {}", proj.name));
    }

    info!(
        time_context = %time_context,
        proactive = proactive_memories.len(),
        predicted_categories = predicted_categories.len(),
        user_id,
        "predictive_recall"
    );

    Ok(PredictiveContext {
        time_context,
        predicted_categories,
        predicted_project,
        proactive_memories,
        suggested_actions,
    })
}

/// Minimum number of observations of a (antecedent, consequent) pair
/// before it's surfaced as a predictive sequence pattern. Below this,
/// the "pattern" is statistical noise.
pub const SEQUENCE_MIN_SUPPORT: i64 = 2;

/// Mine consecutive-bigram patterns from a user's memory timeline.
///
/// Memories are scanned in created-at order. For each adjacent pair
/// `(m_i, m_{i+1})` where the gap is at most `window_mins` minutes we
/// count the `(category_i -> category_{i+1})` bigram. Pairs observed at
/// least `SEQUENCE_MIN_SUPPORT` times are returned as
/// `SequencePattern { antecedent, consequent, support, confidence }`.
///
/// `confidence = support / antecedent_total`, i.e. P(consequent |
/// antecedent), measured over consecutive pairs only.
///
/// Results are sorted by `support` descending, then `confidence`
/// descending, so the strongest patterns surface first.
pub async fn detect_sequence_patterns(
    db: &Database,
    user_id: i64,
    window_mins: i64,
) -> Result<Vec<SequencePattern>> {
    if window_mins <= 0 {
        return Ok(Vec::new());
    }

    let rows: Vec<(String, String)> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT category, created_at \
                     FROM memories \
                     WHERE user_id = ?1 \
                       AND is_latest = 1 \
                       AND is_forgotten = 0 \
                       AND is_archived = 0 \
                     ORDER BY created_at ASC, id ASC",
                )
                .map_err(rusqlite_to_eng_error)?;
            let iter = stmt
                .query_map(params![user_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(rusqlite_to_eng_error)?;
            iter.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(rusqlite_to_eng_error)
        })
        .await?;

    if rows.len() < 2 {
        return Ok(Vec::new());
    }

    let window_secs = window_mins.saturating_mul(60);

    let mut bigram_support: HashMap<(String, String), i64> = HashMap::new();
    let mut antecedent_total: HashMap<String, i64> = HashMap::new();

    for pair in rows.windows(2) {
        let (cat_a, ts_a) = &pair[0];
        let (cat_b, ts_b) = &pair[1];
        let a = parse_sql_timestamp(ts_a);
        let b = parse_sql_timestamp(ts_b);
        let (Some(a), Some(b)) = (a, b) else {
            continue;
        };
        let gap = (b - a).num_seconds();
        if gap < 0 || gap > window_secs {
            continue;
        }
        *bigram_support
            .entry((cat_a.clone(), cat_b.clone()))
            .or_insert(0) += 1;
        *antecedent_total.entry(cat_a.clone()).or_insert(0) += 1;
    }

    let mut out: Vec<SequencePattern> = bigram_support
        .into_iter()
        .filter_map(|((a, c), support)| {
            if support < SEQUENCE_MIN_SUPPORT {
                return None;
            }
            let total = *antecedent_total.get(&a).unwrap_or(&support);
            let confidence = if total > 0 {
                support as f64 / total as f64
            } else {
                0.0
            };
            Some(SequencePattern {
                antecedent: a,
                consequent: c,
                support,
                confidence,
            })
        })
        .collect();

    out.sort_by(|x, y| {
        y.support.cmp(&x.support).then_with(|| {
            y.confidence
                .partial_cmp(&x.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });
    Ok(out)
}

fn parse_sql_timestamp(ts: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S") {
        return Some(chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(
            dt,
            chrono::Utc,
        ));
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
        return Some(dt.with_timezone(&chrono::Utc));
    }
    None
}

/// Predict which project the user is likely working on based on recent memory-project links.
async fn predict_project(db: &Database, user_id: i64) -> Result<Option<PredictedProject>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT p.id, p.name, COUNT(*) as cnt \
                 FROM memory_projects mp \
                 JOIN projects p ON p.id = mp.project_id \
                 JOIN memories m ON m.id = mp.memory_id \
                 WHERE p.user_id = ?1 AND m.created_at > datetime('now', '-24 hours') \
                 GROUP BY p.id, p.name \
                 ORDER BY cnt DESC \
                 LIMIT 1",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(params![user_id])
            .map_err(rusqlite_to_eng_error)?;
        if let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            Ok(Some(PredictedProject {
                id: row.get(0).map_err(rusqlite_to_eng_error)?,
                name: row.get(1).map_err(rusqlite_to_eng_error)?,
            }))
        } else {
            Ok(None)
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::types::StoreRequest;

    fn req(content: &str, category: &str, user_id: i64) -> StoreRequest {
        StoreRequest {
            content: content.to_string(),
            category: category.to_string(),
            source: "test".to_string(),
            importance: 5,
            tags: None,
            embedding: None,
            session_id: None,
            is_static: None,
            user_id: Some(user_id),
            space_id: None,
            parent_memory_id: None,
        }
    }

    async fn seed(db: &Database, content: &str, category: &str, user_id: i64) -> i64 {
        crate::memory::store(db, req(content, category, user_id))
            .await
            .expect("store")
            .id
    }

    async fn set_created(db: &Database, mid: i64, created_at: &str) {
        let owned = created_at.to_string();
        db.write(move |conn| {
            conn.execute(
                "UPDATE memories SET created_at = ?1 WHERE id = ?2",
                params![owned, mid],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            Ok(())
        })
        .await
        .expect("set created_at");
    }

    #[tokio::test]
    async fn sequences_empty_below_two_memories() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let out = detect_sequence_patterns(&db, 1, 60).await.expect("det");
        assert!(out.is_empty());
        let _ = seed(&db, "pi alpha", "code", 1).await;
        let out = detect_sequence_patterns(&db, 1, 60).await.expect("det");
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn sequences_empty_when_window_non_positive() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let a = seed(&db, "rho alpha", "code", 1).await;
        let b = seed(&db, "rho beta", "docs", 1).await;
        set_created(&db, a, "2026-04-01 10:00:00").await;
        set_created(&db, b, "2026-04-01 10:05:00").await;
        let out = detect_sequence_patterns(&db, 1, 0).await.expect("det");
        assert!(out.is_empty());
        let out = detect_sequence_patterns(&db, 1, -5).await.expect("det");
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn sequences_empty_when_all_gaps_exceed_window() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let a = seed(&db, "sigma alpha", "code", 1).await;
        let b = seed(&db, "sigma beta", "docs", 1).await;
        set_created(&db, a, "2026-04-01 08:00:00").await;
        set_created(&db, b, "2026-04-01 10:00:00").await; // 120 min gap
        let out = detect_sequence_patterns(&db, 1, 30).await.expect("det");
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn sequences_mines_repeated_bigrams() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let uid = 1;
        let ids = [
            (
                seed(&db, "phi morning rust refactor kicked off", "code", uid).await,
                "2026-04-01 10:00:00",
            ),
            (
                seed(
                    &db,
                    "phi wrote documentation for the new refactor",
                    "docs",
                    uid,
                )
                .await,
                "2026-04-01 10:10:00",
            ),
            (
                seed(
                    &db,
                    "phi afternoon rust optimization pass on retrieval",
                    "code",
                    uid,
                )
                .await,
                "2026-04-01 11:00:00",
            ),
            (
                seed(
                    &db,
                    "phi wrote benchmark report for optimization",
                    "docs",
                    uid,
                )
                .await,
                "2026-04-01 11:10:00",
            ),
            (
                seed(&db, "phi quick chat with master later", "chat", uid).await,
                "2026-04-01 13:00:00",
            ),
        ];
        for (id, ts) in &ids {
            set_created(&db, *id, ts).await;
        }
        let out = detect_sequence_patterns(&db, uid, 30).await.expect("det");
        let top = out.first().expect("at least one");
        assert_eq!(top.antecedent, "code");
        assert_eq!(top.consequent, "docs");
        assert!(top.support >= 2);
        assert!(top.confidence > 0.0);
    }

    #[tokio::test]
    async fn sequences_isolated_per_user() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let a = seed(&db, "upsilon kicked off feature branch", "code", 1).await;
        let b = seed(&db, "upsilon documented the branch changes", "docs", 1).await;
        let c = seed(&db, "upsilon followup refactor pass finished", "code", 1).await;
        let d = seed(&db, "upsilon followup doc changelog entry", "docs", 1).await;
        for (id, ts) in [
            (a, "2026-04-01 10:00:00"),
            (b, "2026-04-01 10:05:00"),
            (c, "2026-04-01 11:00:00"),
            (d, "2026-04-01 11:05:00"),
        ] {
            set_created(&db, id, ts).await;
        }
        let other = detect_sequence_patterns(&db, 99, 30).await.expect("det");
        assert!(other.is_empty());
    }

    #[test]
    fn test_time_context_format() {
        let day_names = [
            "Sunday",
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday",
        ];
        let dow = 3; // Wednesday
        let hour = 14;
        let day_name = day_names[dow];
        let time_period = if hour < 12 {
            "morning"
        } else if hour < 17 {
            "afternoon"
        } else {
            "evening"
        };
        let context = format!("{} {}", day_name, time_period);
        assert_eq!(context, "Wednesday afternoon");
    }
}
