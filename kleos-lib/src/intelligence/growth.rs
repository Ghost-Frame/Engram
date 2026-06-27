//! Growth reflection -- LLM-backed self-reflection and growth tracking.
//!
//! Observes recent activity, generates observations about patterns, and
//! stores them as growth memories.

use crate::brain::dream::types::DreamCycleResult;
use crate::config::Config;
use crate::cred::{has_secret_patterns, CreddClient};
use crate::db::Database;
use crate::intelligence::llm::{call_llm, is_llm_available};
use crate::intelligence::types::{
    GrowthObservation, GrowthReflectRequest, GrowthReflectResult, LlmOptions,
    ScoredGrowthObservation,
};
use crate::{EngError, Result};
use rusqlite::OptionalExtension;
use tracing::{info, warn};

#[tracing::instrument(skip(db), fields(user_id, limit))]
/// Lists recent growth observations owned by `user_id` for the requested limit.
pub async fn list_observations(
    db: &Database,
    user_id: i64,
    limit: usize,
) -> Result<Vec<GrowthObservation>> {
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT id, content, source, importance, created_at \
                 FROM memories \
                 WHERE category = 'growth' AND is_forgotten = 0 AND user_id = ?2 \
                 ORDER BY created_at DESC LIMIT ?1",
        )?;

        let observations = stmt
            .query_map(rusqlite::params![limit as i64, user_id], |row| {
                Ok(GrowthObservation {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    source: row.get(2)?,
                    importance: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(observations)
    })
    .await
}

#[tracing::instrument(skip(db))]
/// Converts one growth observation into an insight memory.
pub async fn materialize(db: &Database, observation_id: i64, user_id: i64) -> Result<i64> {
    db.write(move |conn| {
        let result: Option<(String, String)> = conn
            .query_row(
                "SELECT content, source FROM memories \
                 WHERE id = ?1 AND category = 'growth' AND user_id = ?2",
                rusqlite::params![observation_id, user_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        let (content, source) = result.ok_or_else(|| {
            EngError::NotFound(format!("growth observation {} not found", observation_id))
        })?;

        conn.execute(
            "INSERT INTO memories (content, category, source, importance, version, is_latest, \
             source_count, is_static, is_forgotten, confidence, status, user_id, \
             created_at, updated_at) \
             VALUES (?1, 'insight', ?2, 8, 1, 1, 1, 1, 0, 1.0, 'approved', ?3, \
             datetime('now'), datetime('now'))",
            rusqlite::params![content, source, user_id],
        )?;

        Ok(conn.last_insert_rowid())
    })
    .await
}

/// Embedded defaults for the service-specific reflection system prompts.
/// Each is overridable at runtime via the prompt repository under
/// `growth/<service>/system.txt`.
const ENGRAM_REFLECTION_DEFAULT: &str = include_str!("../../prompts/growth/engram/system.txt");
const CLAUDE_CODE_REFLECTION_DEFAULT: &str =
    include_str!("../../prompts/growth/claude_code/system.txt");
const EIDOLON_REFLECTION_DEFAULT: &str = include_str!("../../prompts/growth/eidolon/system.txt");
const DEFAULT_REFLECTION_DEFAULT: &str = include_str!("../../prompts/growth/default/system.txt");

/// Service-specific reflection prompts. An explicit `prompt_override` wins;
/// otherwise the per-service embedded default is resolved through the prompt
/// repository so operators can override it at runtime.
fn get_prompt_for_service(service: &str, prompt_override: Option<&str>) -> String {
    if let Some(override_prompt) = prompt_override {
        return override_prompt.to_string();
    }

    let (id, default) = match service {
        "engram" => ("growth/engram/system", ENGRAM_REFLECTION_DEFAULT),
        "claude-code" => ("growth/claude_code/system", CLAUDE_CODE_REFLECTION_DEFAULT),
        "eidolon" => ("growth/eidolon/system", EIDOLON_REFLECTION_DEFAULT),
        _ => ("growth/default/system", DEFAULT_REFLECTION_DEFAULT),
    };
    crate::llm::prompts::load_prompt(id, default).into_owned()
}

/// Validate that an observation is meaningful (not empty, not meta-commentary).
fn validate_observation(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.len() < 10 || trimmed.len() > 500 {
        return false;
    }
    if trimmed.to_uppercase() == "NOTHING" {
        return false;
    }
    if trimmed.starts_with("I don't") || trimmed.starts_with("There is nothing") {
        return false;
    }
    true
}

/// Resolves an observation through secret redaction when needed.
async fn resolve_growth_observation(
    db: &Database,
    service: &str,
    observation: &str,
    user_id: i64,
) -> Result<String> {
    if !has_secret_patterns(observation) {
        return Ok(observation.to_string());
    }

    let config = Config::from_env();
    let credd = CreddClient::from_config(&config);
    credd.resolve_text(db, user_id, service, observation).await
}

/// Build context lines from a dream cycle result for growth reflection.
///
/// Extracts per-stage telemetry (items processed/changed) from the
/// `DreamCycleResult` and formats them into human-readable lines that
/// describe what the consolidation cycle did. These are prepended to the
/// recent-memory context so the LLM reflects on both what happened in
/// the brain and what the agent recently experienced.
pub fn build_dream_context(
    result: &DreamCycleResult,
    pattern_count: usize,
    edge_count: usize,
) -> Vec<String> {
    let mut lines = Vec::with_capacity(result.stages.len() + 2);
    lines.push(format!(
        "Dream cycle completed in {}ms",
        result.total_duration_ms
    ));

    for stage in &result.stages {
        lines.push(format!(
            "Stage '{}': processed {}, changed {}",
            stage.stage, stage.items_processed, stage.items_changed
        ));
    }

    lines.push(format!(
        "Current substrate: {} patterns, {} edges",
        pattern_count, edge_count
    ));
    lines
}

/// Perform a growth reflection -- observe recent activity and generate an observation.
#[tracing::instrument(skip(db, req), fields(service = %req.service, context_len = req.context.len(), user_id))]
pub async fn reflect(
    db: &Database,
    req: &GrowthReflectRequest,
    user_id: i64,
) -> Result<GrowthReflectResult> {
    if req.context.is_empty() {
        return Err(crate::EngError::InvalidInput(
            "context array is required and must not be empty".to_string(),
        ));
    }

    if !is_llm_available() {
        warn!(service = %req.service, "growth_reflect_skipped: llm_unavailable");
        return Ok(GrowthReflectResult {
            observation: None,
            stored_memory_id: None,
            reflection_id: None,
        });
    }

    let system_prompt = get_prompt_for_service(&req.service, req.prompt_override.as_deref());

    let rules = "\nRules:\n\
        - Output ONE concise observation (1-3 sentences max)\n\
        - Write in first person as the service\n\
        - Be specific -- not generic advice\n\
        - If nothing interesting happened, output exactly: NOTHING\n\
        - Do NOT output meta-commentary, explanations, or multiple options\n\
        - Do NOT repeat things already known";

    let full_system = format!("{}{}", system_prompt, rules);

    let mut user_prompt = format!("Recent activity:\n\n{}\n\n", req.context.join("\n"));
    if let Some(ref existing) = req.existing_growth {
        let truncated = crate::validation::truncate_on_char_boundary(existing, 4000);
        user_prompt.push_str(&format!(
            "Things I already know (do NOT repeat these):\n{}\n\n",
            truncated
        ));
    }
    user_prompt.push_str("What did I learn or notice? One observation, or NOTHING.");

    let opts = LlmOptions {
        temperature: 0.7,
        max_tokens: 300,
    };

    let response = match call_llm(&full_system, &user_prompt, Some(opts)).await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, service = %req.service, "growth_reflect_failed");
            return Ok(GrowthReflectResult {
                observation: None,
                stored_memory_id: None,
                reflection_id: None,
            });
        }
    };

    let trimmed = response.trim().to_string();

    if !validate_observation(&trimmed) {
        info!(service = %req.service, "growth_nothing_observed");
        return Ok(GrowthReflectResult {
            observation: None,
            stored_memory_id: None,
            reflection_id: None,
        });
    }

    let trimmed = resolve_growth_observation(db, &req.service, &trimmed, user_id).await?;

    // Dedup: skip if a growth memory with same 200-char prefix exists in last 24h
    let prefix: String = trimmed.chars().take(200).collect();
    let prefix_clone = prefix.clone();
    let is_dup: bool = db
        .read(move |conn| {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM memories WHERE category = 'growth' \
                     AND substr(content, 1, 200) = ?1 \
                     AND user_id = ?2 \
                     AND created_at > datetime('now', '-24 hours')",
                    rusqlite::params![prefix_clone, user_id],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            Ok(count > 0)
        })
        .await?;
    if is_dup {
        info!(service = %req.service, "growth_duplicate_skipped");
        return Ok(GrowthReflectResult {
            observation: None,
            stored_memory_id: None,
            reflection_id: None,
        });
    }

    // Store as growth memory
    let source = format!("{}-growth", req.service);

    let trimmed_for_closure = trimmed.clone();
    let source_c = source.clone();
    let (memory_id, reflection_id) = db
        .write(move |conn| {
            let trimmed_refl = trimmed_for_closure.clone();
            conn.execute(
                "INSERT INTO memories (content, category, source, importance, version, is_latest, \
                 source_count, is_static, is_forgotten, is_archived, confidence, status, user_id, \
                 created_at, updated_at) \
                 VALUES (?1, 'growth', ?2, 7, 1, 1, 1, 1, 0, 1, 1.0, 'approved', ?3, \
                 datetime('now'), datetime('now'))",
                rusqlite::params![trimmed_for_closure, source_c, user_id],
            )?;

            let memory_id = conn.last_insert_rowid();

            conn.execute(
                "INSERT INTO reflections (content, reflection_type, source_memory_ids, \
                 confidence, user_id, created_at) \
                 VALUES (?1, 'growth', ?2, 1.0, ?3, datetime('now'))",
                rusqlite::params![trimmed_refl, format!("[{}]", memory_id), user_id],
            )?;

            let reflection_id = conn.last_insert_rowid();

            Ok((memory_id, reflection_id))
        })
        .await?;

    info!(
        service = %req.service,
        memory_id,
        reflection_id,
        observation = %trimmed.chars().take(80).collect::<String>(),
        "growth_observation_stored"
    );

    Ok(GrowthReflectResult {
        observation: Some(trimmed),
        stored_memory_id: Some(memory_id),
        reflection_id: Some(reflection_id),
    })
}

