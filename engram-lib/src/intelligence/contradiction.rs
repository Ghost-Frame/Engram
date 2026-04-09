use crate::db::Database;
use crate::memory::types::{Memory, StoreRequest};
use crate::memory::{self, get, insert_link, mark_archived};
use crate::Result;
use libsql::params;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use super::llm::{call_llm, repair_and_parse_json, LlmOptions};
use super::types::ContradictionResolution;

const DEFAULT_SCAN_THRESHOLD: f32 = 0.6;
const MAX_SCAN_SIMILARITY: f32 = 0.95;

const CONTRADICTION_VERIFY_PROMPT: &str = r#"You detect contradictions between memory pairs. Determine whether each pair directly contradicts each other.

Not contradictions:
- updates where one memory supersedes an older one
- extensions where one memory adds more detail
- unrelated but similar memories

Is a contradiction:
- the memories claim incompatible values for the same property
- one memory says X and the other says not-X

Respond with ONLY a JSON object:
{
  "contradicts": true,
  "explanation": "brief reason"
}"#;

const CONTRADICTION_MERGE_PROMPT: &str = r#"Merge these two contradicting memories into a single accurate memory. Preserve the most recent or most reliable details.

Respond with ONLY a JSON object:
{
  "content": "merged memory text",
  "category": "category"
}"#;

#[derive(Debug, Clone, Serialize)]
pub struct Contradiction {
    pub memory_a: String,
    pub memory_b: String,
    pub confidence: f32,
    pub description: String,
}

