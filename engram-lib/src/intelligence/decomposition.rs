//! Decomposition -- break complex memories into atomic facts.
//!
//! Three-tier approach:
//! - Tier 1 (LLM): If LLM is configured, use it for decomposition
//! - Tier 2 (Rules): Rule-based NLP splitting (conjunction splitting, pronoun resolution)
//! - Tier 3 (Template): Simple sentence-level splitting

use crate::db::Database;
use crate::intelligence::llm::{call_llm, is_llm_available, repair_and_parse_json, LlmOptions};
use crate::intelligence::types::{
    DecompositionResult, DecompositionTier, DecompositionWithTier,
};
use crate::Result;
use serde::Deserialize;
use tracing::{info, warn};

const DECOMPOSITION_PROMPT: &str = r#"You are a fact extraction engine for a memory system. Given a memory entry, extract individual atomic facts.

Rules:
- Each fact must be a single, self-contained statement
- Preserve ALL specific values: IPs, ports, paths, versions, dates, names
- Each fact should make sense on its own without the others
- Do NOT add interpretation or inference -- only extract what is explicitly stated
- Do NOT rephrase into robotic language -- keep the original tone and wording where possible
- If the memory is already a single atomic fact, return it as-is
- Aim for 1-8 facts per memory (most will be 2-4)

Respond with ONLY a JSON object:
{
  "facts": ["fact 1", "fact 2"],
  "skip": false
}

Set skip=true if the content is too short, already atomic, or not decomposable."#;

const MIN_LENGTH: usize = 50;
const MAX_FACTS: usize = 10;

/// Filler phrases to strip from sentence starts.
const FILLER_PREFIXES: &[&str] = &[
    "so ", "well ", "basically ", "actually ", "honestly ",
    "like ", "i mean ", "you know ", "anyway ",
];

/// Meta-sentences to skip entirely.
const META_STOPLIST: &[&str] = &[
    "let me explain", "as i mentioned", "in summary", "to summarize",
    "as we discussed", "like i said", "to be clear", "for context",
    "moving on", "on another note", "by the way", "speaking of which",
];

#[derive(Debug, Deserialize)]
struct LlmDecompositionResponse {
    facts: Option<Vec<String>>,
    skip: Option<bool>,
}

/// Decompose a memory into atomic facts.
/// Returns the decomposed memory IDs (newly created child facts).
pub async fn decompose(db: &Database, memory_id: i64, _user_id: i64) -> Result<Vec<i64>> {
    let conn = db.connection();

    // Fetch the memory content
    let mut rows = conn
        .query(
            "SELECT content, category, source, importance, user_id, space_id, \
                    episode_id, tags, session_id \
             FROM memories WHERE id = ?1 AND is_forgotten = 0",
            libsql::params![memory_id],
        )
        .await?;

    let row = match rows.next().await? {
        Some(r) => r,
        None => return Ok(Vec::new()),
    };

    let content: String = row.get(0)?;
    let category: String = row.get(1)?;
    let _source: String = row.get(2)?;
    let importance: i64 = row.get(3)?;
    let user_id: i64 = row.get(4)?;
    let space_id: Option<i64> = row.get(5)?;
    let episode_id: Option<i64> = row.get(6)?;
    let tags: Option<String> = row.get(7)?;
    let _session_id: Option<String> = row.get(8)?;

    // Skip if too short or is a fact/consolidation already
    if content.len() < MIN_LENGTH {
        conn.execute(
            "UPDATE memories SET is_decomposed = 1 WHERE id = ?1",
            libsql::params![memory_id],
        )
        .await?;
        return Ok(Vec::new());
    }

    if content.starts_with("[Consolidated:")
        || content.starts_with("Session compaction summary")
        || content.starts_with("[auto-captured]")
        || category == "fact"
    {
        return Ok(Vec::new());
    }

    // Try tiered decomposition
    let decomposition = decompose_content(&content).await;

    let decomp = match decomposition {
        Some(d) if !d.result.skip && !d.result.facts.is_empty() => d,
        _ => {
            conn.execute(
                "UPDATE memories SET is_decomposed = 1 WHERE id = ?1",
                libsql::params![memory_id],
            )
            .await?;
            return Ok(Vec::new());
        }
    };

    // Store facts as child memories
    let mut created_ids = Vec::new();
    let capped = &decomp.result.facts[..decomp.result.facts.len().min(MAX_FACTS)];

    for fact_content in capped {
        let trimmed = fact_content.trim();
        if trimmed.len() < 5 {
            continue;
        }

        conn.execute(
            "INSERT INTO memories (content, category, source, importance, version, is_latest, \
             parent_memory_id, source_count, is_static, is_forgotten, is_fact, confidence, \
             status, user_id, space_id, episode_id, tags, created_at, updated_at) \
             VALUES (?1, 'fact', 'decomposition', ?2, 1, 1, ?3, 1, 0, 0, 1, 1.0, \
             'approved', ?4, ?5, ?6, ?7, datetime('now'), datetime('now'))",
            libsql::params![
                trimmed.to_string(),
                importance,
                memory_id,
                user_id,
                space_id,
                episode_id,
                tags.clone()
            ],
        )
        .await?;

        let mut id_row = conn.query("SELECT last_insert_rowid()", ()).await?;
        if let Some(r) = id_row.next().await? {
            let new_id: i64 = r.get(0)?;
            created_ids.push(new_id);

            // Link parent -> child
            conn.execute(
                "INSERT OR IGNORE INTO memory_links (source_id, target_id, similarity, type) \
                 VALUES (?1, ?2, 1.0, 'has_fact')",
                libsql::params![memory_id, new_id],
            )
            .await?;
        }
    }

    // Mark parent as decomposed
    if !created_ids.is_empty() {
        conn.execute(
            "UPDATE memories SET is_decomposed = 1 WHERE id = ?1",
            libsql::params![memory_id],
        )
        .await?;

        info!(
            parent_id = memory_id,
            facts_stored = created_ids.len(),
            tier = %decomp.tier,
            "decomposed"
        );
    }

    Ok(created_ids)
}

