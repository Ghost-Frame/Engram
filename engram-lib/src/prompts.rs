//! Prompts -- context generation for LLM system prompts.
//!
//! Ports: prompts/routes.ts (logic)

use crate::db::Database;
use crate::Result;
use serde::{Deserialize, Serialize};

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
    let mut candidates: Vec<(i64, String, String, f64)> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // Static facts
    let mut rows = db.conn.query(
        "SELECT id, content, category, importance FROM memories WHERE is_static = 1 AND is_forgotten = 0 AND is_consolidated = 0 AND user_id = ?1",
        libsql::params![user_id],
    ).await?;
    while let Some(row) = rows.next().await? {
        let id: i64 = row
            .get(0)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?;
        if seen.insert(id) {
            candidates.push((
                id,
                row.get::<String>(1).unwrap_or_default(),
                row.get::<String>(2).unwrap_or_default(),
                100.0,
            ));
        }
    }

    // Important memories
    let mut rows = db.conn.query(
        "SELECT id, content, category, importance, COALESCE(decay_score, importance) as ds FROM memories WHERE is_forgotten = 0 AND is_archived = 0 AND is_latest = 1 AND is_consolidated = 0 AND user_id = ?1 ORDER BY ds DESC LIMIT 1000",
        libsql::params![user_id],
    ).await?;
    while let Some(row) = rows.next().await? {
        let id: i64 = row
            .get(0)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?;
        if seen.insert(id) {
            let ds: f64 = row.get(4).unwrap_or(5.0);
            candidates.push((
                id,
                row.get::<String>(1).unwrap_or_default(),
                row.get::<String>(2).unwrap_or_default(),
                ds * 2.0,
            ));
        }
    }

    candidates.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal));

    let mut packed: Vec<String> = Vec::new();
    let mut tokens_used = 0;
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
        "anthropic" => format!("<context>\n<engram-memories count=\"{}\" tokens=\"~{}\">\n{}\n</engram-memories>\n</context>\n\nThe above are persistent memories from previous sessions. Use them to maintain continuity. If a memory contradicts the current conversation, prefer the conversation.", count, tokens_used, memory_block),
        "openai" => format!("# Persistent Memory (Engram)\nThe following are {} memories from previous sessions (~{} tokens):\n\n{}\n\nUse these memories for context. If they conflict with the current conversation, prefer the conversation.", count, tokens_used, memory_block),
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
    let mut rows = db.conn.query(
        "SELECT id, content, category, source, model, created_at, importance FROM memories WHERE is_forgotten = 0 AND is_archived = 0 AND is_latest = 1 AND is_consolidated = 0 AND user_id = ?1 ORDER BY created_at DESC LIMIT ?2",
        libsql::params![user_id, (limit * 3) as i64],
    ).await?;
    let mut prior_models = std::collections::HashSet::new();
    let mut prior_work: Vec<serde_json::Value> = Vec::new();
    while let Some(row) = rows.next().await? {
        let model: Option<String> = row.get(4).unwrap_or(None);
        if let Some(ref m) = model {
            if m != actor_model {
                prior_models.insert(m.clone());
                if prior_work.len() < limit {
                    let content: String = row.get::<String>(1).unwrap_or_default();
                    let summary_end = content.len().min(200);
                    prior_work.push(serde_json::json!({
                        "id": row.get::<i64>(0).unwrap_or(0),
                        "model": m,
                        "source": row.get::<String>(3).unwrap_or_default(),
                        "category": row.get::<String>(2).unwrap_or_default(),
                        "summary": &content[..summary_end],
                        "created_at": row.get::<String>(5).unwrap_or_default(),
                    }));
                }
            }
        }
    }

    let prior_list: Vec<String> = prior_models.into_iter().collect();
    let mut lines = vec![
        "# Engram Task Header".to_string(),
        format!("actor_model: {}", actor_model),
        format!("actor_role: {}", actor_role),
        format!("prior_models: [{}]", prior_list.join(", ")),
        String::new(),
        "## Attribution Rule".to_string(),
        format!(
            "You are {}. Memories in Engram tagged with a different model were NOT created by you.",
            actor_model
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
            "actor_model": actor_model,
            "actor_role": actor_role,
            "prior_models": &prior_list,
            "prior_work_count": prior_work.len(),
            "prior_work": prior_work,
        }),
        text: lines.join("\n"),
        actor_model: actor_model.to_string(),
        prior_models: prior_list,
    })
}
