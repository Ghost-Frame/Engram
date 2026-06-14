//! Hypothesis lifecycle: log, close, and recall past hypotheses.
//!
//! Hypotheses are the structured pre-fix record an agent creates before
//! touching code in response to a bug. Outcome tracking allows pattern
//! mining over past errors via `recall_errors`.

use crate::db::Database;
use crate::EngError;
use chrono::Utc;
use rusqlite::params;
use serde_json::Value;
use uuid::Uuid;

/// Persist a new hypothesis row to `forge_hypotheses` and return its ID.
///
/// `confidence` must be in [0.0, 1.0]. `spec_id` is optional; if supplied it
/// must be the ID of an existing `forge_specs` row (FK enforced by schema).
/// `session_id` is stored for gate enforcement queries.
pub async fn log_hypothesis(
    db: &Database,
    user_id: i64,
    session_id: Option<&str>,
    bug_description: String,
    hypothesis: String,
    confidence: Option<f64>,
    spec_id: Option<String>,
) -> crate::Result<Value> {
    let confidence = confidence.unwrap_or(0.7);
    if !(0.0..=1.0).contains(&confidence) {
        return Err(EngError::InvalidInput(
            "confidence must be between 0.0 and 1.0".into(),
        ));
    }

    let id = format!("hyp_{}", &Uuid::new_v4().to_string()[..8]);
    let now = Utc::now().timestamp();
    let session_id = session_id.map(|s| s.to_string());
    let id_clone = id.clone();

    db.write(move |conn| {
        conn.execute(
            "INSERT INTO forge_hypotheses
             (id, user_id, session_id, created_at, bug_description, hypothesis,
              confidence, spec_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id_clone,
                user_id,
                session_id,
                now,
                bug_description,
                hypothesis,
                confidence,
                spec_id,
            ],
        )?;
        Ok(())
    })
    .await?;

    Ok(serde_json::json!({ "id": id, "message": "Hypothesis logged" }))
}

/// Close an open hypothesis by recording its outcome.
///
/// Valid outcomes: `correct`, `incorrect`, `partial`.
/// Returns `NotFound` if `hypothesis_id` does not exist for `user_id`.
pub async fn log_outcome(
    db: &Database,
    user_id: i64,
    hypothesis_id: String,
    outcome: String,
    notes: Option<String>,
) -> crate::Result<Value> {
    if !["correct", "incorrect", "partial"].contains(&outcome.as_str()) {
        return Err(EngError::InvalidInput(
            "outcome must be: correct, incorrect, or partial".into(),
        ));
    }

    let now = Utc::now().timestamp();
    let hypothesis_id_for_err = hypothesis_id.clone();

    let rows = db
        .write(move |conn| {
            let n = conn.execute(
                "UPDATE forge_hypotheses
                 SET outcome = ?1, outcome_notes = ?2, verified_at = ?3
                 WHERE id = ?4 AND user_id = ?5",
                params![outcome, notes, now, hypothesis_id, user_id],
            )?;
            Ok(n)
        })
        .await?;

    if rows == 0 {
        return Err(EngError::NotFound(format!(
            "Hypothesis not found: {hypothesis_id_for_err}"
        )));
    }

    Ok(serde_json::json!({ "message": "Outcome recorded" }))
}

/// Search past hypotheses by keyword, returning recent matches.
///
/// Searches both `bug_description` and `hypothesis` columns via LIKE.
/// Scoped to `user_id` for tenant isolation. Defaults to 10 results.
pub async fn recall_errors(
    db: &Database,
    user_id: i64,
    query: Option<String>,
    limit: Option<usize>,
) -> crate::Result<Value> {
    let query = query.unwrap_or_default();
    let limit = limit.unwrap_or(10) as i64;
    let pattern = format!("%{query}%");

    let results: Vec<Value> = db
        .read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, bug_description, hypothesis, outcome, outcome_notes
                 FROM forge_hypotheses
                 WHERE user_id = ?1
                   AND (bug_description LIKE ?2 OR hypothesis LIKE ?2)
                 ORDER BY created_at DESC
                 LIMIT ?3",
            )?;
            let rows: Vec<Value> = stmt
                .query_map(params![user_id, pattern, limit], |row| {
                    Ok(serde_json::json!({
                        "id": row.get::<_, String>(0)?,
                        "bug_description": row.get::<_, String>(1)?,
                        "hypothesis": row.get::<_, String>(2)?,
                        "outcome": row.get::<_, Option<String>>(3)?,
                        "notes": row.get::<_, Option<String>>(4)?,
                    }))
                })?
                .filter_map(|r| r.ok())
                .collect();
            Ok(rows)
        })
        .await?;

    Ok(serde_json::json!({ "results": results, "count": results.len() }))
}
