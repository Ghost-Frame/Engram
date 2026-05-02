use super::types::ExecutionAnalysis;
use crate::db::Database;
use crate::{EngError, Result};
use rusqlite::params;

// -- Levenshtein edit distance --

pub fn edit_distance(a: &str, b: &str) -> usize {
    let a_len = a.len();
    let b_len = b.len();
    let mut dp = vec![vec![0usize; b_len + 1]; a_len + 1];
    for (i, row) in dp.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, val) in dp[0].iter_mut().enumerate() {
        *val = j;
    }
    for i in 1..=a_len {
        for j in 1..=b_len {
            let cost = if a.as_bytes()[i - 1] == b.as_bytes()[j - 1] {
                0
            } else {
                1
            };
            dp[i][j] = (dp[i - 1][j] + 1)
                .min(dp[i][j - 1] + 1)
                .min(dp[i - 1][j - 1] + cost);
        }
    }
    dp[a_len][b_len]
}

/// Correct a potentially misspelled skill ID against known skill names.
/// Returns the best match name if edit distance <= 3, or prefix match.
#[tracing::instrument(skip(db), fields(name = %name, user_id))]
pub async fn correct_skill_id(db: &Database, name: &str, _user_id: i64) -> Result<Option<String>> {
    let name = name.to_string();
    db.read(move |conn| {
        let mut stmt = conn
            .prepare("SELECT name FROM skill_records WHERE is_active = 1")
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        let names: Vec<String> = stmt
            .query_map(params![], |row| row.get(0))
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        // Exact match
        if names.iter().any(|n| n == &name) {
            return Ok(Some(name.clone()));
        }

        // Edit distance match (threshold <= 3)
        let mut best: Option<(String, usize)> = None;
        for n in &names {
            let dist = edit_distance(&name, n);
            if dist <= 3 && (best.is_none() || dist < best.as_ref().unwrap().1) {
                best = Some((n.clone(), dist));
            }
        }
        if let Some((matched, _)) = best {
            return Ok(Some(matched));
        }

        // Prefix match
        let lower = name.to_lowercase();
        for n in &names {
            if n.to_lowercase().starts_with(&lower) {
                return Ok(Some(n.clone()));
            }
        }

        Ok(None)
    })
    .await
}

pub const ANALYSIS_SYSTEM_PROMPT: &str = "You are a skill execution analyzer. Given a task, the skill that was applied, and the execution result, analyze whether the skill was helpful and provide structured feedback. Return JSON with fields: skill_applied (bool), skill_helpful (bool), tool_calls (string[]), error_category (string|null), improvement_notes (string|null).";

/// Persist an execution analysis to the database.
/// Inserts into execution_analyses, creates skill_judgments entries, and updates counters.
#[tracing::instrument(skip(db, analysis), fields(skill_id, duration_ms = ?duration_ms, agent = %agent))]
pub async fn persist_analysis(
    db: &Database,
    skill_id: i64,
    analysis: &ExecutionAnalysis,
    duration_ms: Option<f64>,
    agent: &str,
) -> Result<()> {
    let success = analysis.skill_applied && analysis.skill_helpful;
    let error_type = analysis.error_category.clone();
    let notes = analysis.improvement_notes.clone();
    let agent = agent.to_string();
    let score = if analysis.skill_helpful { 1.0 } else { 0.0 };
    let rationale = analysis.improvement_notes.clone().unwrap_or_default();

    db.write(move |conn| {
        // Insert execution analysis
        conn.execute(
            "INSERT INTO execution_analyses (skill_id, success, duration_ms, error_type, error_message) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![skill_id, success as i32, duration_ms, error_type, notes],
        ).map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        // Insert judgment
        conn.execute(
            "INSERT INTO skill_judgments (skill_id, judge_agent, score, rationale) VALUES (?1, ?2, ?3, ?4)",
            params![skill_id, agent, score, rationale],
        ).map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        // Update counters on skill_records
        if success {
            conn.execute(
                "UPDATE skill_records SET success_count = success_count + 1, execution_count = execution_count + 1, updated_at = datetime('now') WHERE id = ?1",
                params![skill_id],
            ).map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        } else {
            conn.execute(
                "UPDATE skill_records SET failure_count = failure_count + 1, execution_count = execution_count + 1, updated_at = datetime('now') WHERE id = ?1",
                params![skill_id],
            ).map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        }

        // Update avg duration
        if let Some(dur) = duration_ms {
            conn.execute(
                "UPDATE skill_records SET avg_duration_ms = COALESCE((avg_duration_ms * (execution_count - 1) + ?1) / execution_count, ?1), updated_at = datetime('now') WHERE id = ?2",
                params![dur, skill_id],
            ).map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        }

        // Update trust_score as average of all judgments
        conn.execute(
            "UPDATE skill_records SET trust_score = (SELECT AVG(score) * 100.0 FROM skill_judgments WHERE skill_id = ?1), updated_at = datetime('now') WHERE id = ?1",
            params![skill_id],
        ).map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        Ok(())
    }).await
}