/// Extract lowercase keyword tokens from text, filtering short words and
/// common English/German stopwords to reduce noise in overlap scoring.
fn extract_keywords(text: &str) -> std::collections::HashSet<String> {
    const STOPWORDS: &[&str] = &[
        "the", "a", "an", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
        "do", "does", "did", "will", "would", "could", "should", "may", "might", "shall", "can",
        "that", "this", "these", "those", "i", "my", "we", "our", "it", "its", "in", "on", "at",
        "to", "for", "of", "and", "or", "but", "not", "with", "from", "by", "as",
        // German
        "ich", "mein", "wir", "unser", "es", "ist", "sind", "war", "waren", "die", "der", "das",
        "ein", "eine", "und", "oder", "aber", "nicht", "mit", "von", "zu", "für", "als", "aus",
        "bei", "nach", "über",
    ];
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2)
        .map(|w| w.to_lowercase())
        .filter(|w| !STOPWORDS.contains(&w.as_str()))
        .collect()
}

/// Score a single observation against query keywords.
/// Returns a value in [0.0, 1.0]: keyword_overlap * 0.6 + recency * 0.4.
fn score_observation(
    obs: &GrowthObservation,
    query_keywords: &std::collections::HashSet<String>,
) -> f64 {
    let obs_keywords = extract_keywords(&obs.content);

    let keyword_score = if query_keywords.is_empty() || obs_keywords.is_empty() {
        0.0
    } else {
        let overlap = query_keywords.intersection(&obs_keywords).count() as f64;
        let denominator = (query_keywords.len() as f64).sqrt() * (obs_keywords.len() as f64).sqrt();
        overlap / denominator
    };

    // Recency: decay over days. 1.0 at age=0, ~0.5 at 7 days, ~0.2 at 30 days.
    // An unparseable timestamp sinks the row (recency 0.0) instead of falling
    // back to "now", which would wrongly float a malformed observation to the top.
    let recency_score =
        match chrono::NaiveDateTime::parse_from_str(&obs.created_at, "%Y-%m-%d %H:%M:%S") {
            Ok(dt) => {
                let days = chrono::Utc::now()
                    .signed_duration_since(dt.and_utc())
                    .num_seconds()
                    .max(0) as f64
                    / 86_400.0;
                1.0 / (1.0 + days * 0.1)
            }
            Err(_) => 0.0,
        };

    keyword_score * 0.6 + recency_score * 0.4
}

