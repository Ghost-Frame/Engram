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

/// Service-specific reflection prompts.
fn get_prompt_for_service(service: &str, prompt_override: Option<&str>) -> String {
    if let Some(override_prompt) = prompt_override {
        return override_prompt.to_string();
    }

    match service {
        "engram" => "You are Kleos's internal self-reflection process. Kleos is a persistent memory system.\n\
            Examine the recent activity and ask yourself:\n\
            - Which memories get searched most vs never?\n\
            - What contradictions persist unresolved?\n\
            - What knowledge gaps exist (frequent searches with no results)?\n\
            - What categories are growing fastest?\n\
            - Are memory quality patterns improving or degrading?".to_string(),

        "claude-code" => "You are the self-reflection process for Claude Code agent sessions.\n\
            Examine the session activity and ask yourself:\n\
            - Did a particular approach to a task work well or poorly?\n\
            - Did the user correct a pattern that should be remembered?\n\
            - Was there drift from expected behavior? Why?\n\
            - Was something learned about the codebase or infrastructure?\n\
            - Was there a communication style the user preferred?".to_string(),

        "eidolon" => "You are Eidolon's self-reflection process. Eidolon is the daemon that orchestrates the neurosymbolic brain.\n\
            Examine the dream cycle results and ask yourself:\n\
            - What did this dream cycle reveal about memory patterns?\n\
            - Which patterns keep merging (over-correlated)?\n\
            - What connections are surprising?\n\
            - Is the substrate getting better or worse at targeted activation?".to_string(),

        _ => "You are a self-reflection process for a service in the Syntheos ecosystem.\n\
            Examine the recent activity and extract ONE useful observation about patterns, improvements, or concerns.".to_string(),
    }
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
}
