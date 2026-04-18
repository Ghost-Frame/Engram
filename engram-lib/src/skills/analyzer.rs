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
pub async fn correct_skill_id(db: &Database, name: &str, user_id: i64) -> Result<Option<String>> {
    let name = name.to_string();
    db.read(move |conn| {
        let mut stmt = conn
            .prepare("SELECT name FROM skill_records WHERE user_id = ?1 AND is_active = 1")
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        let names: Vec<String> = stmt
            .query_map(params![user_id], |row| row.get(0))
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
pub async fn get_usage_stats(db: &Database, user_id: i64) -> Result<serde_json::Value> {
    db.read(move |conn| {
        // Underused: active skills with < 5 executions
        let mut stmt = conn.prepare(
            "SELECT id, name, execution_count, trust_score FROM skill_records WHERE user_id = ?1 AND is_active = 1 AND execution_count < 5 ORDER BY execution_count ASC LIMIT 20"
        ).map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        let underused: Vec<serde_json::Value> = stmt.query_map(params![user_id], |row| {
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
            "SELECT id, name, success_count, failure_count, trust_score FROM skill_records WHERE user_id = ?1 AND is_active = 1 AND execution_count > 0 AND CAST(success_count AS REAL) / execution_count < 0.5 ORDER BY trust_score ASC LIMIT 20"
        ).map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        let failing: Vec<serde_json::Value> = stmt.query_map(params![user_id], |row| {
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
}
