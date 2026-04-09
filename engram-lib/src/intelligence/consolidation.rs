use crate::db::Database;
use crate::memory::types::{Memory, StoreRequest};
use crate::memory::{self, insert_link, row_to_memory, update_source_count, MEMORY_COLUMNS};
use crate::Result;
use libsql::params;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use super::llm::{call_llm, repair_and_parse_json, LlmOptions};

const CONSOLIDATION_PROMPT: &str = r#"You are a memory consolidation engine. Given a cluster of related memories and their extracted facts, merge and deduplicate them into a clean fact set.

Rules:
- Identify duplicate or near-duplicate facts and keep only the most recent or most complete version
- Group related facts by topic or entity
- Preserve specific values such as paths, versions, IDs, ports, dates, and names
- Do not add interpretation or speculate
- Keep wording close to the source material where possible

Respond with ONLY a JSON object:
{
  "title": "short 3-5 word cluster label",
  "merged_facts": ["fact 1", "fact 2"],
  "removed_duplicates": ["duplicate fact"],
  "importance": 1-10
}"#;

#[derive(Debug, Deserialize)]
struct ConsolidationLlmResult {
    title: Option<String>,
    merged_facts: Option<Vec<String>>,
    removed_duplicates: Option<Vec<String>>,
    importance: Option<i32>,
}

pub async fn consolidate(db: &Database, memory_ids: &[String]) -> Result<Memory> {
    let source_ids = parse_memory_ids(memory_ids)?;
    if source_ids.len() < 2 {
        return Err(crate::EngError::InvalidInput(
            "at least two memory ids are required for consolidation".to_string(),
        ));
    }

    let memories = load_memories(db, &source_ids).await?;
    let first = memories.first().ok_or_else(|| {
        crate::EngError::InvalidInput("no memories available to consolidate".to_string())
    })?;

    ensure_same_scope(&memories)?;

    if let Some(existing_id) = find_existing_consolidation(db, first.user_id, &source_ids).await? {
        return fetch_memory(db, existing_id).await;
    }

    let prompt_input = build_cluster_prompt(db, &memories).await?;
    let llm_result = call_llm(
        CONSOLIDATION_PROMPT,
        &prompt_input,
        Some(LlmOptions {
            temperature: 0.2,
            max_tokens: 1200,
        }),
    )
    .await
    .ok()
    .and_then(|raw| repair_and_parse_json::<ConsolidationLlmResult>(&raw));

    let merged_facts = build_merged_facts(&memories, llm_result.as_ref());
    let title = build_title(&memories, llm_result.as_ref());
    let importance = llm_result
        .as_ref()
        .and_then(|r| r.importance)
        .unwrap_or_else(|| rounded_average_importance(&memories))
        .clamp(1, 10);
    let model = if llm_result.is_some() {
        Some("llm".to_string())
    } else {
        Some("heuristic".to_string())
    };

    let content = format!(
        "[Consolidated: {}]\n- {}",
        title,
        merged_facts.join("\n- ")
    );

    let stored = memory::store(
        db,
        StoreRequest {
            content,
            category: "discovery".to_string(),
            source: "consolidation".to_string(),
            importance,
            tags: None,
            embedding: None,
            session_id: first.session_id.clone(),
            is_static: Some(false),
            user_id: Some(first.user_id),
            space_id: first.space_id,
            parent_memory_id: None,
        },
    )
    .await?;

    update_source_count(db, stored.id, memories.len() as i32).await?;
    db.conn
        .execute(
            "UPDATE memories SET tags = ?1, episode_id = ?2, confidence = ?3, model = ?4, updated_at = datetime('now') WHERE id = ?5",
            params![
                serde_json::to_string(&vec!["consolidated".to_string(), slugify(&title)]).ok(),
                first.episode_id,
                max_confidence(&memories),
                model,
                stored.id
            ],
        )
        .await?;

    for source in &memories {
        insert_link(db, stored.id, source.id, 1.0, "consolidates").await?;
    }

    let removed_duplicates = llm_result
        .as_ref()
        .and_then(|r| r.removed_duplicates.clone())
        .unwrap_or_default();
    let confidence = consolidation_confidence(memories.len(), removed_duplicates.len());

    db.conn
        .execute(
            "INSERT INTO consolidations (source_ids, result_memory_id, strategy, confidence, user_id) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                serde_json::to_string(&source_ids)?,
                stored.id,
                "merge",
                confidence,
                first.user_id
            ],
        )
        .await?;

    fetch_memory(db, stored.id).await
}