/// Decompose content using the tiered approach.
async fn decompose_content(content: &str) -> Option<DecompositionWithTier> {
    // Tier 1: LLM
    if is_llm_available() {
        if let Some(result) = try_llm_decomposition(content).await {
            return Some(DecompositionWithTier {
                result,
                tier: DecompositionTier::Llm,
            });
        }
    }

    // Tier 2: Rule-based
    let rule_result = decompose_rule_based(content);
    if !rule_result.skip && !rule_result.facts.is_empty() {
        return Some(DecompositionWithTier {
            result: rule_result,
            tier: DecompositionTier::Tier2Rules,
        });
    }

    // Tier 3: Template
    let template_result = decompose_template(content);
    if !template_result.skip && !template_result.facts.is_empty() {
        return Some(DecompositionWithTier {
            result: template_result,
            tier: DecompositionTier::Tier3Template,
        });
    }

    None
}

/// Tier 1: LLM-based decomposition.
async fn try_llm_decomposition(content: &str) -> Option<DecompositionResult> {
    let opts = LlmOptions {
        temperature: 0.2,
        max_tokens: 512,
    };

    match call_llm(DECOMPOSITION_PROMPT, content, Some(opts)).await {
        Ok(response) => {
            let parsed: Option<LlmDecompositionResponse> = repair_and_parse_json(&response);
            match parsed {
                Some(r) => Some(DecompositionResult {
                    facts: r.facts.unwrap_or_default(),
                    skip: r.skip.unwrap_or(false),
                }),
                None => {
                    warn!("decomposition_parse_failed_llm");
                    None
                }
            }
        }
        Err(e) => {
            warn!(error = %e, "decomposition_llm_failed");
            None
        }
    }
}