/// Retrieve growth observations matching `query` via the full-text index,
/// regardless of age. Complements the recency pool in `context_growth` so an
/// old but highly relevant observation is not excluded by the recency window.
/// Mirrors `list_observations`' ownership/forgotten predicates, and returns an
/// empty vec when `query` has no usable FTS tokens (empty or all-stopword).
async fn match_observations(
    db: &Database,
    user_id: i64,
    query: &str,
    limit: usize,
) -> Result<Vec<GrowthObservation>> {
    // Best-effort relevance channel: on oversized input, skip the FTS match
    // (degrade to the recency pool) rather than rejecting the whole request.
    if query.len() > crate::validation::MAX_FTS_QUERY_LEN {
        return Ok(Vec::new());
    }
    let match_expr = crate::memory::fts::fts_or_match_query(query);
    if match_expr.is_empty() {
        return Ok(Vec::new());
    }
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT m.id, m.content, m.source, m.importance, m.created_at \
                 FROM memories_fts \
                 JOIN memories m ON m.id = memories_fts.rowid \
                 WHERE memories_fts MATCH ?1 \
                   AND m.category = 'growth' AND m.is_forgotten = 0 AND m.user_id = ?2 \
                 ORDER BY memories_fts.rank LIMIT ?3",
        )?;
        let observations = stmt
            .query_map(
                rusqlite::params![match_expr, user_id, limit as i64],
                |row| {
                    Ok(GrowthObservation {
                        id: row.get(0)?,
                        content: row.get(1)?,
                        source: row.get(2)?,
                        importance: row.get(3)?,
                        created_at: row.get(4)?,
                    })
                },
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(observations)
    })
    .await
}

