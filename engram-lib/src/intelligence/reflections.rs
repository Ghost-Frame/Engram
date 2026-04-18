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
//! The `generate_reflections_with_llm` entry point upgrades the heuristic by
//! calling an `LlmReflector` for each candidate. The LLM may return a richer
//! rationale (why the memory is worth revisiting and what to add) that gets
//! stored as the reflection `content`. If the LLM is unavailable, errors, or
//! returns malformed JSON, the heuristic output is used so the pipeline never
//! blocks a caller on an optional model.

use super::types::Reflection;
use crate::db::Database;
use crate::llm::{local::LocalModelClient, repair_and_parse_json};
use crate::{EngError, Result};
use async_trait::async_trait;
use rusqlite::params;

/// Importance threshold at or above which an unused memory gets a reflection.
pub const REFLECTION_MIN_IMPORTANCE: i32 = 6;

/// Importance threshold at or above which the reflection suggests
/// reconsolidation (strengthen) rather than enrichment.
pub const REFLECTION_RECONSOLIDATE_IMPORTANCE: i32 = 8;

/// Age (days) a memory must reach before it's eligible for a reflection --
/// below this we can't tell whether it's simply fresh or genuinely unused.
pub const REFLECTION_MIN_AGE_DAYS: i64 = 7;

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

// ---------------------------------------------------------------------------
// LLM-backed reflection pipeline
// ---------------------------------------------------------------------------

/// Minimal trait the reflection pipeline calls into. Any LLM client can
/// implement this; tests swap in an in-memory fake to exercise the parse +
/// fallback path without network traffic.
#[async_trait]
pub trait LlmReflector: Send + Sync {
    /// Issue a reflection prompt and return the raw model output.
    async fn reflect(&self, system_prompt: &str, user_prompt: &str) -> Result<String>;
}

#[async_trait]
impl LlmReflector for LocalModelClient {
    async fn reflect(&self, system_prompt: &str, user_prompt: &str) -> Result<String> {
        self.call(system_prompt, user_prompt, None).await
    }
}

const LLM_REFLECTION_SYSTEM: &str =
    "You are an Engram reflection assistant. For each memory you are shown, \
     decide whether the user should 'enrich' (add missing context so retrieval \
     finds it), 'reconsolidate' (restate and strengthen), or 'archive' (no \
     longer relevant). Respond with ONE compact JSON object and nothing else: \
     {\"action\": \"enrich|reconsolidate|archive\", \"rationale\": \"<1 sentence>\"}.";

const ALLOWED_LLM_ACTIONS: &[&str] = &["enrich", "reconsolidate", "archive"];

/// Ask the LLM to reflect on a single candidate. Returns `(action, rationale)`
/// when the LLM produced a parseable response with an allowed action, otherwise
/// `None` so the caller can fall back to the heuristic.
pub async fn llm_reflect_on_memory(
    llm: &dyn LlmReflector,
    category: &str,
    importance: i32,
    content: &str,
) -> Option<(String, String)> {
    let snippet: String = content.chars().take(400).collect();
    let user_prompt = format!(
        "Memory (category={}, importance={}, recall_hits=0, age>=7d):\n\"{}\"",
        category, importance, snippet
    );

    let raw = match llm.reflect(LLM_REFLECTION_SYSTEM, &user_prompt).await {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!(
                "llm_reflect: LLM call failed, falling back to heuristic: {}",
                e
            );
            return None;
        }
    };

    let parsed = repair_and_parse_json(&raw)?;
    let action = parsed.get("action")?.as_str()?.trim().to_ascii_lowercase();
    if !ALLOWED_LLM_ACTIONS.contains(&action.as_str()) {
        tracing::debug!("llm_reflect: rejected action \"{}\"", action);
        return None;
    }
    let rationale = parsed
        .get("rationale")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if rationale.is_empty() {
        return None;
    }
    Some((action, rationale))
}

/// Scan a user's memories for high-importance items that have never been
/// recalled and emit a reflection per candidate. When an `LlmReflector` is
/// supplied, the LLM's action + rationale drive the reflection; otherwise the
/// heuristic path is used. A per-candidate LLM failure silently falls back to
/// the heuristic so a flaky model never blocks the pipeline.
#[tracing::instrument(skip(db, llm))]
pub async fn generate_reflections_with_llm(
    db: &Database,
    llm: Option<&dyn LlmReflector>,
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
        let Some(heuristic_action) = suggestion_for_unused(c.importance) else {
            continue;
        };

        let (action, content, confidence) = match llm {
            Some(model) => {
                match llm_reflect_on_memory(model, &c.category, c.importance, &c.content).await {
                    Some((llm_action, rationale)) => {
                        let snippet: String = c.content.chars().take(120).collect();
                        let body = format!(
                            "[{}] {} memory (importance {}): {} -- {}",
                            llm_action, c.category, c.importance, snippet, rationale
                        );
                        // Confidence mixes importance with a small LLM-certainty
                        // bump so LLM-reviewed reflections sort ahead of heuristics
                        // at the same importance.
                        let confidence = ((c.importance as f64 / 10.0) * 0.9 + 0.1).clamp(0.0, 1.0);
                        (llm_action, body, confidence)
                    }
                    None => heuristic_line(heuristic_action, &c.category, c.importance, &c.content),
                }
            }
            None => heuristic_line(heuristic_action, &c.category, c.importance, &c.content),
        };

        let reflection =
            create_reflection(db, &content, &action, &[c.id], confidence, user_id).await?;
        out.push(reflection);
    }
    Ok(out)
}

