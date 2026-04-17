//! Reflections -- the active-learning loop.
//!
//! A reflection is a meta-memory summarizing *why* a group of source memories
//! matters. `create_reflection` / `list_reflections` are the manual path used
//! by the LLM and client UIs. `generate_reflections` is the automatic path:
//! it scans for high-importance memories that are never recalled and emits a
//! heuristic reflection suggesting whether to enrich, reconsolidate, or
//! delete each one.
//!
//! The suggestion is encoded in the `reflection_type` column so that the
//! caller (or a downstream LLM) can filter by the action to take. The two
//! inputs the heuristic uses today are:
//!
//!   - `recall_hits == 0` (never retrieved)
//!   - `age >= 7 days` (stable enough to judge)
//!
//! and the output bucket is:
//!
//!   - `importance >= 8`  -> `reconsolidate` (probably still useful, strengthen)
//!   - `importance >= 6`  -> `enrich`        (add context so retrieval finds it)
//!   - everything else    -> not generated   (too low-signal to reflect on)
//!
//! Future work can swap the heuristic for an LLM call without changing the
//! interface.

use crate::db::Database;
use crate::{EngError, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};

/// Importance threshold at or above which an unused memory gets a reflection.
pub const REFLECTION_MIN_IMPORTANCE: i32 = 6;

/// Importance threshold at or above which the reflection suggests
/// reconsolidation (strengthen) rather than enrichment.
pub const REFLECTION_RECONSOLIDATE_IMPORTANCE: i32 = 8;

/// Age (days) a memory must reach before it's eligible for a reflection --
/// below this we can't tell whether it's simply fresh or genuinely unused.
pub const REFLECTION_MIN_AGE_DAYS: i64 = 7;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reflection {
    pub id: i64,
    pub content: String,
    pub reflection_type: String,
    pub source_memory_ids: Vec<i64>,
    pub confidence: f64,
    pub user_id: i64,
    pub created_at: String,
}

/// Create a reflection from source memories.
#[tracing::instrument(skip(db, content, source_memory_ids))]
pub async fn create_reflection(
    db: &Database,
    content: &str,
    reflection_type: &str,
    source_memory_ids: &[i64],
    confidence: f64,
    user_id: i64,
) -> Result<Reflection> {
    let ids_json = serde_json::to_string(source_memory_ids).unwrap_or_default();
    let content_owned = content.to_string();
    let reflection_type_owned = reflection_type.to_string();

    let id = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO reflections (content, reflection_type, source_memory_ids, confidence, user_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![content_owned, reflection_type_owned, ids_json, confidence, user_id],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            Ok(conn.last_insert_rowid())
        })
        .await?;

    Ok(Reflection {
        id,
        content: content.into(),
        reflection_type: reflection_type.into(),
        source_memory_ids: source_memory_ids.to_vec(),
        confidence,
        user_id,
        created_at: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    })
}

/// Decide which follow-up action a reflection should suggest for an
/// unused memory. Returns `None` when the memory isn't worth reflecting
/// on (importance too low to justify the noise).
pub fn suggestion_for_unused(importance: i32) -> Option<&'static str> {
    if importance >= REFLECTION_RECONSOLIDATE_IMPORTANCE {
        Some("reconsolidate")
    } else if importance >= REFLECTION_MIN_IMPORTANCE {
        Some("enrich")
    } else {
        None
    }
}

/// Scan a user's memories for high-importance items that have never been
/// recalled, and emit a reflection for each suggesting the follow-up action.
///
/// Returns reflections in descending-importance order (most urgent first),
/// capped at `limit`. Each reflection's `content` is a short,
/// human-readable line so it can be surfaced directly in a UI without a
/// further LLM pass.
#[tracing::instrument(skip(db))]
pub async fn generate_reflections(
    db: &Database,
    user_id: i64,
    limit: usize,
) -> Result<Vec<Reflection>> {
    if limit == 0 {
        return Ok(Vec::new());
    }

    struct Candidate {
        id: i64,
        content: String,
        category: String,
        importance: i32,
    }

    let min_importance = REFLECTION_MIN_IMPORTANCE;
    let age_cutoff = format!("-{} days", REFLECTION_MIN_AGE_DAYS);
    let fetch_limit = limit as i64;

    let candidates: Vec<Candidate> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, content, category, importance \
                     FROM memories \
                     WHERE user_id = ?1 \
                       AND is_latest = 1 \
                       AND is_forgotten = 0 \
                       AND is_archived = 0 \
                       AND recall_hits = 0 \
                       AND importance >= ?2 \
                       AND created_at <= datetime('now', ?3) \
                     ORDER BY importance DESC, created_at ASC \
                     LIMIT ?4",
                )
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            let rows = stmt
                .query_map(
                    params![user_id, min_importance, age_cutoff, fetch_limit],
                    |row| {
                        Ok(Candidate {
                            id: row.get(0)?,
                            content: row.get(1)?,
                            category: row.get(2)?,
                            importance: row.get(3)?,
                        })
                    },
                )
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))
        })
        .await?;

    let mut out = Vec::with_capacity(candidates.len());
    for c in candidates {
        let Some(action) = suggestion_for_unused(c.importance) else {
            continue;
        };
        let snippet: String = c.content.chars().take(120).collect();
        let content = format!(
            "[{}] unused {} memory (importance {}): {}",
            action, c.category, c.importance, snippet
        );
        let confidence = (c.importance as f64 / 10.0).clamp(0.0, 1.0);
        let reflection =
            create_reflection(db, &content, action, &[c.id], confidence, user_id).await?;
        out.push(reflection);
    }
    Ok(out)
}

