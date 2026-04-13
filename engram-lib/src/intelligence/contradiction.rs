//! Contradiction detection -- find memories that contradict each other
//! using SVO (subject-verb-object) triple matching from structured_facts.

use crate::db::Database;
use crate::memory::types::Memory;
use crate::{EngError, Result};
use rusqlite::params;
use serde::Serialize;
use tracing::info;

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

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
    let memory_id = memory.id;
    let user_id = memory.user_id;

    // Get structured facts for this memory (tenant-scoped)
    let new_facts: Vec<(i64, String, String, String, f64)> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, subject, predicate, object, confidence \
                     FROM structured_facts \
                     WHERE memory_id = ?1 AND user_id = ?2",
                )
                .map_err(rusqlite_to_eng_error)?;
            let rows = stmt
                .query_map(params![memory_id, user_id], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, f64>(4)?,
                    ))
                })
                .map_err(rusqlite_to_eng_error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(rusqlite_to_eng_error)?;
            Ok(rows)
        })
        .await?;

    let mut contradictions = Vec::new();

    // For each new fact, check against existing facts with same subject+predicate
    // SECURITY: scoped to memory.user_id to prevent cross-tenant fact leakage.
    for (new_fact_id, subject, predicate, new_object, _confidence) in &new_facts {
        let subject_c = subject.clone();
        let predicate_c = predicate.clone();
        let nfid = *new_fact_id;

        let existing: Vec<(i64, String, i64, f64)> = db
            .read(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT sf.id, sf.object, sf.memory_id, sf.confidence \
                         FROM structured_facts sf \
                         WHERE sf.user_id = ?1 \
                           AND sf.subject = ?2 AND sf.predicate = ?3 \
                           AND sf.memory_id != ?4 \
                           AND sf.id != ?5 \
                         ORDER BY sf.confidence DESC",
                    )
                    .map_err(rusqlite_to_eng_error)?;
                let rows = stmt
                    .query_map(
                        params![user_id, subject_c, predicate_c, memory_id, nfid],
                        |row| {
                            Ok((
                                row.get::<_, i64>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, i64>(2)?,
                                row.get::<_, f64>(3)?,
                            ))
                        },
                    )
                    .map_err(rusqlite_to_eng_error)?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(rusqlite_to_eng_error)?;
                Ok(rows)
            })
            .await?;

        for (_old_fact_id, old_object, old_memory_id, old_confidence) in existing {
            // Compare objects -- if they differ, it's a contradiction
            if !objects_match(new_object, &old_object) {
                let conf = old_confidence as f32 * 0.8; // Scale by old fact confidence

                contradictions.push(Contradiction {
                    memory_a: memory_id.to_string(),
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
            memory_id = memory_id,
            user_id = user_id,
            contradictions = contradictions.len(),
            "contradictions_detected"
        );

        // Record contradiction links in memory_links
        for c in &contradictions {
            let mem_b_id: i64 = c.memory_b.parse().unwrap_or(0);
            if mem_b_id > 0 {
                let conf_f64 = c.confidence as f64;
                let _ = db
                    .write(move |conn| {
                        conn.execute(
                            "INSERT OR IGNORE INTO memory_links \
                             (source_id, target_id, similarity, type) \
                             VALUES (?1, ?2, ?3, 'contradicts')",
                            params![memory_id, mem_b_id, conf_f64],
                        )
                        .map_err(rusqlite_to_eng_error)?;
                        Ok(())
                    })
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
#[allow(clippy::type_complexity)]
pub async fn scan_all_contradictions(db: &Database, user_id: i64) -> Result<Vec<Contradiction>> {
    let rows: Vec<(i64, i64, String, String, String, String, f64, f64)> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
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
                )
                .map_err(rusqlite_to_eng_error)?;
            let rows = stmt
                .query_map(params![user_id], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, f64>(6)?,
                        row.get::<_, f64>(7)?,
                    ))
                })
                .map_err(rusqlite_to_eng_error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(rusqlite_to_eng_error)?;
            Ok(rows)
        })
        .await?;

    let mut contradictions = Vec::new();

    for (mem_a_id, mem_b_id, subject, predicate, object_a, object_b, conf_a, conf_b) in rows {
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