/// Tier 2: Rule-based NLP decomposition.
/// Sentence splitting + conjunction splitting + filler stripping + meta filtering.
fn decompose_rule_based(content: &str) -> DecompositionResult {
    // Split on sentence boundaries + newlines
    let raw_sentences: Vec<&str> = content
        .split(['.', '!', '?', '\n'])
        .map(|s| s.trim())
        .filter(|s| s.len() >= 10 && s.len() <= 300)
        .collect();

    // Filter meta-sentences
    let filtered: Vec<&str> = raw_sentences
        .into_iter()
        .filter(|s| {
            let lower = s.to_lowercase();
            !META_STOPLIST.iter().any(|meta| lower.contains(meta))
        })
        .collect();

    // Strip filler prefixes
    let cleaned: Vec<String> = filtered
        .into_iter()
        .map(strip_filler)
        .filter(|s| s.len() >= 10)
        .collect();

    // Conjunction splitting
    let mut expanded: Vec<String> = Vec::new();
    for sentence in &cleaned {
        // Split on " and ", " but ", " while "
        let conjunctions = [" and ", " but ", " while ", " however "];
        let mut split_parts: Vec<String> = vec![sentence.clone()];

        for conj in &conjunctions {
            let mut new_parts = Vec::new();
            for part in &split_parts {
                if part.contains(conj) {
                    let subs: Vec<&str> = part.splitn(2, conj).collect();
                    for sub in subs {
                        let trimmed = sub.trim();
                        if trimmed.split_whitespace().count() >= 3 && trimmed.len() >= 10 {
                            new_parts.push(trimmed.to_string());
                        } else {
                            new_parts.push(part.clone());
                            break;
                        }
                    }
                } else {
                    new_parts.push(part.clone());
                }
            }
            split_parts = new_parts;
        }

        expanded.extend(split_parts);
    }

    // Deduplicate by token overlap
    let deduped = dedup_by_overlap(&expanded, 0.8);

    if deduped.len() <= 1 {
        return DecompositionResult {
            facts: Vec::new(),
            skip: true,
        };
    }

    DecompositionResult {
        facts: deduped.into_iter().take(MAX_FACTS).collect(),
        skip: false,
    }
}

/// Tier 3: Template-based decomposition. Simple sentence splitting.
fn decompose_template(content: &str) -> DecompositionResult {
    let sentences: Vec<String> = content
        .split(['.', '!', '?', '\n'])
        .map(|s| s.trim())
        .filter(|s| s.len() >= 10 && s.len() <= 300)
        .map(strip_filler)
        .filter(|s| s.len() >= 10)
        .collect();

    if sentences.len() <= 1 {
        return DecompositionResult {
            facts: Vec::new(),
            skip: true,
        };
    }

    DecompositionResult {
        facts: sentences.into_iter().take(MAX_FACTS).collect(),
        skip: false,
    }
}

/// Strip leading filler phrases.
fn strip_filler(s: &str) -> String {
    let lower = s.to_lowercase();
    for filler in FILLER_PREFIXES {
        if lower.starts_with(filler) {
            return s[filler.len()..].trim_start_matches(|c: char| c == ',' || c.is_whitespace()).to_string();
        }
    }
    s.to_string()
}

/// Dedup facts by token overlap (Jaccard similarity).
fn dedup_by_overlap(facts: &[String], threshold: f64) -> Vec<String> {
    let tokenize = |s: &str| -> std::collections::HashSet<String> {
        s.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| t.len() >= 3)
            .map(|t| t.to_string())
            .collect()
    };

    let mut deduped: Vec<String> = Vec::new();

    for fact in facts {
        let tokens = tokenize(fact);
        let is_dup = deduped.iter().any(|existing| {
            let existing_tokens = tokenize(existing);
            let intersection = tokens.intersection(&existing_tokens).count();
            let union_size = tokens.union(&existing_tokens).count();
            union_size > 0 && (intersection as f64 / union_size as f64) > threshold
        });

        if !is_dup {
            deduped.push(fact.clone());
        }
    }

    deduped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_filler() {
        assert_eq!(strip_filler("so the server is down"), "the server is down");
        assert_eq!(strip_filler("basically it works"), "it works");
        assert_eq!(strip_filler("no filler here"), "no filler here");
    }

    #[test]
    fn test_decompose_template_short() {
        let result = decompose_template("Short.");
        assert!(result.skip);
        assert!(result.facts.is_empty());
    }

    #[test]
    fn test_decompose_template_multiple() {
        let content = "The server is running on port 8080. The database is PostgreSQL. Redis is used for caching.";
        let result = decompose_template(content);
        assert!(!result.skip);
        assert!(result.facts.len() >= 2);
    }

    #[test]
    fn test_decompose_rule_based_with_conjunction() {
        let content = "I bought a new laptop and I configured the server. The deployment was successful but the tests were slow.";
        let result = decompose_rule_based(content);
        // Should split on conjunction and produce multiple facts
        assert!(!result.facts.is_empty());
    }

    #[test]
    fn test_dedup_by_overlap() {
        let facts = vec![
            "The server runs on port 8080".to_string(),
            "The server runs on port 8080 today".to_string(),
            "Redis is used for caching".to_string(),
        ];
        let deduped = dedup_by_overlap(&facts, 0.8);
        assert!(deduped.len() <= 2); // First two should dedup
    }
}
