//! Pack -- greedy knapsack memory packing into a token budget.
//!
//! Ports: pack/index.ts

use crate::db::Database;
use crate::{EngError, Result};
use serde::{Deserialize, Serialize};

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

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

#[tracing::instrument(skip(db, _context), fields(token_budget, format = ?format, user_id))]
pub async fn pack_memories(
    db: &Database,
    _context: &str,
    token_budget: usize,
    format: PackFormat,
    _user_id: i64,
) -> Result<PackResult> {
    // Layer 1: Static facts
    let static_candidates: Vec<PackCandidate> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, content, category, importance \
                     FROM memories \
                     WHERE is_static = 1 AND is_forgotten = 0 AND is_archived = 0 \
                       AND is_consolidated = 0 AND user_id = ?1",
                )
                .map_err(rusqlite_to_eng_error)?;
            let rows = stmt
                .query_map(rusqlite::params![user_id], |row| {
                    Ok(PackCandidate {
                        id: row.get(0)?,
                        content: row.get(1)?,
                        category: row.get(2)?,
                        importance: row.get(3)?,
                        score: 100.0,
                        source: "static".to_string(),
                    })
                })
                .map_err(rusqlite_to_eng_error)?;
            let mut results = Vec::new();
            for row in rows {
                results.push(row.map_err(rusqlite_to_eng_error)?);
            }
            Ok(results)
        })
        .await?;

    // Layer 2: High-importance memories
    let important_candidates: Vec<PackCandidate> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, content, category, importance, \
                            COALESCE(decay_score, importance) as ds \
                     FROM memories \
                     WHERE is_forgotten = 0 AND is_archived = 0 AND is_latest = 1 \
                       AND is_consolidated = 0 AND user_id = ?1 \
                     ORDER BY ds DESC LIMIT 30",
                )
                .map_err(rusqlite_to_eng_error)?;
            let rows = stmt
                .query_map(rusqlite::params![user_id], |row| {
                    let ds: f64 = row.get::<_, f64>(4).unwrap_or(5.0);
                    Ok(PackCandidate {
                        id: row.get(0)?,
                        content: row.get(1)?,
                        category: row.get(2)?,
                        importance: row.get(3)?,
                        score: ds * 2.0,
                        source: "important".to_string(),
                    })
                })
                .map_err(rusqlite_to_eng_error)?;
            let mut results = Vec::new();
            for row in rows {
                results.push(row.map_err(rusqlite_to_eng_error)?);
            }
            Ok(results)
        })
        .await?;

    let mut seen = std::collections::HashSet::new();
    let mut candidates: Vec<PackCandidate> = Vec::new();

    for c in static_candidates {
        if seen.insert(c.id) {
            candidates.push(c);
        }
    }
    for c in important_candidates {
        if seen.insert(c.id) {
            candidates.push(c);
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
            let items: Vec<serde_json::Value> = packed
                .iter()
                .map(|p| {
                    serde_json::json!({
                        "id": p.id,
                        "content": p.content,
                        "category": p.category,
                        "importance": p.importance
                    })
                })
                .collect();
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

    let util = (tokens_used * 100).checked_div(token_budget).unwrap_or(0);
    Ok(PackResult {
        packed: output,
        memories_included: packed.len(),
        tokens_estimated: tokens_used,
        token_budget,
        utilization: format!("{}%", util),
    })
}
