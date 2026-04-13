//! Prompts -- context generation for LLM system prompts.
//!
//! Ports: prompts/routes.ts (logic)

use crate::config::ServerEntry;
use crate::db::Database;
use crate::{EngError, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptResult {
    pub prompt: String,
    pub format: String,
    pub memories_included: usize,
    pub tokens_estimated: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeaderResult {
    pub header: serde_json::Value,
    pub text: String,
    pub actor_model: String,
    pub prior_models: Vec<String>,
}

pub async fn generate_prompt(
    db: &Database,
    format: &str,
    token_budget: usize,
    _context: &str,
    user_id: i64,
) -> Result<PromptResult> {
    // (id, content, category, score)
    let candidates: Vec<(i64, String, String, f64)> = db
        .read(move |conn| {
            let mut seen = std::collections::HashSet::new();
            let mut out: Vec<(i64, String, String, f64)> = Vec::new();

            // Static facts
            {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, content, category, importance \
                         FROM memories \
                         WHERE is_static = 1 AND is_forgotten = 0 \
                           AND is_consolidated = 0 AND user_id = ?1",
                    )
                    .map_err(rusqlite_to_eng_error)?;
                let rows = stmt
                    .query_map(params![user_id], |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    })
                    .map_err(rusqlite_to_eng_error)?;
                for row in rows {
                    let (id, content, category) = row.map_err(rusqlite_to_eng_error)?;
                    if seen.insert(id) {
                        out.push((id, content, category, 100.0));
                    }
                }
            }

            // Important memories
            {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, content, category, importance, \
                                COALESCE(decay_score, importance) AS ds \
                         FROM memories \
                         WHERE is_forgotten = 0 AND is_archived = 0 \
                           AND is_latest = 1 AND is_consolidated = 0 \
                           AND user_id = ?1 \
                         ORDER BY ds DESC LIMIT 1000",
                    )
                    .map_err(rusqlite_to_eng_error)?;
                let rows = stmt
                    .query_map(params![user_id], |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, f64>(4).unwrap_or(5.0),
                        ))
                    })
                    .map_err(rusqlite_to_eng_error)?;
                for row in rows {
                    let (id, content, category, ds) = row.map_err(rusqlite_to_eng_error)?;
                    if seen.insert(id) {
                        out.push((id, content, category, ds * 2.0));
                    }
                }
            }

            out.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal));
            Ok(out)
        })
        .await?;

    let mut packed: Vec<String> = Vec::new();
    let mut tokens_used = 0usize;
    for (_id, content, category, _score) in &candidates {
        let t = content.len() / 4 + 5;
        if tokens_used + t > token_budget {
            continue;
        }
        packed.push(format!("[{}] {}", category, content));
        tokens_used += t;
    }

    let memory_block = packed.join("\n\n");
    let count = packed.len();

    let prompt = match format {
        "anthropic" => format!(
            "<context>\n<engram-memories count=\"{}\" tokens=\"~{}\">\n{}\n</engram-memories>\n</context>\n\nThe above are persistent memories from previous sessions. Use them to maintain continuity. If a memory contradicts the current conversation, prefer the conversation.",
            count, tokens_used, memory_block
        ),
        "openai" => format!(
            "# Persistent Memory (Engram)\nThe following are {} memories from previous sessions (~{} tokens):\n\n{}\n\nUse these memories for context. If they conflict with the current conversation, prefer the conversation.",
            count, tokens_used, memory_block
        ),
        "llamaindex" => format!("[MEMORY CONTEXT]\n{}\n[/MEMORY CONTEXT]", memory_block),
        _ => memory_block,
    };

    Ok(PromptResult {
        prompt,
        format: format.to_string(),
        memories_included: count,
        tokens_estimated: tokens_used,
    })
}

