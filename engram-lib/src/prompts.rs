//! Prompts -- context generation for LLM system prompts.
//!
//! Ports: prompts/routes.ts (logic)

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
            "<context>\n<kleos-memories count=\"{}\" tokens=\"~{}\">\n{}\n</kleos-memories>\n</context>\n\nThe above are persistent memories from previous sessions. Use them to maintain continuity. If a memory contradicts the current conversation, prefer the conversation.",
            count, tokens_used, memory_block
        ),
        "openai" => format!(
            "# Persistent Memory (Kleos)\nThe following are {} memories from previous sessions (~{} tokens):\n\n{}\n\nUse these memories for context. If they conflict with the current conversation, prefer the conversation.",
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
        "# Kleos Task Header".to_string(),
        format!("actor_model: {}", actor_model_owned),
        format!("actor_role: {}", actor_role),
        format!("prior_models: [{}]", prior_list.join(", ")),
        String::new(),
        "## Attribution Rule".to_string(),
        format!(
            "You are {}. Memories in Kleos tagged with a different model were NOT created by you.",
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
