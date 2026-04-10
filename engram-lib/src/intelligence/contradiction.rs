//! Contradiction detection -- find memories that contradict each other
//! using SVO (subject-verb-object) triple matching from structured_facts.

use crate::db::Database;
use crate::memory::types::Memory;
use crate::Result;
use serde::Serialize;
use tracing::info;

#[derive(Debug, Clone, Serialize)]
pub struct Contradiction {
    pub memory_a: String,
    pub memory_b: String,
    pub confidence: f32,
    pub description: String,
}

/// Detect contradictions between a new memory and existing facts.
///
/// Extracts SVO triples from the memory's structured_facts and compares
/// against existing facts with the same subject+predicate. If the object
/// differs, flags as a contradiction.
pub async fn detect_contradictions(db: &Database, memory: &Memory) -> Result<Vec<Contradiction>> {
    let conn = db.connection();
    let mut contradictions = Vec::new();

    // Get structured facts for this memory
    let mut fact_rows = conn
        .query(
            "SELECT id, subject, predicate, object, confidence \
             FROM structured_facts \
             WHERE memory_id = ?1",
            libsql::params![memory.id],
        )
        .await?;

    let mut new_facts: Vec<(i64, String, String, String, f64)> = Vec::new();
    while let Some(row) = fact_rows.next().await? {
        new_facts.push((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        ));
    }

    // For each new fact, check against existing facts with same subject+predicate
    for (new_fact_id, subject, predicate, new_object, _confidence) in &new_facts {
        let mut existing_rows = conn
            .query(
                "SELECT sf.id, sf.object, sf.memory_id, sf.confidence \
                 FROM structured_facts sf \
                 WHERE sf.subject = ?1 AND sf.predicate = ?2 \
                   AND sf.memory_id != ?3 \
                   AND sf.id != ?4 \
                 ORDER BY sf.confidence DESC",
                libsql::params![subject.clone(), predicate.clone(), memory.id, *new_fact_id],
            )
            .await?;

        while let Some(row) = existing_rows.next().await? {
            let _old_fact_id: i64 = row.get(0)?;
            let old_object: String = row.get(1)?;
            let old_memory_id: i64 = row.get(2)?;
            let old_confidence: f64 = row.get(3)?;

            // Compare objects -- if they differ, it's a contradiction
            if !objects_match(new_object, &old_object) {
                let conf = old_confidence as f32 * 0.8; // Scale by old fact confidence

                contradictions.push(Contradiction {
                    memory_a: memory.id.to_string(),
                    memory_b: old_memory_id.to_string(),
                    confidence: conf.min(1.0),
                    description: format!(
                        "Conflicting {}: '{}' vs '{}' (subject: {}, predicate: {})",
                        predicate, new_object, old_object, subject, predicate
                    ),
                });
            }
        }
    }

    if !contradictions.is_empty() {
        info!(
            memory_id = memory.id,
            contradictions = contradictions.len(),
            "contradictions_detected"
        );

        // Record contradiction links in memory_links
        for c in &contradictions {
            let mem_b_id: i64 = c.memory_b.parse().unwrap_or(0);
            if mem_b_id > 0 {
                let _ = conn
                    .execute(
                        "INSERT OR IGNORE INTO memory_links (source_id, target_id, similarity, type) \
                         VALUES (?1, ?2, ?3, 'contradicts')",
                        libsql::params![memory.id, mem_b_id, c.confidence as f64],
                    )
                    .await;
            }
        }
    }

    Ok(contradictions)
}

/// Scan all memories for internal contradictions.
///
/// Compares all structured_facts with the same subject+predicate to find
/// conflicting objects. Returns all detected contradictions.
pub async fn scan_all_contradictions(db: &Database, user_id: i64) -> Result<Vec<Contradiction>> {
    let conn = db.connection();
    let mut contradictions = Vec::new();

    // Find all facts where subject+predicate match but object differs
    // Scoped by user_id to use idx_facts_user_subject_predicate index
    let mut rows = conn
        .query(
            "SELECT sf1.memory_id, sf2.memory_id, \
                    sf1.subject, sf1.predicate, sf1.object, sf2.object, \
                    sf1.confidence, sf2.confidence \
             FROM structured_facts sf1 \
             JOIN structured_facts sf2 ON sf1.user_id = sf2.user_id \
               AND sf1.subject = sf2.subject \
               AND sf1.predicate = sf2.predicate \
               AND sf1.id < sf2.id \
               AND sf1.memory_id != sf2.memory_id \
             WHERE sf1.user_id = ?1 \
             LIMIT 500",
            [user_id],
        )
        .await?;

    while let Some(row) = rows.next().await? {
        let mem_a_id: i64 = row.get(0)?;
        let mem_b_id: i64 = row.get(1)?;
        let subject: String = row.get(2)?;
        let predicate: String = row.get(3)?;
        let object_a: String = row.get(4)?;
        let object_b: String = row.get(5)?;
        let conf_a: f64 = row.get(6)?;
        let conf_b: f64 = row.get(7)?;

        if !objects_match(&object_a, &object_b) {
            let conf = (conf_a.min(conf_b) * 0.8) as f32;

            contradictions.push(Contradiction {
                memory_a: mem_a_id.to_string(),
                memory_b: mem_b_id.to_string(),
                confidence: conf.min(1.0),
                description: format!(
                    "Conflicting {}: '{}' vs '{}' (subject: {}, predicate: {})",
                    predicate, object_a, object_b, subject, predicate
                ),
            });
        }
    }

    info!(
        contradictions = contradictions.len(),
        "scan_all_contradictions_complete"
    );

    Ok(contradictions)
}

/// Compare two object strings for equivalence.
/// Handles case-insensitive comparison and minor whitespace differences.
fn objects_match(a: &str, b: &str) -> bool {
    let a_norm = a.trim().to_lowercase();
    let b_norm = b.trim().to_lowercase();
    a_norm == b_norm
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_objects_match_identical() {
        assert!(objects_match("hello", "hello"));
    }

    #[test]
    fn test_objects_match_case_insensitive() {
        assert!(objects_match("Hello World", "hello world"));
    }

    #[test]
    fn test_objects_match_whitespace() {
        assert!(objects_match("  hello  ", "hello"));
    }

    #[test]
    fn test_objects_mismatch() {
        assert!(!objects_match("blue", "red"));
    }

    #[test]
    fn test_contradiction_description_format() {
        let c = Contradiction {
            memory_a: "1".to_string(),
            memory_b: "2".to_string(),
            confidence: 0.8,
            description: format!(
                "Conflicting {}: '{}' vs '{}' (subject: {}, predicate: {})",
                "prefers", "coffee", "tea", "user", "prefers"
            ),
        };
        assert!(c.description.contains("coffee"));
        assert!(c.description.contains("tea"));
    }
}