#[derive(Debug, Deserialize)]
struct ContradictionVerification {
    contradicts: bool,
    explanation: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MergedMemory {
    content: String,
    category: Option<String>,
}

pub async fn detect_contradictions(
    db: &Database,
    memory: &Memory,
) -> Result<Vec<Contradiction>> {
    let mut contradictions = known_contradictions_for_memory(db, memory.id).await?;
    let mut seen = contradiction_pair_keys(&contradictions);

    let mut candidates = similarity_candidates_for_memory(db, memory, DEFAULT_SCAN_THRESHOLD).await?;
    candidates.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    for (other, similarity, source) in candidates {
        let key = pair_key(memory.id, other.id);
        if !seen.insert(key) {
            continue;
        }

        let verdict = verify_contradiction_pair(memory, &other).await;
        if let Some((confidence, description)) = verdict {
            contradictions.push(Contradiction {
                memory_a: memory.id.to_string(),
                memory_b: other.id.to_string(),
                confidence: confidence.max(similarity),
                description: format!("{} ({})", description, source),
            });
        }
    }

    contradictions.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(contradictions)
}

pub async fn scan_all_contradictions(db: &Database, user_id: i64) -> Result<Vec<Contradiction>> {
    let mut contradictions = known_contradictions(db).await?;
    let mut seen = contradiction_pair_keys(&contradictions);

    let mut rows = db
        .conn
        .query(
            "SELECT ml.source_id, ml.target_id, ml.similarity,
                    ms.id, ms.content, ms.category, ms.created_at, ms.importance, ms.source, ms.confidence, ms.user_id, ms.space_id,
                    mt.id, mt.content, mt.category, mt.created_at, mt.importance, mt.source, mt.confidence, mt.user_id, mt.space_id
             FROM memory_links ml
             JOIN memories ms ON ms.id = ml.source_id
             JOIN memories mt ON mt.id = ml.target_id
             WHERE ml.type = 'similarity'
               AND ml.similarity >= ?1
               AND ml.similarity < ?2
               AND ms.user_id = ?3
               AND ms.category = mt.category
               AND ms.user_id = mt.user_id
               AND COALESCE(ms.space_id, -1) = COALESCE(mt.space_id, -1)
               AND ms.is_forgotten = 0
               AND mt.is_forgotten = 0
               AND ms.is_archived = 0
               AND mt.is_archived = 0
               AND ms.is_latest = 1
               AND mt.is_latest = 1
             ORDER BY ml.similarity DESC
             LIMIT 200",
            params![DEFAULT_SCAN_THRESHOLD, MAX_SCAN_SIMILARITY, user_id],
        )
        .await?;

    while let Some(row) = rows.next().await? {
        let source_id: i64 = row.get(0)?;
        let target_id: i64 = row.get(1)?;
        let similarity: f32 = row.get::<f64>(2)? as f32;

        if !seen.insert(pair_key(source_id, target_id)) {
            continue;
        }

        let a = Memory {
            id: row.get(3)?,
            content: row.get(4)?,
            category: row.get(5)?,
            source: row.get(8)?,
            session_id: None,
            importance: row.get(7)?,
            embedding: None,
            version: 1,
            is_latest: true,
            parent_memory_id: None,
            root_memory_id: None,
            source_count: 1,
            is_static: false,
            is_forgotten: false,
            is_archived: false,
            is_inference: false,
            is_fact: false,
            is_decomposed: false,
            forget_after: None,
            forget_reason: None,
            model: None,
            recall_hits: 0,
            recall_misses: 0,
            adaptive_score: None,
            pagerank_score: None,
            last_accessed_at: None,
            access_count: 0,
            tags: None,
            episode_id: None,
            decay_score: None,
            confidence: row.get(9)?,
            sync_id: None,
            status: "approved".to_string(),
            user_id: row.get(10)?,
            space_id: row.get(11)?,
            fsrs_stability: None,
            fsrs_difficulty: None,
            fsrs_storage_strength: None,
            fsrs_retrieval_strength: None,
            fsrs_learning_state: None,
            fsrs_reps: None,
            fsrs_lapses: None,
            fsrs_last_review_at: None,
            valence: None,
            arousal: None,
            dominant_emotion: None,
            created_at: row.get(6)?,
            updated_at: row.get(6)?,
        };
        let b = Memory {
            id: row.get(12)?,
            content: row.get(13)?,
            category: row.get(14)?,
            source: row.get(17)?,
            session_id: None,
            importance: row.get(16)?,
            embedding: None,
            version: 1,
            is_latest: true,
            parent_memory_id: None,
            root_memory_id: None,
            source_count: 1,
            is_static: false,
            is_forgotten: false,
            is_archived: false,
            is_inference: false,
            is_fact: false,
            is_decomposed: false,
            forget_after: None,
            forget_reason: None,
            model: None,
            recall_hits: 0,
            recall_misses: 0,
            adaptive_score: None,
            pagerank_score: None,
            last_accessed_at: None,
            access_count: 0,
            tags: None,
            episode_id: None,
            decay_score: None,
            confidence: row.get(18)?,
            sync_id: None,
            status: "approved".to_string(),
            user_id: row.get(19)?,
            space_id: row.get(20)?,
            fsrs_stability: None,
            fsrs_difficulty: None,
            fsrs_storage_strength: None,
            fsrs_retrieval_strength: None,
            fsrs_learning_state: None,
            fsrs_reps: None,
            fsrs_lapses: None,
            fsrs_last_review_at: None,
            valence: None,
            arousal: None,
            dominant_emotion: None,
            created_at: row.get(15)?,
            updated_at: row.get(15)?,
        };

        if let Some((confidence, description)) = verify_contradiction_pair(&a, &b).await {
            contradictions.push(Contradiction {
                memory_a: a.id.to_string(),
                memory_b: b.id.to_string(),
                confidence: confidence.max(similarity),
                description: format!("{} (scan)", description),
            });
        }
    }

    contradictions.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(contradictions)
}

pub async fn resolve_contradiction(
    db: &Database,
    memory_a_id: i64,
    memory_b_id: i64,
    resolution: ContradictionResolution,
    user_id: i64,
) -> Result<Option<Memory>> {
    match resolution {
        ContradictionResolution::KeepA => {
            mark_archived(db, memory_b_id, user_id).await?;
            insert_link(db, memory_a_id, memory_b_id, 1.0, "resolves", user_id).await?;
            clear_contradiction_links(db, memory_a_id, memory_b_id).await?;
            Ok(None)
        }
        ContradictionResolution::KeepB => {
            mark_archived(db, memory_a_id, user_id).await?;
            insert_link(db, memory_b_id, memory_a_id, 1.0, "resolves", user_id).await?;
            clear_contradiction_links(db, memory_a_id, memory_b_id).await?;
            Ok(None)
        }
        ContradictionResolution::KeepBoth => {
            clear_contradiction_links(db, memory_a_id, memory_b_id).await?;
            insert_link(db, memory_a_id, memory_b_id, 0.9, "related", user_id).await?;
            Ok(None)
        }
        ContradictionResolution::Merge => {
            let mem_a = get(db, memory_a_id, user_id).await?;
            let mem_b = get(db, memory_b_id, user_id).await?;
            let merged = merge_memories(&mem_a, &mem_b).await?;

            let stored = memory::store(
                db,
                StoreRequest {
                    content: merged.content,
                    category: merged.category.unwrap_or_else(|| mem_b.category.clone()),
                    source: "contradiction-merge".to_string(),
                    importance: mem_a.importance.max(mem_b.importance),
                    tags: None,
                    embedding: None,
                    session_id: mem_b.session_id.clone().or(mem_a.session_id.clone()),
                    is_static: Some(false),
                    user_id: Some(mem_a.user_id),
                    space_id: mem_a.space_id.or(mem_b.space_id),
                    parent_memory_id: None,
                },
            )
            .await?;

            mark_archived(db, memory_a_id, user_id).await?;
            mark_archived(db, memory_b_id, user_id).await?;
            insert_link(db, stored.id, memory_a_id, 1.0, "resolves", user_id).await?;
            insert_link(db, stored.id, memory_b_id, 1.0, "resolves", user_id).await?;
            clear_contradiction_links(db, memory_a_id, memory_b_id).await?;

            Ok(Some(get(db, stored.id, user_id).await?))
        }
    }
}

async fn known_contradictions_for_memory(db: &Database, memory_id: i64) -> Result<Vec<Contradiction>> {
    let mut rows = db
        .conn
        .query(
            "SELECT ml.source_id, ml.target_id, ml.similarity
             FROM memory_links ml
             JOIN memories ms ON ms.id = ml.source_id
             JOIN memories mt ON mt.id = ml.target_id
             WHERE ml.type = 'contradicts'
               AND (ml.source_id = ?1 OR ml.target_id = ?1)
               AND ms.is_forgotten = 0
               AND mt.is_forgotten = 0",
            params![memory_id],
        )
        .await?;

    let mut contradictions = Vec::new();
    while let Some(row) = rows.next().await? {
        let a: i64 = row.get(0)?;
        let b: i64 = row.get(1)?;
        let similarity: f32 = row.get::<f64>(2)? as f32;
        contradictions.push(Contradiction {
            memory_a: a.to_string(),
            memory_b: b.to_string(),
            confidence: similarity,
            description: "existing contradiction link".to_string(),
        });
    }
    Ok(contradictions)
}

async fn known_contradictions(db: &Database) -> Result<Vec<Contradiction>> {
    let mut rows = db
        .conn
        .query(
            "SELECT source_id, target_id, similarity
             FROM memory_links
             WHERE type = 'contradicts'
             ORDER BY created_at DESC
             LIMIT 100",
            (),
        )
        .await?;

    let mut contradictions = Vec::new();
    while let Some(row) = rows.next().await? {
        contradictions.push(Contradiction {
            memory_a: row.get::<i64>(0)?.to_string(),
            memory_b: row.get::<i64>(1)?.to_string(),
            confidence: row.get::<f64>(2)? as f32,
            description: "existing contradiction link".to_string(),
        });
    }
    Ok(contradictions)
}

async fn similarity_candidates_for_memory(
    db: &Database,
    memory: &Memory,
    threshold: f32,
) -> Result<Vec<(Memory, f32, &'static str)>> {
    let mut rows = db
        .conn
        .query(
            "SELECT other.id, other.content, other.category, other.source, other.session_id,
                    other.importance, other.version, other.is_latest, other.parent_memory_id, other.root_memory_id,
                    other.source_count, other.is_static, other.is_forgotten, other.is_archived, other.is_inference,
                    other.is_fact, other.is_decomposed, other.forget_after, other.forget_reason, other.model,
                    other.recall_hits, other.recall_misses, other.adaptive_score, other.pagerank_score,
                    other.last_accessed_at, other.access_count, other.tags, other.episode_id, other.decay_score,
                    other.confidence, other.sync_id, other.status, other.user_id, other.space_id,
                    other.fsrs_stability, other.fsrs_difficulty, other.fsrs_storage_strength, other.fsrs_retrieval_strength,
                    other.fsrs_learning_state, other.fsrs_reps, other.fsrs_lapses, other.fsrs_last_review_at,
                    other.valence, other.arousal, other.dominant_emotion, other.created_at, other.updated_at,
                    ml.similarity
             FROM memory_links ml
             JOIN memories source ON source.id = ml.source_id
             JOIN memories other ON other.id = ml.target_id
             WHERE ml.source_id = ?1
               AND ml.type = 'similarity'
               AND ml.similarity >= ?2
               AND ml.similarity < ?3
               AND other.category = ?4
               AND other.user_id = ?5
               AND COALESCE(other.space_id, -1) = COALESCE(?6, -1)
               AND other.is_forgotten = 0
               AND other.is_archived = 0
               AND other.is_latest = 1
             ORDER BY ml.similarity DESC
             LIMIT 30",
            params![
                memory.id,
                f64::from(threshold),
                f64::from(MAX_SCAN_SIMILARITY),
                memory.category.clone(),
                memory.user_id,
                memory.space_id
            ],
        )
        .await?;

    let mut candidates = Vec::new();
    while let Some(row) = rows.next().await? {
        let other = Memory {
            id: row.get(0)?,
            content: row.get(1)?,
            category: row.get(2)?,
            source: row.get(3)?,
            session_id: row.get(4)?,
            importance: row.get(5)?,
            embedding: None,
            version: row.get(6)?,
            is_latest: row.get::<i32>(7)? != 0,
            parent_memory_id: row.get(8)?,
            root_memory_id: row.get(9)?,
            source_count: row.get(10)?,
            is_static: row.get::<i32>(11)? != 0,
            is_forgotten: row.get::<i32>(12)? != 0,
            is_archived: row.get::<i32>(13)? != 0,
            is_inference: row.get::<i32>(14)? != 0,
            is_fact: row.get::<i32>(15)? != 0,
            is_decomposed: row.get::<i32>(16)? != 0,
            forget_after: row.get(17)?,
            forget_reason: row.get(18)?,
            model: row.get(19)?,
            recall_hits: row.get(20)?,
            recall_misses: row.get(21)?,
            adaptive_score: row.get(22)?,
            pagerank_score: row.get(23)?,
            last_accessed_at: row.get(24)?,
            access_count: row.get(25)?,
            tags: row.get(26)?,
            episode_id: row.get(27)?,
            decay_score: row.get(28)?,
            confidence: row.get(29)?,
            sync_id: row.get(30)?,
            status: row.get(31)?,
            user_id: row.get(32)?,
            space_id: row.get(33)?,
            fsrs_stability: row.get(34)?,
            fsrs_difficulty: row.get(35)?,
            fsrs_storage_strength: row.get(36)?,
            fsrs_retrieval_strength: row.get(37)?,
            fsrs_learning_state: row.get(38)?,
            fsrs_reps: row.get(39)?,
            fsrs_lapses: row.get(40)?,
            fsrs_last_review_at: row.get(41)?,
            valence: row.get(42)?,
            arousal: row.get(43)?,
            dominant_emotion: row.get(44)?,
            created_at: row.get(45)?,
            updated_at: row.get(46)?,
        };
        let similarity: f32 = row.get::<f64>(47)? as f32;
        candidates.push((other, similarity, "similarity link"));
    }
    Ok(candidates)
}

async fn verify_contradiction_pair(a: &Memory, b: &Memory) -> Option<(f32, String)> {
    if let Some(explanation) = heuristic_contradiction(a, b) {
        return Some((0.72, explanation));
    }

    let user = format!(
        "Memory A (#{}, created {}): {}\n\nMemory B (#{}, created {}): {}",
        a.id, a.created_at, a.content, b.id, b.created_at, b.content
    );

    let raw = call_llm(
        CONTRADICTION_VERIFY_PROMPT,
        &user,
        Some(LlmOptions {
            temperature: 0.0,
            max_tokens: 300,
        }),
    )
    .await
    .ok()?;

    let parsed = repair_and_parse_json::<ContradictionVerification>(&raw)?;
    if parsed.contradicts {
        Some((
            0.83,
            parsed
                .explanation
                .unwrap_or_else(|| "LLM-verified contradiction".to_string()),
        ))
    } else {
        None
    }
}

fn heuristic_contradiction(a: &Memory, b: &Memory) -> Option<String> {
    let a_norm = normalize(&a.content);
    let b_norm = normalize(&b.content);

    if a_norm == b_norm {
        return None;
    }

    let pairs = [
        (" enabled ", " disabled "),
        (" on ", " off "),
        (" true ", " false "),
        (" yes ", " no "),
        (" installed ", " removed "),
        (" uses ", " does not use "),
        (" is ", " is not "),
    ];

    for (left, right) in pairs {
        let a_has_left = a_norm.contains(left);
        let a_has_right = a_norm.contains(right);
        let b_has_left = b_norm.contains(left);
        let b_has_right = b_norm.contains(right);
        if (a_has_left && b_has_right) || (a_has_right && b_has_left) {
            return Some("opposing state language".to_string());
        }
    }

    let a_numbers = extract_numbers(&a_norm);
    let b_numbers = extract_numbers(&b_norm);
    if !a_numbers.is_empty() && !b_numbers.is_empty() && a_numbers != b_numbers {
        let overlap = token_overlap(&a_norm, &b_norm);
        if overlap >= 0.5 {
            return Some("same topic with conflicting numeric values".to_string());
        }
    }

    None
}

async fn merge_memories(a: &Memory, b: &Memory) -> Result<MergedMemory> {
    let user = format!(
        "Memory A (#{}, created {}): {}\n\nMemory B (#{}, created {}): {}",
        a.id, a.created_at, a.content, b.id, b.created_at, b.content
    );

    if let Ok(raw) = call_llm(
        CONTRADICTION_MERGE_PROMPT,
        &user,
        Some(LlmOptions {
            temperature: 0.1,
            max_tokens: 500,
        }),
    )
    .await
    {
        if let Some(parsed) = repair_and_parse_json::<MergedMemory>(&raw) {
            if !parsed.content.trim().is_empty() {
                return Ok(parsed);
            }
        }
    }

    let newer_first = if a.created_at >= b.created_at { a } else { b };
    Ok(MergedMemory {
        content: newer_first.content.clone(),
        category: Some(newer_first.category.clone()),
    })
}

async fn clear_contradiction_links(db: &Database, memory_a_id: i64, memory_b_id: i64) -> Result<()> {
    db.conn
        .execute(
            "DELETE FROM memory_links WHERE type = 'contradicts' AND ((source_id = ?1 AND target_id = ?2) OR (source_id = ?2 AND target_id = ?1))",
            params![memory_a_id, memory_b_id],
        )
        .await?;
    Ok(())
}

fn contradiction_pair_keys(contradictions: &[Contradiction]) -> HashSet<(String, String)> {
    contradictions
        .iter()
        .map(|c| {
            if c.memory_a <= c.memory_b {
                (c.memory_a.clone(), c.memory_b.clone())
            } else {
                (c.memory_b.clone(), c.memory_a.clone())
            }
        })
        .collect()
}

fn pair_key(a: i64, b: i64) -> (String, String) {
    let a = a.to_string();
    let b = b.to_string();
    if a <= b { (a, b) } else { (b, a) }
}

fn normalize(text: &str) -> String {
    format!(" {} ", text.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" "))
}

fn extract_numbers(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_digit() && c != '.')
        .filter(|part| !part.is_empty())
        .map(|s| s.to_string())
        .collect()
}

fn token_overlap(a: &str, b: &str) -> f32 {
    let a_tokens: HashSet<&str> = a.split_whitespace().collect();
    let b_tokens: HashSet<&str> = b.split_whitespace().collect();
    if a_tokens.is_empty() || b_tokens.is_empty() {
        return 0.0;
    }
    let intersection = a_tokens.intersection(&b_tokens).count() as f32;
    let union = a_tokens.union(&b_tokens).count() as f32;
    if union == 0.0 { 0.0 } else { intersection / union }
}
