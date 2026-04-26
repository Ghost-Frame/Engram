pub use super::types::{EvolutionRequest, EvolutionResult};

use crate::db::Database;
use crate::llm::local::LocalModelClient;
use crate::llm::types::{CallOptions, Priority};
use crate::skills;
use crate::{EngError, Result};
use rusqlite::params;

pub const FIX_SYSTEM_PROMPT: &str =
    "You are a skill fixer. Analyze failures and improve skill content.";
pub const DERIVE_SYSTEM_PROMPT: &str =
    "You are a skill deriver. Combine parent skills into a new derived skill.";
pub const CAPTURE_SYSTEM_PROMPT: &str =
    "You are a skill capturer. Create a new skill from a workflow description.";

const NAME_SHOT_SUFFIX: &str = "\n\nRespond with ONLY a short kebab-case slug (2 to 5 words, lowercase letters, digits, and hyphens). No punctuation, no quotes, no explanation, no code fences.";
const DESC_SHOT_SUFFIX: &str = "\n\nRespond with ONLY a single-sentence description of this skill (under 200 characters). No quotes, no explanation, no prefix.";
const CODE_SHOT_SUFFIX: &str = "\n\nRespond with ONLY the skill body as markdown. Start with a short heading. Include concrete steps, examples, and guardrails. Keep it under 4000 characters. Do not wrap the output in code fences.";

/// Strip code fences from LLM output.
pub fn strip_code_fences(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.starts_with("```") {
        let lines: Vec<&str> = trimmed.lines().collect();
        if lines.len() >= 2 {
            let start = 1;
            let end = if lines.last().is_some_and(|l| l.trim() == "```") {
                lines.len() - 1
            } else {
                lines.len()
            };
            return lines[start..end].join("\n");
        }
    }
    trimmed.to_string()
}

/// Normalize a model-produced slug into a kebab-case id-safe string.
fn sanitize_slug(raw: &str) -> String {
    let cleaned = strip_code_fences(raw);
    let first_line = cleaned.lines().next().unwrap_or("").trim();
    let lowered: String = first_line
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let collapsed = lowered
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let bounded = if collapsed.len() > 80 {
        collapsed[..80].to_string()
    } else {
        collapsed
    };
    if bounded.is_empty() {
        "unnamed-skill".to_string()
    } else {
        bounded
    }
}

/// Normalize a one-line description from the model.
fn sanitize_description(raw: &str) -> String {
    let cleaned = strip_code_fences(raw);
    let first_line = cleaned.lines().next().unwrap_or("").trim();
    let unquoted = first_line
        .trim_matches(|c: char| c == '"' || c == '\'')
        .trim();
    if unquoted.len() > 500 {
        unquoted[..500].to_string()
    } else {
        unquoted.to_string()
    }
}

/// Generate a new unique skill ID.
pub fn generate_skill_id(base_name: &str) -> String {
    let slug = sanitize_slug(base_name);
    let short_id = &uuid::Uuid::new_v4().to_string()[..8];
    format!("{}-{}", slug, short_id)
}

/// Require a local LLM client, or produce a clear error.
fn require_llm(llm: Option<&LocalModelClient>) -> Result<&LocalModelClient> {
    llm.ok_or_else(|| {
        EngError::Internal(
            "skill evolution requires a local LLM (OLLAMA_URL/OLLAMA_MODEL) and none is configured"
                .into(),
        )
    })
}

/// Run a single background-priority LLM call with a bounded response.
async fn llm_shot(
    llm: &LocalModelClient,
    system_prompt: &str,
    user_prompt: &str,
    max_tokens: u32,
) -> Result<String> {
    let opts = CallOptions {
        priority: Priority::Background,
        temperature: Some(0.2),
        max_tokens: Some(max_tokens),
        ..Default::default()
    };
    llm.call(system_prompt, user_prompt, Some(opts)).await
}