/// Get usage stats for skills (underused or failing).
#[tracing::instrument(skip(db), fields(user_id))]
pub async fn get_usage_stats(db: &Database, _user_id: i64) -> Result<serde_json::Value> {
    db.read(move |conn| {
        // Underused: active skills with < 5 executions
        let mut stmt = conn.prepare(
            "SELECT id, name, execution_count, trust_score FROM skill_records WHERE is_active = 1 AND execution_count < 5 ORDER BY execution_count ASC LIMIT 20"
        ).map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        let underused: Vec<serde_json::Value> = stmt.query_map(params![], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, i64>(0)?,
                "name": row.get::<_, String>(1)?,
                "execution_count": row.get::<_, i32>(2)?,
                "trust_score": row.get::<_, f64>(3)?,
            }))
        })
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
        .filter_map(|r| r.ok())
        .collect();

        // Failing: active skills with success_rate < 50%
        let mut stmt = conn.prepare(
            "SELECT id, name, success_count, failure_count, trust_score FROM skill_records WHERE is_active = 1 AND execution_count > 0 AND CAST(success_count AS REAL) / execution_count < 0.5 ORDER BY trust_score ASC LIMIT 20"
        ).map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        let failing: Vec<serde_json::Value> = stmt.query_map(params![], |row| {
            let sc: i32 = row.get(2)?;
            let fc: i32 = row.get(3)?;
            let total = sc + fc;
            let rate = if total > 0 { sc as f64 / total as f64 } else { 0.0 };
            Ok(serde_json::json!({
                "id": row.get::<_, i64>(0)?,
                "name": row.get::<_, String>(1)?,
                "success_count": sc,
                "failure_count": fc,
                "success_rate": rate,
                "trust_score": row.get::<_, f64>(4)?,
            }))
        })
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
        .filter_map(|r| r.ok())
        .collect();

        Ok(serde_json::json!({
            "underused": underused,
            "failing": failing,
        }))
    }).await
}