/// List reflections.
#[tracing::instrument(skip(db))]
pub async fn list_reflections(
    db: &Database,
    user_id: i64,
    limit: usize,
) -> Result<Vec<Reflection>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, content, reflection_type, source_memory_ids, confidence, user_id, created_at \
                 FROM reflections WHERE user_id = ?1 ORDER BY id DESC LIMIT ?2",
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        let rows = stmt
            .query_map(params![user_id, limit as i64], |row| {
                let ids_json: Option<String> = row.get(3)?;
                let source_memory_ids: Vec<i64> = ids_json
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_default();
                Ok(Reflection {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    reflection_type: row.get(2)?,
                    source_memory_ids,
                    confidence: row.get(4)?,
                    user_id: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::types::StoreRequest;

    fn req(content: &str, importance: i32, user_id: i64) -> StoreRequest {
        StoreRequest {
            content: content.to_string(),
            category: "task".to_string(),
            source: "test".to_string(),
            importance,
            tags: None,
            embedding: None,
            session_id: None,
            is_static: None,
            user_id: Some(user_id),
            space_id: None,
            parent_memory_id: None,
        }
    }

    async fn seed(db: &Database, content: &str, importance: i32, user_id: i64) -> i64 {
        crate::memory::store(db, req(content, importance, user_id))
            .await
            .expect("store")
            .id
    }

    async fn set_age_days(db: &Database, mid: i64, days: i64) {
        let expr = format!("datetime('now', '-{} days')", days);
        db.write(move |conn| {
            conn.execute(
                &format!("UPDATE memories SET created_at = {} WHERE id = ?1", expr),
                params![mid],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            Ok(())
        })
        .await
        .expect("age update");
    }

    #[tokio::test]
    async fn suggestion_buckets_by_importance() {
        assert_eq!(suggestion_for_unused(5), None);
        assert_eq!(suggestion_for_unused(6), Some("enrich"));
        assert_eq!(suggestion_for_unused(7), Some("enrich"));
        assert_eq!(suggestion_for_unused(8), Some("reconsolidate"));
        assert_eq!(suggestion_for_unused(10), Some("reconsolidate"));
    }

    #[tokio::test]
    async fn generate_reflections_returns_empty_when_no_candidates() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let out = generate_reflections(&db, 1, 10).await.expect("gen");
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn generate_reflections_returns_empty_when_limit_zero() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let mid = seed(&db, "kappa critical unreferenced fact", 9, 1).await;
        set_age_days(&db, mid, 30).await;
        let out = generate_reflections(&db, 1, 0).await.expect("gen");
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn generate_reflections_includes_boundary_seven_day_zero_hit() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let mid = seed(&db, "lambda seven day boundary unused", 7, 1).await;
        set_age_days(&db, mid, 7).await;
        let out = generate_reflections(&db, 1, 10).await.expect("gen");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].reflection_type, "enrich");
        assert_eq!(out[0].source_memory_ids, vec![mid]);
    }

    #[tokio::test]
    async fn generate_reflections_skips_below_importance_threshold() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let low = seed(&db, "mu low priority unused note", 5, 1).await;
        set_age_days(&db, low, 30).await;
        let out = generate_reflections(&db, 1, 10).await.expect("gen");
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn generate_reflections_isolated_per_user() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let mid = seed(&db, "nu private unused fact", 9, 1).await;
        set_age_days(&db, mid, 30).await;
        let other = generate_reflections(&db, 2, 10).await.expect("gen");
        assert!(other.is_empty());
        let mine = generate_reflections(&db, 1, 10).await.expect("gen");
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].reflection_type, "reconsolidate");
    }

    #[tokio::test]
    async fn generate_reflections_excludes_fresh_memories() {
        let db = Database::connect_memory().await.expect("in-mem db");
        // default created_at = now, so age ~= 0 days
        let _mid = seed(&db, "xi brand new critical note", 9, 1).await;
        let out = generate_reflections(&db, 1, 10).await.expect("gen");
        assert!(out.is_empty(), "fresh memory must not reflect");
    }
}
