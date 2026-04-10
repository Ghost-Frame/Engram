use crate::db::Database;
use crate::json_io::Output;
use crate::tools::{ToolError, ToolResult};
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct LogHypothesisInput {
    pub bug_description: Option<String>,
    pub hypothesis: Option<String>,
    pub confidence: Option<f64>,
}

pub fn log_hypothesis(db: &Database, input: LogHypothesisInput) -> ToolResult {
    let bug_description = input
        .bug_description
        .ok_or_else(|| ToolError::MissingField("bug_description".into()))?;

    let hypothesis = input
        .hypothesis
        .ok_or_else(|| ToolError::MissingField("hypothesis".into()))?;

    let confidence = input.confidence.unwrap_or(0.7);
    if !(0.0..=1.0).contains(&confidence) {
        return Err(ToolError::InvalidValue(
            "confidence must be between 0.0 and 1.0".into(),
        ));
    }

    let id = format!("hyp_{}", &Uuid::new_v4().to_string()[..8]);
    let now = Utc::now().timestamp();

    db.conn()
        .execute(
            r#"
            INSERT INTO hypotheses (id, created_at, bug_description, hypothesis, confidence)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            rusqlite::params![id, now, bug_description, hypothesis, confidence],
        )
        .map_err(|e| ToolError::DatabaseError(e.to_string()))?;

    Ok(Output::ok_with_id(id, "Hypothesis logged"))
}

#[derive(Deserialize)]
pub struct LogOutcomeInput {
    pub hypothesis_id: Option<String>,
    pub outcome: Option<String>,
    pub notes: Option<String>,
}

pub fn log_outcome(db: &Database, input: LogOutcomeInput) -> ToolResult {
    let hypothesis_id = input
        .hypothesis_id
        .ok_or_else(|| ToolError::MissingField("hypothesis_id".into()))?;

    let outcome = input
        .outcome
        .ok_or_else(|| ToolError::MissingField("outcome".into()))?;

    if !["correct", "incorrect", "partial"].contains(&outcome.as_str()) {
        return Err(ToolError::InvalidValue(
            "outcome must be: correct, incorrect, or partial".into(),
        ));
    }

    let now = Utc::now().timestamp();

    let rows = db
        .conn()
        .execute(
            r#"
            UPDATE hypotheses SET outcome = ?1, outcome_notes = ?2, verified_at = ?3
            WHERE id = ?4
            "#,
            rusqlite::params![outcome, input.notes, now, hypothesis_id],
        )
        .map_err(|e| ToolError::DatabaseError(e.to_string()))?;

    if rows == 0 {
        return Err(ToolError::InvalidValue(format!(
            "Hypothesis not found: {}",
            hypothesis_id
        )));
    }

    Ok(Output::ok("Outcome recorded"))
}

#[derive(Deserialize)]
pub struct RecallErrorsInput {
    pub query: Option<String>,
    pub limit: Option<usize>,
}

pub fn recall_errors(db: &Database, input: RecallErrorsInput) -> ToolResult {
    let query = input.query.unwrap_or_default();
    let limit = input.limit.unwrap_or(10);

    let mut stmt = db
        .conn()
        .prepare(
            r#"
            SELECT id, bug_description, hypothesis, outcome, outcome_notes
            FROM hypotheses
            WHERE bug_description LIKE ?1 OR hypothesis LIKE ?1
            ORDER BY created_at DESC
            LIMIT ?2
            "#,
        )
        .map_err(|e| ToolError::DatabaseError(e.to_string()))?;

    let pattern = format!("%{}%", query);
    let rows = stmt
        .query_map(rusqlite::params![pattern, limit], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "bug_description": row.get::<_, String>(1)?,
                "hypothesis": row.get::<_, String>(2)?,
                "outcome": row.get::<_, Option<String>>(3)?,
                "notes": row.get::<_, Option<String>>(4)?,
            }))
        })
        .map_err(|e| ToolError::DatabaseError(e.to_string()))?;

    let results: Vec<_> = rows.filter_map(|r| r.ok()).collect();

    let mut output = Output::ok(format!("Found {} past hypotheses", results.len()));
    output.data = Some(serde_json::json!({ "results": results }));
    Ok(output)
}