pub async fn find_consolidation_candidates(
    db: &Database,
    threshold: f32,
) -> Result<Vec<Vec<String>>> {
    let sim_floor = f64::from(threshold.clamp(0.0, 1.0));
    let mut rows = db
        .conn
        .query(
            "SELECT ml.source_id, ml.target_id
             FROM memory_links ml
             JOIN memories ms ON ms.id = ml.source_id
             JOIN memories mt ON mt.id = ml.target_id
             WHERE ml.type = 'similarity'
               AND ml.similarity >= ?1
               AND ms.user_id = mt.user_id
               AND ms.is_forgotten = 0
               AND mt.is_forgotten = 0
               AND ms.is_archived = 0
               AND mt.is_archived = 0
               AND ms.is_latest = 1
               AND mt.is_latest = 1
               AND ms.category = mt.category",
            params![sim_floor],
        )
        .await?;

    let mut graph: HashMap<i64, HashSet<i64>> = HashMap::new();
    while let Some(row) = rows.next().await? {
        let a: i64 = row.get(0)?;
        let b: i64 = row.get(1)?;
        if a == b {
            continue;
        }
        graph.entry(a).or_default().insert(b);
        graph.entry(b).or_default().insert(a);
    }

    let mut seen = HashSet::new();
    let mut clusters = Vec::new();
    for &start in graph.keys() {
        if !seen.insert(start) {
            continue;
        }
        let mut stack = vec![start];
        let mut component = Vec::new();

        while let Some(node) = stack.pop() {
            component.push(node);
            if let Some(neighbors) = graph.get(&node) {
                for &neighbor in neighbors {
                    if seen.insert(neighbor) {
                        stack.push(neighbor);
                    }
                }
            }
        }

        if component.len() >= 2 {
            component.sort_unstable();
            clusters.push(
                component
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>(),
            );
        }
    }

    clusters.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    Ok(clusters)
}

#[derive(Debug, Clone, Serialize)]
pub struct ConsolidationRecord {
    pub id: i64,
    pub summary: String,
}

pub async fn list_consolidations(
    db: &Database,
    user_id: i64,
    limit: usize,
) -> Result<Vec<ConsolidationRecord>> {
    let mut rows = db
        .conn
        .query(
            "SELECT c.id, m.content
             FROM consolidations c
             JOIN memories m ON m.id = c.result_memory_id
             WHERE c.user_id = ?1
             ORDER BY c.created_at DESC
             LIMIT ?2",
            params![user_id, limit as i64],
        )
        .await?;

    let mut records = Vec::new();
    while let Some(row) = rows.next().await? {
        records.push(ConsolidationRecord {
            id: row.get(0)?,
            summary: row.get(1)?,
        });
    }
    Ok(records)
}

fn parse_memory_ids(memory_ids: &[String]) -> Result<Vec<i64>> {
    let mut parsed = Vec::new();
    let mut seen = HashSet::new();
    for raw in memory_ids {
        let id = raw.parse::<i64>().map_err(|_| {
            crate::EngError::InvalidInput(format!("invalid memory id: {}", raw))
        })?;
        if seen.insert(id) {
            parsed.push(id);
        }
    }
    Ok(parsed)
}

async fn load_memories(db: &Database, ids: &[i64]) -> Result<Vec<Memory>> {
    let mut memories = Vec::with_capacity(ids.len());
    for &id in ids {
        memories.push(fetch_memory(db, id).await?);
    }
    Ok(memories)
}

async fn fetch_memory(db: &Database, id: i64) -> Result<Memory> {
    let sql = format!("SELECT {} FROM memories WHERE id = ?1", MEMORY_COLUMNS);
    let mut rows = db.conn.query(&sql, params![id]).await?;
    if let Some(row) = rows.next().await? {
        row_to_memory(&row)
    } else {
        Err(crate::EngError::NotFound(format!("memory {} not found", id)))
    }
}

