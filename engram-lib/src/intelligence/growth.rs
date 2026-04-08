//! Growth reflection -- LLM-backed self-reflection and growth tracking.
//!
//! Observes recent activity, generates observations about patterns, and
//! stores them as growth memories.

use crate::db::Database;
use crate::intelligence::llm::{call_llm, is_llm_available, LlmOptions};
use crate::intelligence::types::{GrowthReflectRequest, GrowthReflectResult};
use crate::Result;
use tracing::{info, warn};

/// Service-specific reflection prompts.
fn get_prompt_for_service(service: &str, prompt_override: Option<&str>) -> String {
    if let Some(override_prompt) = prompt_override {
        return override_prompt.to_string();
    }

    match service {
        "engram" => "You are Engram's internal self-reflection process. Engram is a persistent memory system.\n\
            Examine the recent activity and ask yourself:\n\
            - Which memories get searched most vs never?\n\
            - What contradictions persist unresolved?\n\
            - What knowledge gaps exist (frequent searches with no results)?\n\
            - What categories are growing fastest?\n\
            - Are memory quality patterns improving or degrading?".to_string(),

        "claude-code" => "You are the self-reflection process for Claude Code sessions with Master (Zan).\n\
            Examine the session activity and ask yourself:\n\
            - Did a particular approach to a task work well or poorly?\n\
            - Did Master correct a pattern that should be remembered?\n\
            - Was there drift from expected behavior? Why?\n\
            - Was something learned about the codebase or infrastructure?\n\
            - Was there a communication style Master preferred?".to_string(),

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

/// Perform a growth reflection -- observe recent activity and generate an observation.
pub async fn reflect(
    db: &Database,
    req: &GrowthReflectRequest,
    user_id: i64,
) -> Result<GrowthReflectResult> {
    let conn = db.connection();

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

    let system_prompt = get_prompt_for_service(
        &req.service,
        req.prompt_override.as_deref(),
    );

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
        let truncated = if existing.len() > 4000 {
            &existing[..4000]
        } else {
            existing
        };
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

    // Store as growth memory
    let source = format!("{}-growth", req.service);

    conn.execute(
        "INSERT INTO memories (content, category, source, importance, version, is_latest, \
         source_count, is_static, is_forgotten, confidence, status, user_id, \
         created_at, updated_at) \
         VALUES (?1, 'growth', ?2, 7, 1, 1, 1, 1, 0, 1.0, 'approved', ?3, \
         datetime('now'), datetime('now'))",
        libsql::params![trimmed.clone(), source.clone(), user_id],
    )
    .await?;

    let mut id_row = conn.query("SELECT last_insert_rowid()", ()).await?;
    let memory_id: i64 = if let Some(row) = id_row.next().await? {
        row.get(0)?
    } else {
        0
    };

    // Store in reflections table
    conn.execute(
        "INSERT INTO reflections (content, reflection_type, source_memory_ids, \
         confidence, user_id, created_at) \
         VALUES (?1, 'growth', ?2, 1.0, ?3, datetime('now'))",
        libsql::params![
            trimmed.clone(),
            format!("[{}]", memory_id),
            user_id
        ],
    )
    .await?;

    let mut refl_id_row = conn.query("SELECT last_insert_rowid()", ()).await?;
    let reflection_id: i64 = if let Some(row) = refl_id_row.next().await? {
        row.get(0)?
    } else {
        0
    };

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

/// Self-reflection for Engram -- called periodically (e.g., every hour).
/// Gathers memory stats, builds context, and generates a growth observation.
pub async fn self_reflect(db: &Database, user_id: i64) -> Result<GrowthReflectResult> {
    let conn = db.connection();

    // Min activity threshold: 50 new memories in last hour
    let mut count_row = conn
        .query(
            "SELECT COUNT(*) FROM memories \
             WHERE created_at > datetime('now', '-1 hour') AND user_id = ?1",
            libsql::params![user_id],
        )
        .await?;
    let recent_count: i64 = if let Some(row) = count_row.next().await? {
        row.get(0)?
    } else {
        0
    };

    if recent_count < 50 {
        return Ok(GrowthReflectResult {
            observation: None,
            stored_memory_id: None,
            reflection_id: None,
        });
    }

    // 15% probability gate
    if rand::random::<f64>() > 0.15 {
        return Ok(GrowthReflectResult {
            observation: None,
            stored_memory_id: None,
            reflection_id: None,
        });
    }

    // Build context from memory stats
    let mut stats_row = conn
        .query(
            "SELECT COUNT(*) as total, \
                    SUM(CASE WHEN access_count = 0 THEN 1 ELSE 0 END) as never_accessed, \
                    SUM(CASE WHEN category = 'growth' THEN 1 ELSE 0 END) as growth_count, \
                    AVG(importance) as avg_importance \
             FROM memories WHERE is_forgotten = 0 AND user_id = ?1",
            libsql::params![user_id],
        )
        .await?;

    let (total, never_accessed, growth_count, avg_importance) =
        if let Some(row) = stats_row.next().await? {
            (
                row.get::<i64>(0).unwrap_or(0),
                row.get::<i64>(1).unwrap_or(0),
                row.get::<i64>(2).unwrap_or(0),
                row.get::<f64>(3).unwrap_or(0.0),
            )
        } else {
            (0, 0, 0, 0.0)
        };

    let context = vec![
        format!(
            "Memory stats: {} total, {} never accessed, {} growth entries, avg importance {:.1}",
            total, never_accessed, growth_count, avg_importance
        ),
        format!("New memories in last hour: {}", recent_count),
    ];

    // Get existing growth for anti-repeat
    let mut growth_rows = conn
        .query(
            "SELECT content FROM memories \
             WHERE category = 'growth' AND source = 'engram-growth' AND is_forgotten = 0 AND user_id = ?1 \
             ORDER BY created_at DESC LIMIT 10",
            libsql::params![user_id],
        )
        .await?;

    let mut existing_lines = Vec::new();
    while let Some(row) = growth_rows.next().await? {
        let content: String = row.get(0)?;
        existing_lines.push(format!("- {}", content));
    }

    let req = GrowthReflectRequest {
        service: "engram".to_string(),
        context,
        existing_growth: if existing_lines.is_empty() {
            None
        } else {
            Some(existing_lines.join("\n"))
        },
        prompt_override: None,
    };

    reflect(db, &req, user_id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_observation_valid() {
        assert!(validate_observation(
            "I noticed that memory access patterns shift during weekday evenings."
        ));
    }

    #[test]
    fn test_validate_observation_too_short() {
        assert!(!validate_observation("short"));
    }

    #[test]
    fn test_validate_observation_nothing() {
        assert!(!validate_observation("NOTHING"));
    }

    #[test]
    fn test_validate_observation_meta() {
        assert!(!validate_observation("I don't see anything interesting"));
        assert!(!validate_observation("There is nothing notable"));
    }

    #[test]
    fn test_get_prompt_override() {
        let p = get_prompt_for_service("engram", Some("Custom prompt"));
        assert_eq!(p, "Custom prompt");
    }

    #[test]
    fn test_get_prompt_default() {
        let p = get_prompt_for_service("unknown_service", None);
        assert!(p.contains("self-reflection process"));
    }
}