pub async fn generate_header(
    db: &Database,
    actor_model: &str,
    actor_role: &str,
    _context: &str,
    limit: usize,
    user_id: i64,
) -> Result<HeaderResult> {
    let actor_model_owned = actor_model.to_string();
    let fetch_limit = (limit * 3) as i64;

    // (model, id, source, category, content_start, created_at)
    let rows: Vec<(Option<String>, i64, String, String, String, String)> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, content, category, source, model, created_at, importance \
                     FROM memories \
                     WHERE is_forgotten = 0 AND is_archived = 0 \
                       AND is_latest = 1 AND is_consolidated = 0 \
                       AND user_id = ?1 \
                     ORDER BY created_at DESC LIMIT ?2",
                )
                .map_err(rusqlite_to_eng_error)?;
            let rows = stmt
                .query_map(params![user_id, fetch_limit], |row| {
                    Ok((
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(3).unwrap_or_default(),
                        row.get::<_, String>(2).unwrap_or_default(),
                        row.get::<_, String>(1).unwrap_or_default(),
                        row.get::<_, String>(5).unwrap_or_default(),
                    ))
                })
                .map_err(rusqlite_to_eng_error)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(rusqlite_to_eng_error)?);
            }
            Ok(out)
        })
        .await?;

    let mut prior_models = std::collections::HashSet::new();
    let mut prior_work: Vec<serde_json::Value> = Vec::new();

    for (model_opt, id, source, category, content, created_at) in rows {
        if let Some(ref m) = model_opt {
            if m != &actor_model_owned {
                prior_models.insert(m.clone());
                if prior_work.len() < limit {
                    let summary_end = content.len().min(200);
                    prior_work.push(serde_json::json!({
                        "id": id,
                        "model": m,
                        "source": source,
                        "category": category,
                        "summary": &content[..summary_end],
                        "created_at": created_at,
                    }));
                }
            }
        }
    }

    let prior_list: Vec<String> = prior_models.into_iter().collect();
    let mut lines = vec![
        "# Engram Task Header".to_string(),
        format!("actor_model: {}", actor_model_owned),
        format!("actor_role: {}", actor_role),
        format!("prior_models: [{}]", prior_list.join(", ")),
        String::new(),
        "## Attribution Rule".to_string(),
        format!(
            "You are {}. Memories in Engram tagged with a different model were NOT created by you.",
            actor_model_owned
        ),
    ];
    if !prior_work.is_empty() {
        lines.push(String::new());
        lines.push("## Recent Work by Other Models".to_string());
        for pw in prior_work.iter().take(5) {
            lines.push(format!(
                "- [{}] {}",
                pw["model"].as_str().unwrap_or("?"),
                pw["summary"].as_str().unwrap_or("")
            ));
        }
    }

    Ok(HeaderResult {
        header: serde_json::json!({
            "actor_model": actor_model_owned,
            "actor_role": actor_role,
            "prior_models": &prior_list,
            "prior_work_count": prior_work.len(),
            "prior_work": prior_work,
        }),
        text: lines.join("\n"),
        actor_model: actor_model_owned,
        prior_models: prior_list,
    })
}

// ---------------------------------------------------------------------------
// Living prompt -- Eidolon-style context block with brain recall
// ---------------------------------------------------------------------------

/// Credential scrubbing patterns. Lines matching any of these patterns AND
/// containing "=" or ":" (but not "://" or "path") are redacted.
const SCRUB_PATTERNS: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "token",
    "api_key",
    "apikey",
    "private_key",
    "bearer",
    "authorization",
    "credential",
];

/// Remove credential values from arbitrary text before inserting into prompts.
pub fn scrub_credentials(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for line in text.lines() {
        let line_lower = line.to_lowercase();
        let is_cred = SCRUB_PATTERNS
            .iter()
            .any(|pat| line_lower.contains(pat));
        if is_cred
            && (line.contains('=')
                || (line.contains(':')
                    && !line.contains("://")
                    && !line.contains("path")))
        {
            result.push_str("[CREDENTIAL REDACTED - use credential manager]\n");
            continue;
        }
        result.push_str(line);
        result.push('\n');
    }
    result
}

/// A single activated memory returned from a brain query, ready for prompt
/// injection. Credentials are already scrubbed at construction time.
#[derive(Debug, Clone)]
pub struct MemorySummary {
    pub id: i64,
    pub content: String,
    pub category: String,
    pub activation: f64,
}

