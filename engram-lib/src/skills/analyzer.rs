use crate::db::Database;
use crate::Result;
use libsql::params;
use serde::{Deserialize, Serialize};

// -- Levenshtein edit distance --

pub fn edit_distance(a: &str, b: &str) -> usize {
    let a_len = a.len();
    let b_len = b.len();
    let mut dp = vec![vec![0usize; b_len + 1]; a_len + 1];
    for i in 0..=a_len { dp[i][0] = i; }
    for j in 0..=b_len { dp[0][j] = j; }
    for i in 1..=a_len {
        for j in 1..=b_len {
            let cost = if a.as_bytes()[i - 1] == b.as_bytes()[j - 1] { 0 } else { 1 };
            dp[i][j] = (dp[i - 1][j] + 1)
                .min(dp[i][j - 1] + 1)
                .min(dp[i - 1][j - 1] + cost);
        }
    }
    dp[a_len][b_len]
}

/// Correct a potentially misspelled skill ID against known skill names.
/// Returns the best match name if edit distance <= 3, or prefix match.
pub async fn correct_skill_id(db: &Database, name: &str, user_id: i64) -> Result<Option<String>> {
    let conn = db.connection();
    let mut rows = conn.query(
        "SELECT name FROM skill_records WHERE user_id = ?1 AND is_active = 1",
        params![user_id],
    ).await?;

    let mut names = Vec::new();
    while let Some(row) = rows.next().await? {
        names.push(row.get::<String>(0)?);
    }

    // Exact match
    if names.iter().any(|n| n == name) {
        return Ok(Some(name.to_string()));
    }

    // Edit distance match (threshold <= 3)
    let mut best: Option<(String, usize)> = None;
    for n in &names {
        let dist = edit_distance(name, n);
        if dist <= 3
            && (best.is_none() || dist < best.as_ref().unwrap().1) {
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
}

// -- Analysis types --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionAnalysis {
    pub skill_applied: bool,
    pub skill_helpful: bool,
    pub tool_calls: Vec<String>,
    pub error_category: Option<String>,
    pub improvement_notes: Option<String>,
}

pub const ANALYSIS_SYSTEM_PROMPT: &str = "You are a skill execution analyzer. Given a task, the skill that was applied, and the execution result, analyze whether the skill was helpful and provide structured feedback. Return JSON with fields: skill_applied (bool), skill_helpful (bool), tool_calls (string[]), error_category (string|null), improvement_notes (string|null).";

/// Persist an execution analysis to the database.
/// Inserts into execution_analyses, creates skill_judgments entries, and updates counters.
pub async fn persist_analysis(
    db: &Database,
    skill_id: i64,
    analysis: &ExecutionAnalysis,
    duration_ms: Option<f64>,
    agent: &str,
) -> Result<()> {
    let conn = db.connection();

    // Insert execution analysis
    let success = analysis.skill_applied && analysis.skill_helpful;
    let error_type = analysis.error_category.clone();
    let notes = analysis.improvement_notes.clone();

    conn.execute(
        "INSERT INTO execution_analyses (skill_id, success, duration_ms, error_type, error_message) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![skill_id, success as i32, duration_ms, error_type, notes],
    ).await?;

    // Insert judgment
    let score = if analysis.skill_helpful { 1.0 } else { 0.0 };
    let rationale = analysis.improvement_notes.clone().unwrap_or_default();
    conn.execute(
        "INSERT INTO skill_judgments (skill_id, judge_agent, score, rationale) VALUES (?1, ?2, ?3, ?4)",
        params![skill_id, agent.to_string(), score, rationale],
    ).await?;

    // Update counters on skill_records
    if success {
        conn.execute(
            "UPDATE skill_records SET success_count = success_count + 1, execution_count = execution_count + 1, updated_at = datetime('now') WHERE id = ?1",
            params![skill_id],
        ).await?;
    } else {
        conn.execute(
            "UPDATE skill_records SET failure_count = failure_count + 1, execution_count = execution_count + 1, updated_at = datetime('now') WHERE id = ?1",
            params![skill_id],
        ).await?;
    }

    // Update avg duration
    if let Some(dur) = duration_ms {
        conn.execute(
            "UPDATE skill_records SET avg_duration_ms = COALESCE((avg_duration_ms * (execution_count - 1) + ?1) / execution_count, ?1), updated_at = datetime('now') WHERE id = ?2",
            params![dur, skill_id],
        ).await?;
    }

    // Update trust_score as average of all judgments
    conn.execute(
        "UPDATE skill_records SET trust_score = (SELECT AVG(score) * 100.0 FROM skill_judgments WHERE skill_id = ?1), updated_at = datetime('now') WHERE id = ?1",
        params![skill_id],
    ).await?;

    Ok(())
}

/// Get usage stats for skills (underused or failing).
pub async fn get_usage_stats(db: &Database, user_id: i64) -> Result<serde_json::Value> {
    let conn = db.connection();

    // Underused: active skills with < 5 executions
    let mut underused_rows = conn.query(
        "SELECT id, name, execution_count, trust_score FROM skill_records WHERE user_id = ?1 AND is_active = 1 AND execution_count < 5 ORDER BY execution_count ASC LIMIT 20",
        params![user_id],
    ).await?;
    let mut underused = Vec::new();
    while let Some(row) = underused_rows.next().await? {
        underused.push(serde_json::json!({
            "id": row.get::<i64>(0)?,
            "name": row.get::<String>(1)?,
            "execution_count": row.get::<i32>(2)?,
            "trust_score": row.get::<f64>(3)?,
        }));
    }

    // Failing: active skills with success_rate < 50%
    let mut failing_rows = conn.query(
        "SELECT id, name, success_count, failure_count, trust_score FROM skill_records WHERE user_id = ?1 AND is_active = 1 AND execution_count > 0 AND CAST(success_count AS REAL) / execution_count < 0.5 ORDER BY trust_score ASC LIMIT 20",
        params![user_id],
    ).await?;
    let mut failing = Vec::new();
    while let Some(row) = failing_rows.next().await? {
        let sc: i32 = row.get(2)?;
        let fc: i32 = row.get(3)?;
        let total = sc + fc;
        let rate = if total > 0 { sc as f64 / total as f64 } else { 0.0 };
        failing.push(serde_json::json!({
            "id": row.get::<i64>(0)?,
            "name": row.get::<String>(1)?,
            "success_count": sc,
            "failure_count": fc,
            "success_rate": rate,
            "trust_score": row.get::<f64>(4)?,
        }));
    }

    Ok(serde_json::json!({
        "underused": underused,
        "failing": failing,
    }))
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