fn ensure_same_scope(memories: &[Memory]) -> Result<()> {
    let first = memories.first().ok_or_else(|| {
        crate::EngError::InvalidInput("empty memory cluster".to_string())
    })?;
    for memory in memories.iter().skip(1) {
        if memory.user_id != first.user_id || memory.space_id != first.space_id {
            return Err(crate::EngError::InvalidInput(
                "all memories in a consolidation must belong to the same user and space".to_string(),
            ));
        }
    }
    Ok(())
}

async fn find_existing_consolidation(
    db: &Database,
    user_id: i64,
    source_ids: &[i64],
) -> Result<Option<i64>> {
    let source_json = serde_json::to_string(source_ids)?;
    let mut rows = db
        .conn
        .query(
            "SELECT result_memory_id FROM consolidations WHERE user_id = ?1 AND source_ids = ?2 LIMIT 1",
            params![user_id, source_json],
        )
        .await?;
    if let Some(row) = rows.next().await? {
        Ok(Some(row.get(0)?))
    } else {
        Ok(None)
    }
}

async fn build_cluster_prompt(db: &Database, memories: &[Memory]) -> Result<String> {
    let mut blocks = Vec::with_capacity(memories.len());
    for memory in memories {
        let facts = fetch_child_facts(db, memory.id).await?;
        let fact_suffix = if facts.is_empty() {
            String::new()
        } else {
            format!("\n  Facts: {}", facts.join("; "))
        };
        blocks.push(format!(
            "[#{} | {} | imp={} | {}]: {}{}",
            memory.id, memory.category, memory.importance, memory.created_at, memory.content, fact_suffix
        ));
    }
    Ok(blocks.join("\n\n"))
}

async fn fetch_child_facts(db: &Database, parent_id: i64) -> Result<Vec<String>> {
    let mut rows = db
        .conn
        .query(
            "SELECT content
             FROM memories
             WHERE parent_memory_id = ?1 AND is_fact = 1
             ORDER BY id ASC",
            params![parent_id],
        )
        .await?;

    let mut facts = Vec::new();
    while let Some(row) = rows.next().await? {
        facts.push(row.get(0)?);
    }
    Ok(facts)
}

fn build_merged_facts(memories: &[Memory], llm_result: Option<&ConsolidationLlmResult>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut facts = Vec::new();

    if let Some(llm) = llm_result {
        for fact in llm.merged_facts.clone().unwrap_or_default() {
            let trimmed = fact.trim();
            if !trimmed.is_empty() && seen.insert(trimmed.to_lowercase()) {
                facts.push(trimmed.to_string());
            }
        }
    }

    if facts.is_empty() {
        for memory in memories {
            let trimmed = normalize_line(&memory.content);
            if trimmed.len() >= 5 && seen.insert(trimmed.to_lowercase()) {
                facts.push(trimmed);
            }
        }
    }

    if facts.is_empty() {
        facts.push("Consolidated memory cluster".to_string());
    }

    facts
}

fn build_title(memories: &[Memory], llm_result: Option<&ConsolidationLlmResult>) -> String {
    if let Some(title) = llm_result.and_then(|r| r.title.as_deref()) {
        let trimmed = title.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    let fallback = memories
        .first()
        .map(|m| normalize_line(&m.content))
        .unwrap_or_else(|| "consolidated memory".to_string());
    fallback
        .split_whitespace()
        .take(5)
        .collect::<Vec<_>>()
        .join(" ")
}

fn rounded_average_importance(memories: &[Memory]) -> i32 {
    let sum: i32 = memories.iter().map(|m| m.importance).sum();
    ((sum as f64 / memories.len() as f64).round() as i32).clamp(1, 10)
}

fn max_confidence(memories: &[Memory]) -> f64 {
    memories
        .iter()
        .map(|m| m.confidence)
        .fold(1.0_f64, f64::max)
}

fn consolidation_confidence(source_count: usize, duplicate_count: usize) -> f64 {
    let cluster_factor = (source_count as f64 / 6.0).min(1.0);
    let dedupe_factor = (duplicate_count as f64 / (source_count.max(1) as f64)).min(1.0) * 0.2;
    (0.7 + cluster_factor * 0.2 + dedupe_factor).clamp(0.7, 0.98)
}

fn normalize_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn slugify(text: &str) -> String {
    let mut slug = String::with_capacity(text.len());
    let mut prev_dash = false;
    for ch in text.chars().flat_map(|c| c.to_lowercase()) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    slug.trim_matches('-').to_string()
}