/// A resolved contradiction pair from the brain.
#[derive(Debug, Clone)]
pub struct ContradictionInfo {
    pub winner_content: String,
    pub loser_content: String,
    pub reason: String,
}

fn format_memories_as_bullets(memories: &[MemorySummary]) -> String {
    if memories.is_empty() {
        return "No relevant patterns activated.".to_string();
    }
    memories
        .iter()
        .take(10)
        .map(|m| {
            let strength = if m.activation > 0.8 {
                "HIGH"
            } else if m.activation > 0.5 {
                "MED"
            } else {
                "LOW"
            };
            format!("- [{}|{}] {}", m.category, strength, m.content.trim())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_contradictions(contradictions: &[ContradictionInfo]) -> String {
    if contradictions.is_empty() {
        return String::new();
    }
    let mut out =
        String::from("\n## Resolved Contradictions (brain settled these conflicts)\n");
    for c in contradictions.iter().take(5) {
        out.push_str(&format!(
            "- **Current truth:** {}\n  ~~Superseded:~~ {}\n  Reason: {}\n",
            c.winner_content.trim(),
            c.loser_content.trim(),
            c.reason,
        ));
    }
    out
}

/// Build the full living-context prompt block.
///
/// All memory lists are optional -- callers pass what they have from brain
/// queries. The engram_url is embedded in the Syntheos tools table.
#[allow(clippy::too_many_arguments)]
pub fn build_living_prompt(
    task: &str,
    task_memories: &[MemorySummary],
    task_contradictions: &[ContradictionInfo],
    infra_memories: &[MemorySummary],
    failure_memories: &[MemorySummary],
    engram_url: &str,
    servers: &[ServerEntry],
    safety_rules: &[String],
) -> String {
    let task_context = format_memories_as_bullets(task_memories);
    let infra_context = format_memories_as_bullets(infra_memories);
    let failure_context = format_memories_as_bullets(failure_memories);
    let contradiction_section = format_contradictions(task_contradictions);

    let server_table = if servers.is_empty() {
        "No server reference configured.".to_string()
    } else {
        let mut table =
            "| Server | Role | SSH User | Notes |\n|--------|------|----------|-------|\n"
                .to_string();
        for s in servers {
            table.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                s.name, s.role, s.ssh_user, s.notes
            ));
        }
        table
    };

    let safety_section = if safety_rules.is_empty() {
        String::new()
    } else {
        let rules = safety_rules
            .iter()
            .map(|r| format!("- {}", r))
            .collect::<Vec<_>>()
            .join("\n");
        format!("\n## Safety Constraints\n{}\n", rules)
    };

    format!(
        r#"=== EIDOLON LIVING CONTEXT ===
# Eidolon Session Context

This context was generated through neural pattern completion, not keyword search.
Contradiction resolution has already been applied -- what you see below reflects
the brain's current understanding.

## Your Task
{task}

## What the Brain Knows About This Task
{task_context}
{contradiction_section}
## Infrastructure Understanding
{infra_context}
{safety_section}
## Server Reference
{server_table}

## Past Failures and Issues Related to This Task
{failure_context}

---

## Syntheos Tools

All services at **{engram_url}**. Use these throughout your session.

| Service | Key Endpoints | When to Use |
|---------|--------------|-------------|
| Engram | POST /search, POST /store, POST /context | Search before guessing. Store after completing work. |
| Chiasm | POST /tasks, PATCH /tasks/:id | Create task on start. Update during. Complete on end. |
| Broca | POST /broca/actions, POST /broca/ask | Log significant actions. Ask infrastructure questions. |
| Axon | POST /axon/publish | Publish events on major milestones. |
| Soma | POST /soma/agents, POST /soma/agents/:id/heartbeat | Register on start. Heartbeat during. |

**MANDATORY:** Search Engram BEFORE asking the user ANY question about servers, credentials, architecture, or past decisions.

=== END LIVING CONTEXT ===
"#,
        task = task,
        task_context = task_context,
        contradiction_section = contradiction_section,
        infra_context = infra_context,
        safety_section = safety_section,
        failure_context = failure_context,
        engram_url = engram_url,
        server_table = server_table,
    )
}
