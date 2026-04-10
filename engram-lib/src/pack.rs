//! Pack -- greedy knapsack memory packing into a token budget.
//!
//! Ports: pack/index.ts

use crate::db::Database;
use crate::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum PackFormat {
    #[default]
    Text,
    Json,
    Xml,
}

#[derive(Debug, Clone, Serialize)]
pub struct PackCandidate {
    pub id: i64,
    pub content: String,
    pub category: String,
    pub importance: i64,
    pub score: f64,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PackResult {
    pub packed: String,
    pub memories_included: usize,
    pub tokens_estimated: usize,
    pub token_budget: usize,
    pub utilization: String,
}

pub async fn pack_memories(
    db: &Database,
    _context: &str,
    token_budget: usize,
    format: PackFormat,
    user_id: i64,
) -> Result<PackResult> {
    let mut candidates: Vec<PackCandidate> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // Layer 1: Static facts
    let mut rows = db.conn.query(
        "SELECT id, content, category, importance FROM memories WHERE is_static = 1 AND is_forgotten = 0 AND is_archived = 0 AND user_id = ?1",
        libsql::params![user_id],
    ).await?;
    while let Some(row) = rows.next().await? {
        let id: i64 = row
            .get(0)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?;
        if seen.insert(id) {
            candidates.push(PackCandidate {
                id,
                content: row
                    .get(1)
                    .map_err(|e| crate::EngError::Internal(e.to_string()))?,
                category: row
                    .get(2)
                    .map_err(|e| crate::EngError::Internal(e.to_string()))?,
                importance: row
                    .get(3)
                    .map_err(|e| crate::EngError::Internal(e.to_string()))?,
                score: 100.0,
                source: "static".into(),
            });
        }
    }

    // Layer 2: High-importance memories
    let mut rows = db.conn.query(
        "SELECT id, content, category, importance, COALESCE(decay_score, importance) as ds FROM memories WHERE is_forgotten = 0 AND is_archived = 0 AND is_latest = 1 AND user_id = ?1 ORDER BY ds DESC LIMIT 30",
        libsql::params![user_id],
    ).await?;
    while let Some(row) = rows.next().await? {
        let id: i64 = row
            .get(0)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?;
        if seen.insert(id) {
            let ds: f64 = row.get(4).unwrap_or(5.0);
            candidates.push(PackCandidate {
                id,
                content: row
                    .get(1)
                    .map_err(|e| crate::EngError::Internal(e.to_string()))?,
                category: row
                    .get(2)
                    .map_err(|e| crate::EngError::Internal(e.to_string()))?,
                importance: row
                    .get(3)
                    .map_err(|e| crate::EngError::Internal(e.to_string()))?,
                score: ds * 2.0,
                source: "important".into(),
            });
        }
    }

    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut packed: Vec<&PackCandidate> = Vec::new();
    let mut tokens_used = 0usize;
    for c in &candidates {
        let mem_tokens = c.content.len() / 4 + 10;
        if tokens_used + mem_tokens > token_budget {
            continue;
        }
        packed.push(c);
        tokens_used += mem_tokens;
    }

    let output = match format {
        PackFormat::Xml => {
            let parts: Vec<String> = packed
                .iter()
                .map(|p| {
                    format!(
                        "<memory id=\"{}\" category=\"{}\" importance=\"{}\">\n{}\n</memory>",
                        p.id, p.category, p.importance, p.content
                    )
                })
                .collect();
            parts.join("\n")
        }
        PackFormat::Json => {
            let items: Vec<serde_json::Value> = packed.iter().map(|p| {
                serde_json::json!({"id": p.id, "content": p.content, "category": p.category, "importance": p.importance})
            }).collect();
            serde_json::to_string(&items).unwrap_or_default()
        }
        PackFormat::Text => {
            let parts: Vec<String> = packed
                .iter()
                .map(|p| format!("[{}] {}", p.category, p.content))
                .collect();
            parts.join("\n\n")
        }
    };

    let util = if token_budget > 0 {
        tokens_used * 100 / token_budget
    } else {
        0
    };
    Ok(PackResult {
        packed: output,
        memories_included: packed.len(),
        tokens_estimated: tokens_used,
        token_budget,
        utilization: format!("{}%", util),
    })
}