/// Skills whose success rate has fallen below `max_success_rate`, eligible
/// for auto-fix. Excludes any parent whose most recent fixed child (a row in
/// `skill_records` with `parent_skill_id = sr.id`) was created within
/// `cooldown_secs`. Uses `skill_records.created_at` as the cooldown anchor
/// because `skill_tags` has no timestamp column.
#[tracing::instrument(
    skip(db),
    fields(user_id, min_executions, max_success_rate, cooldown_secs, limit)
)]
pub async fn get_failing_skill_candidates(
    db: &Database,
    _user_id: i64,
    min_executions: u32,
    max_success_rate: f32,
    cooldown_secs: u64,
    limit: usize,
) -> Result<Vec<i64>> {
    let cooldown_clause = format!("-{} seconds", cooldown_secs as i64);
    db.read(move |conn| {
        let sql = "SELECT sr.id FROM skill_records sr \
                   WHERE sr.is_active = 1 \
                     AND sr.is_deprecated = 0 \
                     AND sr.execution_count >= ?1 \
                     AND CAST(sr.success_count AS REAL) / sr.execution_count < ?2 \
                     AND NOT EXISTS ( \
                         SELECT 1 FROM skill_records child \
                         WHERE child.parent_skill_id = sr.id \
                           AND child.created_at > datetime('now', ?3) \
                     ) \
                   ORDER BY (CAST(sr.success_count AS REAL) / sr.execution_count) ASC, \
                            sr.trust_score ASC \
                   LIMIT ?4";
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        let ids: Vec<i64> = stmt
            .query_map(
                params![
                    min_executions as i64,
                    max_success_rate as f64,
                    cooldown_clause,
                    limit as i64,
                ],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(ids)
    })
    .await
}

/// Memories explicitly tagged as skill candidates that have not yet been
/// captured. `capture_tag` is matched against the JSON-encoded
/// `memories.tags` column via LIKE; duplicates by content are collapsed.
/// Memories whose content is already the name or description of an existing
/// active skill (case-insensitive substring) are excluded so we do not spam
/// the LLM re-capturing the same idea.
#[tracing::instrument(skip(db, capture_tag), fields(user_id, since_secs, limit))]
pub async fn get_capture_candidates(
    db: &Database,
    _user_id: i64,
    capture_tag: &str,
    since_secs: u64,
    limit: usize,
) -> Result<Vec<String>> {
    let tag_needle = format!("%\"{}\"%", capture_tag.replace('"', ""));
    let since_clause = format!("-{} seconds", since_secs as i64);
    db.read(move |conn| {
        let sql = "SELECT DISTINCT m.content FROM memories m \
                   WHERE m.is_forgotten = 0 \
                     AND m.is_archived = 0 \
                     AND m.is_latest = 1 \
                     AND m.tags IS NOT NULL \
                     AND m.tags LIKE ?1 \
                     AND m.created_at > datetime('now', ?2) \
                     AND NOT EXISTS ( \
                         SELECT 1 FROM skill_records sr \
                         WHERE sr.is_active = 1 \
                           AND ( \
                               LOWER(m.content) LIKE '%' || LOWER(sr.name) || '%' \
                               OR LOWER(m.content) LIKE '%' || LOWER(COALESCE(sr.description, '')) || '%' \
                           ) \
                     ) \
                   ORDER BY m.created_at DESC \
                   LIMIT ?3";
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        let rows: Vec<String> = stmt
            .query_map(
                params![tag_needle, since_clause, limit as i64],
                |row| row.get::<_, String>(0),
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    })
    .await
}

/// Pairs of active skills whose tag sets overlap at least `similarity`
/// (Jaccard) and that do not already share a derived child. Each pair is
/// returned as `(vec![a_id, b_id], direction_hint)` where the direction
/// hint is a short natural-language phrase synthesised from the pair.
#[tracing::instrument(skip(db), fields(user_id, similarity, limit))]
pub async fn get_derive_candidates(
    db: &Database,
    _user_id: i64,
    similarity: f32,
    limit: usize,
) -> Result<Vec<(Vec<i64>, String)>> {
    let similarity = similarity.clamp(0.0, 1.0) as f64;
    db.read(move |conn| {
        // Load tag sets for every active skill the user owns. Skills with no
        // tags are excluded; there is nothing for Jaccard to work with.
        let mut stmt = conn
            .prepare(
                "SELECT sr.id, sr.name, st.tag \
                 FROM skill_records sr \
                 INNER JOIN skill_tags st ON st.skill_id = sr.id \
                 WHERE sr.is_active = 1 AND sr.is_deprecated = 0",
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        let rows = stmt
            .query_map(params![], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        let mut tags_by_skill: std::collections::BTreeMap<
            i64,
            (String, std::collections::BTreeSet<String>),
        > = std::collections::BTreeMap::new();
        for r in rows.flatten() {
            let entry = tags_by_skill
                .entry(r.0)
                .or_insert_with(|| (r.1.clone(), std::collections::BTreeSet::new()));
            entry.1.insert(r.2);
        }

        // Pull every (skill_id, parent_id) pair so we can reject pairs whose
        // derived child already exists.
        let mut parents_stmt = conn
            .prepare("SELECT slp.skill_id, slp.parent_id FROM skill_lineage_parents slp")
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        let parent_rows = parents_stmt
            .query_map(params![], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        let mut lineage: std::collections::HashMap<i64, std::collections::BTreeSet<i64>> =
            std::collections::HashMap::new();
        for r in parent_rows.flatten() {
            lineage.entry(r.0).or_default().insert(r.1);
        }

        let ids: Vec<i64> = tags_by_skill.keys().copied().collect();
        let mut scored: Vec<(f64, i64, i64, String, String)> = Vec::new();
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                let a = ids[i];
                let b = ids[j];
                let (name_a, tags_a) = &tags_by_skill[&a];
                let (name_b, tags_b) = &tags_by_skill[&b];
                let inter = tags_a.intersection(tags_b).count();
                let union = tags_a.union(tags_b).count();
                if union == 0 {
                    continue;
                }
                let score = inter as f64 / union as f64;
                if score < similarity {
                    continue;
                }
                let already = lineage
                    .values()
                    .any(|parents| parents.contains(&a) && parents.contains(&b));
                if already {
                    continue;
                }
                scored.push((score, a, b, name_a.clone(), name_b.clone()));
            }
        }
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        let out: Vec<(Vec<i64>, String)> = scored
            .into_iter()
            .take(limit)
            .map(|(_, a, b, na, nb)| {
                let direction = format!(
                    "Combine the strengths of '{}' and '{}' into a single skill, \
                     removing redundancy and preserving both workflows.",
                    na, nb
                );
                (vec![a, b], direction)
            })
            .collect();
        Ok(out)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_edit_distance_identical() {
        assert_eq!(edit_distance("hello", "hello"), 0);
    }

    #[test]
    fn test_edit_distance_one() {
        assert_eq!(edit_distance("hello", "helo"), 1);
    }

    #[test]
    fn test_edit_distance_swap() {
        assert_eq!(edit_distance("abc", "bac"), 2);
    }

    #[test]
    fn test_edit_distance_empty() {
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("abc", ""), 3);
    }

    #[test]
    fn test_analysis_deserialize() {
        let json = r#"{"skill_applied":true,"skill_helpful":false,"tool_calls":["read_file"],"error_category":"timeout","improvement_notes":"needs retry"}"#;
        let a: ExecutionAnalysis = serde_json::from_str(json).unwrap();
        assert!(a.skill_applied);
        assert!(!a.skill_helpful);
        assert_eq!(a.tool_calls.len(), 1);
    }

    async fn memory_db() -> Database {
        Database::connect_memory().await.expect("in-mem db")
    }

    async fn seed_skill(
        db: &Database,
        _user_id: i64,
        name: &str,
        executions: i64,
        successes: i64,
        created_offset_secs: i64,
    ) -> i64 {
        let name = name.to_string();
        db.write(move |conn| {
            conn.execute(
                "INSERT INTO skill_records \
                    (name, agent, description, code, execution_count, success_count, \
                     failure_count, is_active, is_deprecated, created_at) \
                 VALUES (?1, 'test', '', '', ?2, ?3, ?4, 1, 0, \
                         datetime('now', ?5))",
                params![
                    name,
                    executions,
                    successes,
                    executions - successes,
                    format!("-{} seconds", created_offset_secs),
                ],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            Ok(conn.last_insert_rowid())
        })
        .await
        .expect("seed skill")
    }

    async fn seed_skill_tag(db: &Database, skill_id: i64, tag: &str) {
        let tag = tag.to_string();
        db.write(move |conn| {
            conn.execute(
                "INSERT INTO skill_tags (skill_id, tag) VALUES (?1, ?2)",
                params![skill_id, tag],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            Ok(())
        })
        .await
        .expect("seed tag");
    }

    async fn seed_lineage(db: &Database, child_id: i64, parent_id: i64) {
        db.write(move |conn| {
            conn.execute(
                "INSERT INTO skill_lineage_parents (skill_id, parent_id) VALUES (?1, ?2)",
                params![child_id, parent_id],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            Ok(())
        })
        .await
        .expect("seed lineage");
    }

    #[tokio::test]
    async fn failing_candidates_returns_failing_skill() {
        let db = memory_db().await;
        let id = seed_skill(&db, 1, "flaky", 20, 4, 7200).await;
        let ids = get_failing_skill_candidates(&db, 1, 10, 0.5, 3600, 10)
            .await
            .expect("query");
        assert_eq!(ids, vec![id]);
    }

    #[tokio::test]
    async fn failing_candidates_ignores_underused() {
        let db = memory_db().await;
        seed_skill(&db, 1, "rarely-run", 3, 0, 7200).await;
        let ids = get_failing_skill_candidates(&db, 1, 10, 0.5, 3600, 10)
            .await
            .expect("query");
        assert!(ids.is_empty());
    }

    #[tokio::test]
    async fn failing_candidates_respects_cooldown() {
        let db = memory_db().await;
        let parent = seed_skill(&db, 1, "flaky", 20, 4, 7200).await;
        // Recent child -> parent is in cooldown.
        let child_id = {
            db.write(move |conn| {
                conn.execute(
                    "INSERT INTO skill_records \
                        (name, agent, description, code, execution_count, success_count, \
                         failure_count, is_active, is_deprecated, \
                         parent_skill_id, created_at) \
                     VALUES ('flaky-v2', 'test', '', '', 0, 0, 0, 1, 0, ?1, \
                             datetime('now', '-60 seconds'))",
                    params![parent],
                )
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
                Ok(conn.last_insert_rowid())
            })
            .await
            .expect("seed child")
        };
        let _ = child_id;
        let ids = get_failing_skill_candidates(&db, 1, 10, 0.5, 3600, 10)
            .await
            .expect("query");
        assert!(ids.is_empty(), "cooldown should hide parent");
    }

    #[tokio::test]
    async fn capture_candidates_dedupes() {
        let db = memory_db().await;
        db.write(|conn| {
            conn.execute_batch(
                "INSERT INTO memories (content, tags, is_latest) \
                    VALUES ('use ripgrep over grep', '[\"skill_candidate\"]', 1); \
                 INSERT INTO memories (content, tags, is_latest) \
                    VALUES ('use ripgrep over grep', '[\"skill_candidate\"]', 1); \
                 INSERT INTO memories (content, tags, is_latest) \
                    VALUES ('unrelated note', '[\"other\"]', 1);",
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            Ok(())
        })
        .await
        .expect("seed memories");
        let rows = get_capture_candidates(&db, 1, "skill_candidate", 86_400, 10)
            .await
            .expect("query");
        assert_eq!(rows, vec!["use ripgrep over grep".to_string()]);
    }

    #[tokio::test]
    async fn derive_candidates_finds_similar_pair() {
        let db = memory_db().await;
        let a = seed_skill(&db, 1, "shell-cheatsheet", 5, 5, 600).await;
        let b = seed_skill(&db, 1, "shell-oneliners", 5, 5, 600).await;
        for tag in ["shell", "bash", "cli"] {
            seed_skill_tag(&db, a, tag).await;
            seed_skill_tag(&db, b, tag).await;
        }
        let pairs = get_derive_candidates(&db, 1, 0.5, 5).await.expect("query");
        assert_eq!(pairs.len(), 1, "should find one pair");
        assert_eq!(pairs[0].0, vec![a, b]);
        assert!(pairs[0].1.contains("shell-cheatsheet"));
    }

    #[tokio::test]
    async fn derive_candidates_skip_already_derived() {
        let db = memory_db().await;
        let a = seed_skill(&db, 1, "alpha", 5, 5, 600).await;
        let b = seed_skill(&db, 1, "beta", 5, 5, 600).await;
        let child = seed_skill(&db, 1, "alpha-beta", 1, 1, 300).await;
        for tag in ["x", "y", "z"] {
            seed_skill_tag(&db, a, tag).await;
            seed_skill_tag(&db, b, tag).await;
        }
        seed_lineage(&db, child, a).await;
        seed_lineage(&db, child, b).await;
        let pairs = get_derive_candidates(&db, 1, 0.5, 5).await.expect("query");
        assert!(pairs.is_empty(), "existing derivation should suppress pair");
    }
}