/// Retrieve the top-N growth observations most relevant to `query`.
///
/// Builds a candidate pool from the most recent observations UNION the
/// full-text matches for `query` (so an old but highly relevant observation
/// still competes), scores each by keyword overlap and recency, and returns
/// the top `limit`. When `query` is empty the FTS pool is empty and scoring
/// collapses to pure recency.
#[tracing::instrument(skip(db), fields(user_id, limit))]
pub async fn context_growth(
    db: &Database,
    user_id: i64,
    query: &str,
    limit: usize,
) -> Result<Vec<ScoredGrowthObservation>> {
    let pool_size = (limit * 10).min(200);

    // Recency pool (also the sole pool when `query` is empty), then merge in
    // full-text matches of any age, de-duplicated by id.
    let mut observations = list_observations(db, user_id, pool_size).await?;
    let mut seen: std::collections::HashSet<i64> = observations.iter().map(|o| o.id).collect();
    for obs in match_observations(db, user_id, query, pool_size).await? {
        if seen.insert(obs.id) {
            observations.push(obs);
        }
    }

    let query_keywords = extract_keywords(query);
    let mut scored: Vec<(f64, GrowthObservation)> = observations
        .into_iter()
        .map(|obs| {
            let score = score_observation(&obs, &query_keywords);
            (score, obs)
        })
        .collect();

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    Ok(scored
        .into_iter()
        .take(limit)
        .map(|(score, obs)| ScoredGrowthObservation {
            id: obs.id,
            content: obs.content,
            source: obs.source,
            score: (score * 1000.0).round() / 1000.0,
            created_at: obs.created_at,
        })
        .collect())
}