/// Persist an evolved skill to the database.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip(db, description, code, tags), fields(name = %name, agent = %agent, parent_ids = ?parent_ids, user_id))]
pub async fn persist_evolved_skill(
    db: &Database,
    name: &str,
    description: &str,
    code: &str,
    agent: &str,
    parent_ids: &[i64],
    tags: &[String],
    _user_id: i64,
) -> Result<i64> {
    let name_owned = name.to_string();
    let description_owned = description.to_string();
    let code_owned = code.to_string();
    let agent_owned = agent.to_string();
    let parent_ids_owned = parent_ids.to_vec();
    let tags_owned = tags.to_vec();

    db.write(move |conn| {
        let (version, root_id) = if let Some(&parent_id) = parent_ids_owned.first() {
            let mut stmt = conn
                .prepare("SELECT version, root_skill_id FROM skill_records WHERE id = ?1")
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            let mut rows = stmt
                .query(params![parent_id])
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            if let Some(row) = rows.next().map_err(|e| EngError::DatabaseMessage(e.to_string()))? {
                let pv: i32 =
                    row.get(0).map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
                let pr: Option<i64> =
                    row.get(1).map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
                (pv + 1, pr.or(Some(parent_id)))
            } else {
                (1, None)
            }
        } else {
            (1, None)
        };

        conn.execute(
            "INSERT INTO skill_records (name, agent, description, code, language, version, parent_skill_id, root_skill_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                name_owned,
                agent_owned,
                description_owned,
                code_owned,
                "markdown".to_string(),
                version,
                parent_ids_owned.first().copied(),
                root_id,
            ],
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        let new_id = conn.last_insert_rowid();

        for &pid in &parent_ids_owned {
            conn.execute(
                "INSERT OR IGNORE INTO skill_lineage_parents (skill_id, parent_id) VALUES (?1, ?2)",
                params![new_id, pid],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        }
        for tag in &tags_owned {
            conn.execute(
                "INSERT OR IGNORE INTO skill_tags (skill_id, tag) VALUES (?1, ?2)",
                params![new_id, tag],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        }
        Ok(new_id)
    })
    .await
}

/// Deactivate a skill (soft-delete).
#[tracing::instrument(skip(db), fields(skill_id))]
pub async fn deactivate_skill(db: &Database, skill_id: i64) -> Result<()> {
    db.write(move |conn| {
        conn.execute(
            "UPDATE skill_records SET is_active = 0, updated_at = datetime('now') WHERE id = ?1",
            params![skill_id],
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        Ok(())
    })
    .await
}

/// Fix a failing skill: read current content + recent failures, ask the local
/// model to produce a new name, description, and body, and persist as a new
/// version whose parent is the failing skill.
#[tracing::instrument(skip(db, llm, agent), fields(skill_id, user_id))]
pub async fn fix_skill(
    db: &Database,
    llm: Option<&LocalModelClient>,
    skill_id: i64,
    agent: &str,
    user_id: i64,
) -> Result<EvolutionResult> {
    let llm = require_llm(llm)?;

    let current = skills::get_skill(db, skill_id, user_id).await?;
    let failures = skills::get_executions(db, skill_id, user_id, 5)
        .await
        .unwrap_or_default();

    let mut failure_notes = String::new();
    for rec in failures.iter().filter(|r| !r.success) {
        let et = rec.error_type.as_deref().unwrap_or("");
        let em = rec.error_message.as_deref().unwrap_or("");
        failure_notes.push_str(&format!("- [{}] {}\n", et, em));
    }
    if failure_notes.is_empty() {
        failure_notes
            .push_str("(no recorded failures; treat the current body as the sole signal)\n");
    }

    let context = format!(
        "Failing skill name: {}\nCurrent description: {}\nCurrent body:\n---\n{}\n---\nRecent failures:\n{}",
        current.name,
        current.description.clone().unwrap_or_default(),
        current.code,
        failure_notes,
    );

    let name_user = format!(
        "Propose a new kebab-case slug for a fixed version of this skill.\n\n{}{}",
        context, NAME_SHOT_SUFFIX
    );
    let desc_user = format!(
        "Write a new one-sentence description for the fixed version.\n\n{}{}",
        context, DESC_SHOT_SUFFIX
    );
    let code_user = format!(
        "Rewrite the skill body so that the listed failures no longer occur. Preserve the intent, tighten the steps, and add guardrails for the observed errors.\n\n{}{}",
        context, CODE_SHOT_SUFFIX
    );

    let name_raw = llm_shot(llm, FIX_SYSTEM_PROMPT, &name_user, 64).await?;
    let desc_raw = llm_shot(llm, FIX_SYSTEM_PROMPT, &desc_user, 256).await?;
    let code_raw = llm_shot(llm, FIX_SYSTEM_PROMPT, &code_user, 2000).await?;

    let name = sanitize_slug(&name_raw);
    let description = sanitize_description(&desc_raw);
    let code = strip_code_fences(&code_raw);

    let tags = vec!["fixed".to_string()];
    let new_id = persist_evolved_skill(
        db,
        &name,
        &description,
        &code,
        agent,
        &[skill_id],
        &tags,
        user_id,
    )
    .await?;

    Ok(EvolutionResult {
        success: true,
        skill_id: Some(new_id),
        evolution_type: "fix".into(),
        message: format!(
            "fixed {} -> new skill {} (id={})",
            current.name, name, new_id
        ),
    })
}

/// Derive a new skill from one or more parents plus a direction hint.
#[tracing::instrument(skip(db, llm, agent), fields(parent_ids = ?parent_ids, direction = %direction, user_id))]
pub async fn derive_skill(
    db: &Database,
    llm: Option<&LocalModelClient>,
    parent_ids: &[i64],
    direction: &str,
    agent: &str,
    user_id: i64,
) -> Result<EvolutionResult> {
    if parent_ids.is_empty() {
        return Err(EngError::InvalidInput(
            "derive requires at least one parent".into(),
        ));
    }
    let llm = require_llm(llm)?;

    // R8-S-002: cap user-controlled direction so an adversary cannot feed a
    // multi-MB prompt-injection payload into the local LLM. 2000 chars is
    // generous for a one-line "make it do X instead" direction.
    const MAX_DIRECTION: usize = 2000;
    let direction_trimmed = direction.trim();
    if direction_trimmed.is_empty() {
        return Err(EngError::InvalidInput(
            "derive requires a non-empty direction".into(),
        ));
    }
    let direction_capped = if direction_trimmed.chars().count() > MAX_DIRECTION {
        direction_trimmed
            .chars()
            .take(MAX_DIRECTION)
            .collect::<String>()
    } else {
        direction_trimmed.to_string()
    };

    let mut parents = Vec::with_capacity(parent_ids.len());
    for pid in parent_ids {
        parents.push(skills::get_skill(db, *pid, user_id).await?);
    }

    let mut parent_ctx = String::new();
    for p in &parents {
        parent_ctx.push_str(&format!(
            "<parent id=\"{}\" name=\"{}\">\n{}\n</parent>\n\n",
            p.id, p.name, p.code
        ));
    }

    let context = format!(
        "Direction:\n{}\n\nParents:\n{}",
        direction_capped, parent_ctx
    );

    let name_user = format!(
        "Propose a kebab-case slug for a skill derived from the parents, in the requested direction.\n\n{}{}",
        context, NAME_SHOT_SUFFIX
    );
    let desc_user = format!(
        "Write a one-sentence description for this derived skill.\n\n{}{}",
        context, DESC_SHOT_SUFFIX
    );
    let code_user = format!(
        "Write the body of the derived skill. Combine the best of the parent skills, apply the direction, and remove redundancy.\n\n{}{}",
        context, CODE_SHOT_SUFFIX
    );

    let name_raw = llm_shot(llm, DERIVE_SYSTEM_PROMPT, &name_user, 64).await?;
    let desc_raw = llm_shot(llm, DERIVE_SYSTEM_PROMPT, &desc_user, 256).await?;
    let code_raw = llm_shot(llm, DERIVE_SYSTEM_PROMPT, &code_user, 2000).await?;

    let name = sanitize_slug(&name_raw);
    let description = sanitize_description(&desc_raw);
    let code = strip_code_fences(&code_raw);

    let tags = vec!["derived".to_string()];
    let new_id = persist_evolved_skill(
        db,
        &name,
        &description,
        &code,
        agent,
        parent_ids,
        &tags,
        user_id,
    )
    .await?;

    Ok(EvolutionResult {
        success: true,
        skill_id: Some(new_id),
        evolution_type: "derived".into(),
        message: format!("derived {} from {:?} (id={})", name, parent_ids, new_id),
    })
}

/// Capture a brand-new skill from a freeform description.
#[tracing::instrument(skip(db, llm, description, agent), fields(description_len = description.len(), user_id))]
pub async fn capture_skill(
    db: &Database,
    llm: Option<&LocalModelClient>,
    description: &str,
    agent: &str,
    user_id: i64,
) -> Result<EvolutionResult> {
    let trimmed_owned = description.trim();
    if trimmed_owned.is_empty() {
        return Err(EngError::InvalidInput(
            "capture requires a non-empty description".into(),
        ));
    }
    // R8-S-002: cap to keep LLM prompt size bounded regardless of caller.
    const MAX_DESCRIPTION: usize = 2000;
    let capped: String = if trimmed_owned.chars().count() > MAX_DESCRIPTION {
        trimmed_owned.chars().take(MAX_DESCRIPTION).collect()
    } else {
        trimmed_owned.to_string()
    };
    let trimmed = capped.as_str();
    let llm = require_llm(llm)?;

    let name_user = format!(
        "A user wants a reusable skill captured from this workflow description:\n---\n{}\n---\nPropose a kebab-case slug.{}",
        trimmed, NAME_SHOT_SUFFIX
    );
    let desc_user = format!(
        "A user wants a reusable skill captured from this workflow description:\n---\n{}\n---\nWrite a one-sentence description of the skill.{}",
        trimmed, DESC_SHOT_SUFFIX
    );
    let code_user = format!(
        "A user wants a reusable skill captured from this workflow description:\n---\n{}\n---\nWrite the body of the skill: concrete steps, examples, and guardrails.{}",
        trimmed, CODE_SHOT_SUFFIX
    );

    let name_raw = llm_shot(llm, CAPTURE_SYSTEM_PROMPT, &name_user, 64).await?;
    let desc_raw = llm_shot(llm, CAPTURE_SYSTEM_PROMPT, &desc_user, 256).await?;
    let code_raw = llm_shot(llm, CAPTURE_SYSTEM_PROMPT, &code_user, 2000).await?;

    let name = sanitize_slug(&name_raw);
    let description_final = sanitize_description(&desc_raw);
    let code = strip_code_fences(&code_raw);

    let tags = vec!["captured".to_string()];
    let new_id = persist_evolved_skill(
        db,
        &name,
        &description_final,
        &code,
        agent,
        &[],
        &tags,
        user_id,
    )
    .await?;

    Ok(EvolutionResult {
        success: true,
        skill_id: Some(new_id),
        evolution_type: "captured".into(),
        message: format!("captured new skill {} (id={})", name, new_id),
    })
}

/// Main evolution dispatcher.
#[tracing::instrument(skip(db, llm, req), fields(evolution_type = %req.evolution_type, agent = %agent, user_id))]
pub async fn evolve(
    db: &Database,
    llm: Option<&LocalModelClient>,
    req: &EvolutionRequest,
    agent: &str,
    user_id: i64,
) -> Result<EvolutionResult> {
    match req.evolution_type.as_str() {
        "fix" => {
            let sid = req
                .target_skill_ids
                .first()
                .ok_or_else(|| EngError::InvalidInput("fix requires a target skill id".into()))?;
            fix_skill(db, llm, *sid, agent, user_id).await
        }
        "derived" => {
            derive_skill(
                db,
                llm,
                &req.target_skill_ids,
                &req.direction,
                agent,
                user_id,
            )
            .await
        }
        "captured" => capture_skill(db, llm, &req.direction, agent, user_id).await,
        other => Err(EngError::InvalidInput(format!(
            "unknown evolution type: {}",
            other
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_strip_none() {
        assert_eq!(strip_code_fences("hello"), "hello");
    }
    #[test]
    fn test_strip_with_lang() {
        let input = "```md\ncontent\n```";
        assert_eq!(strip_code_fences(input), "content");
    }
    #[test]
    fn test_gen_id() {
        let id = generate_skill_id("My Cool Skill");
        assert!(id.starts_with("my-cool-skill-"));
    }
    #[test]
    fn test_sanitize_slug_strips_punct() {
        assert_eq!(sanitize_slug("  Fix The Bug!!!  "), "fix-the-bug");
    }
    #[test]
    fn test_sanitize_slug_collapses_dashes() {
        assert_eq!(sanitize_slug("foo---bar"), "foo-bar");
    }
    #[test]
    fn test_sanitize_slug_empty_fallback() {
        assert_eq!(sanitize_slug("!!!"), "unnamed-skill");
    }
    #[test]
    fn test_sanitize_description_single_line() {
        let d = sanitize_description("\"This is a skill.\"\nExtra line.");
        assert_eq!(d, "This is a skill.");
    }
}