fn heuristic_line(
    action: &'static str,
    category: &str,
    importance: i32,
    content: &str,
) -> (String, String, f64) {
    let snippet: String = content.chars().take(120).collect();
    let body = format!(
        "[{}] unused {} memory (importance {}): {}",
        action, category, importance, snippet
    );
    let confidence = (importance as f64 / 10.0).clamp(0.0, 1.0);
    (action.to_string(), body, confidence)
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

    // -- LLM-backed pipeline ------------------------------------------------

    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CannedReflector {
        response: String,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl LlmReflector for CannedReflector {
        async fn reflect(&self, _system: &str, _user: &str) -> Result<String> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(self.response.clone())
        }
    }

    struct FailingReflector {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl LlmReflector for FailingReflector {
        async fn reflect(&self, _system: &str, _user: &str) -> Result<String> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Err(EngError::Internal("simulated LLM failure".into()))
        }
    }

    #[tokio::test]
    async fn llm_pipeline_uses_model_action_and_rationale() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let mid = seed(&db, "omicron unused orientation guide", 7, 1).await;
        set_age_days(&db, mid, 30).await;

        let reflector = CannedReflector {
            response:
                r#"{"action":"archive","rationale":"user pivoted to new stack, guide obsolete"}"#
                    .to_string(),
            calls: AtomicUsize::new(0),
        };

        let out = generate_reflections_with_llm(&db, Some(&reflector), 1, 10)
            .await
            .expect("gen");
        assert_eq!(reflector.calls.load(Ordering::Relaxed), 1);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].reflection_type, "archive");
        assert!(
            out[0].content.contains("user pivoted"),
            "expected LLM rationale in reflection content, got: {}",
            out[0].content
        );
    }

    #[tokio::test]
    async fn llm_pipeline_falls_back_to_heuristic_on_error() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let mid = seed(&db, "pi critical unreferenced fact", 9, 1).await;
        set_age_days(&db, mid, 30).await;

        let reflector = FailingReflector {
            calls: AtomicUsize::new(0),
        };

        let out = generate_reflections_with_llm(&db, Some(&reflector), 1, 10)
            .await
            .expect("gen");
        assert_eq!(reflector.calls.load(Ordering::Relaxed), 1);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].reflection_type, "reconsolidate");
        assert!(out[0].content.starts_with("[reconsolidate]"));
    }

    #[tokio::test]
    async fn llm_pipeline_falls_back_when_action_invalid() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let mid = seed(&db, "rho unused doc with bad action", 7, 1).await;
        set_age_days(&db, mid, 30).await;

        let reflector = CannedReflector {
            response: r#"{"action":"yeet","rationale":"nope"}"#.to_string(),
            calls: AtomicUsize::new(0),
        };

        let out = generate_reflections_with_llm(&db, Some(&reflector), 1, 10)
            .await
            .expect("gen");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].reflection_type, "enrich");
        assert!(out[0].content.starts_with("[enrich]"));
    }

    #[tokio::test]
    async fn llm_pipeline_without_llm_matches_heuristic() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let mid = seed(&db, "sigma unused critical note", 9, 1).await;
        set_age_days(&db, mid, 30).await;
        let out = generate_reflections_with_llm(&db, None, 1, 10)
            .await
            .expect("gen");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].reflection_type, "reconsolidate");
    }

    #[tokio::test]
    async fn llm_pipeline_respects_limit_zero() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let mid = seed(&db, "tau unused critical note", 9, 1).await;
        set_age_days(&db, mid, 30).await;
        let reflector = CannedReflector {
            response: r#"{"action":"enrich","rationale":"needs context"}"#.to_string(),
            calls: AtomicUsize::new(0),
        };
        let out = generate_reflections_with_llm(&db, Some(&reflector), 1, 0)
            .await
            .expect("gen");
        assert!(out.is_empty());
        assert_eq!(reflector.calls.load(Ordering::Relaxed), 0);
    }
}