/// Tests the growth reflection helpers and validation rules.
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that valid observations pass validation.
    #[test]
    fn test_validate_observation_valid() {
        assert!(validate_observation(
            "I noticed that memory access patterns shift during weekday evenings."
        ));
    }

    /// Verifies that short observations are rejected.
    #[test]
    fn test_validate_observation_too_short() {
        assert!(!validate_observation("short"));
    }

    /// Verifies that the literal NOTHING is rejected.
    #[test]
    fn test_validate_observation_nothing() {
        assert!(!validate_observation("NOTHING"));
    }

    /// Verifies that meta-commentary is rejected.
    #[test]
    fn test_validate_observation_meta() {
        assert!(!validate_observation("I don't see anything interesting"));
        assert!(!validate_observation("There is nothing notable"));
    }

    /// Verifies that a prompt override is returned unchanged.
    #[test]
    fn test_get_prompt_override() {
        let p = get_prompt_for_service("engram", Some("Custom prompt"));
        assert_eq!(p, "Custom prompt");
    }

    /// Verifies that the default service prompt includes the expected guidance.
    #[test]
    fn test_get_prompt_default() {
        let p = get_prompt_for_service("unknown_service", None);
        assert!(p.contains("self-reflection process"));
    }

    /// Stopwords and short words are excluded from keyword extraction.
    #[test]
    fn test_extract_keywords_filters_stopwords() {
        let kw = extract_keywords("the quick brown fox and a dog");
        assert!(!kw.contains("the"));
        assert!(!kw.contains("and"));
        assert!(kw.contains("fox")); // len=3 passes the >2 filter and is not a stopword
        assert!(kw.contains("quick"));
        assert!(kw.contains("brown"));
    }

    /// Words of length <= 2 are always filtered out.
    #[test]
    fn test_extract_keywords_min_length() {
        let kw = extract_keywords("I am ok go now");
        assert!(!kw.contains("i"));
        assert!(!kw.contains("am"));
        assert!(!kw.contains("ok"));
        assert!(!kw.contains("go"));
    }

    /// German stopwords are excluded.
    #[test]
    fn test_extract_keywords_german_stopwords() {
        let kw = extract_keywords("ich habe eine neue Erkenntnis gemacht");
        assert!(!kw.contains("ich"));
        assert!(!kw.contains("eine"));
        assert!(kw.contains("neue"));
        assert!(kw.contains("erkenntnis"));
        assert!(kw.contains("gemacht"));
    }

    fn make_obs(content: &str, created_at: &str) -> GrowthObservation {
        GrowthObservation {
            id: 1,
            content: content.to_string(),
            source: "test".to_string(),
            importance: 7,
            created_at: created_at.to_string(),
        }
    }

    /// A perfectly matching observation scores higher than an unrelated one.
    #[test]
    fn test_score_observation_keyword_ranking() {
        let query_kw = extract_keywords("docker compose sidecar");
        let recent = "2099-01-01 00:00:00"; // far future = maximum recency
        let relevant = make_obs("fixing docker compose sidecar configuration", recent);
        let irrelevant = make_obs("the German lexicon was migrated to toml", recent);
        let score_rel = score_observation(&relevant, &query_kw);
        let score_irrel = score_observation(&irrelevant, &query_kw);
        assert!(
            score_rel > score_irrel,
            "relevant={score_rel:.3} should beat irrelevant={score_irrel:.3}"
        );
    }

    /// An empty query collapses scoring to pure recency.
    #[test]
    fn test_score_observation_empty_query_uses_recency() {
        let empty_kw = extract_keywords("");
        let newer = make_obs("some observation about memory", "2099-06-01 12:00:00");
        let older = make_obs("some observation about memory", "2020-01-01 00:00:00");
        let score_new = score_observation(&newer, &empty_kw);
        let score_old = score_observation(&older, &empty_kw);
        assert!(
            score_new > score_old,
            "newer={score_new:.3} should beat older={score_old:.3}"
        );
    }

    /// A malformed timestamp yields zero recency instead of falling back to
    /// "now", so a bad-`created_at` row cannot float to the top on recency.
    #[test]
    fn test_score_observation_unparseable_timestamp_sinks() {
        let empty_kw = extract_keywords("");
        let valid_recent = make_obs("some observation about memory", "2099-01-01 00:00:00");
        let malformed = make_obs("some observation about memory", "not-a-timestamp");
        let score_valid = score_observation(&valid_recent, &empty_kw);
        let score_bad = score_observation(&malformed, &empty_kw);
        assert_eq!(
            score_bad, 0.0,
            "unparseable timestamp must score 0 on an empty query, not max recency"
        );
        assert!(
            score_valid > score_bad,
            "valid recent={score_valid:.3} must beat malformed-timestamp row={score_bad:.3}"
        );
    }

    /// The candidate pool is FTS-augmented: an old observation pushed out of the
    /// recency window still surfaces when it matches the query. Regression guard
    /// for the recency-window limitation (recency-only pools could never see it).
    #[tokio::test]
    async fn test_context_growth_surfaces_old_relevant_observation() {
        let db = Database::connect_memory().await.expect("connect_memory");
        db.write(move |conn| {
            // Fill the recency window with recent, unrelated growth observations.
            for i in 0..40 {
                conn.execute(
                    "INSERT INTO memories (content, category, source, user_id, created_at) \
                         VALUES (?1, 'growth', 'test', 1, datetime('now'))",
                    rusqlite::params![format!(
                        "recent unrelated reflection {i} about cooking dinner"
                    )],
                )?;
            }
            // One OLD observation -- the only one matching the query terms.
            conn.execute(
                "INSERT INTO memories (content, category, source, user_id, created_at) \
                     VALUES (?1, 'growth', 'test', 1, '2020-01-01 00:00:00')",
                rusqlite::params!["learned to fix the docker compose sidecar networking"],
            )?;
            Ok(())
        })
        .await
        .expect("seed growth observations");

        // limit=3 -> recency pool caps at 30 rows, so the 41st-by-recency old
        // observation is excluded from the recency pool; only the FTS channel
        // can surface it.
        let results = context_growth(&db, 1, "docker compose sidecar", 3)
            .await
            .expect("context_growth");
        assert!(
            results
                .iter()
                .any(|o| o.content.contains("docker compose sidecar")),
            "old but FTS-relevant observation must surface; got {results:?}"
        );
    }
}
