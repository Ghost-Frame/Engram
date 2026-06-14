//! `consider_approaches` -- records two or more named design alternatives and
//! emits a structured comparison prompt for agent reasoning.
//!
//! Approaches are persisted to `forge_approaches` with optional spec linkage.
//! A `chosen_index` marks the ultimately selected option so future recall shows
//! which trade-off was made.

use crate::db::Database;
use crate::EngError;
use chrono::Utc;
use rusqlite::params;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

/// One design alternative: a short name, prose description, pros, cons, and
/// an optional numeric score (higher is better).
#[derive(Deserialize, Clone)]
pub struct ApproachItem {
    /// Short identifier for this alternative.
    pub name: String,
    /// Full prose description of the approach.
    pub description: String,
    /// Advantages of this approach.
    #[serde(default)]
    pub pros: Vec<String>,
    /// Disadvantages of this approach.
    #[serde(default)]
    pub cons: Vec<String>,
    /// Optional numeric desirability score (higher is better).
    pub score: Option<f64>,
}

/// Validate inputs, persist all approaches to `forge_approaches` (marking the
/// chosen one if `chosen_index` is supplied), and return a structured
/// comparison prompt suitable for agent reasoning.
///
/// Requires at least 2 approaches. If `spec_id` is supplied the referenced
/// spec must exist in `forge_specs` for `user_id`.
pub async fn consider_approaches(
    db: &Database,
    user_id: i64,
    spec_id: Option<String>,
    problem: String,
    approaches: Vec<ApproachItem>,
    chosen_index: Option<usize>,
) -> crate::Result<Value> {
    if approaches.len() < 2 {
        return Err(EngError::InvalidInput(
            "At least 2 approaches required for comparison".into(),
        ));
    }
    if let Some(idx) = chosen_index {
        if idx >= approaches.len() {
            return Err(EngError::InvalidInput(format!(
                "chosen_index {idx} out of range (have {} approaches)",
                approaches.len()
            )));
        }
    }

    // Verify spec_id exists for this user if one was provided.
    if let Some(ref sid) = spec_id {
        let sid = sid.clone();
        let exists: i64 = db
            .read(move |conn| {
                let n: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM forge_specs WHERE id = ?1 AND user_id = ?2",
                    params![sid, user_id],
                    |row| row.get(0),
                )?;
                Ok(n)
            })
            .await?;
        if exists == 0 {
            return Err(EngError::InvalidInput(format!(
                "spec_id '{}' does not exist for this user",
                spec_id.as_deref().unwrap_or("")
            )));
        }
    }

    let now = Utc::now().timestamp();

    // Clone values needed inside the async closure.
    let spec_id_clone = spec_id.clone();
    let approaches_clone = approaches.clone();
    let chosen_index_clone = chosen_index;

    let stored_ids: Vec<String> = db
        .write(move |conn| {
            let mut ids = Vec::with_capacity(approaches_clone.len());
            for (i, approach) in approaches_clone.iter().enumerate() {
                let id = format!("appr_{}", &Uuid::new_v4().to_string()[..8]);
                let chosen = matches!(chosen_index_clone, Some(c) if c == i) as i64;
                let pros_json = serde_json::to_string(&approach.pros)
                    .map_err(crate::EngError::Serialization)?;
                let cons_json = serde_json::to_string(&approach.cons)
                    .map_err(crate::EngError::Serialization)?;
                conn.execute(
                    "INSERT INTO forge_approaches
                     (id, user_id, spec_id, created_at, name, description, pros, cons,
                      score, chosen)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        id,
                        user_id,
                        spec_id_clone,
                        now,
                        approach.name,
                        approach.description,
                        pros_json,
                        cons_json,
                        approach.score,
                        chosen,
                    ],
                )?;
                ids.push(id);
            }
            Ok(ids)
        })
        .await?;

    // Build the structured comparison prompt the agent can use for reasoning.
    let comparison_prompt = format!(
        "Evaluate the following approaches to: {}\n\n{}\n\n\
         For each approach, weigh pros vs cons. Identify the dominant factor \
         (correctness, complexity, performance, blast radius). Recommend one and \
         justify in 3 sentences or fewer.",
        problem,
        approaches
            .iter()
            .enumerate()
            .map(|(i, a)| format!(
                "Approach {}: {}\n  Description: {}\n  Pros: {}\n  Cons: {}\n  Score: {}",
                i,
                a.name,
                a.description,
                if a.pros.is_empty() {
                    "(none)".to_string()
                } else {
                    a.pros.join("; ")
                },
                if a.cons.is_empty() {
                    "(none)".to_string()
                } else {
                    a.cons.join("; ")
                },
                a.score
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "n/a".into()),
            ))
            .collect::<Vec<_>>()
            .join("\n\n"),
    );

    let approaches_out: Vec<Value> = approaches
        .iter()
        .enumerate()
        .map(|(i, a)| {
            serde_json::json!({
                "index": i,
                "name": a.name,
                "description": a.description,
                "pros": a.pros,
                "cons": a.cons,
                "score": a.score,
                "chosen": Some(i) == chosen_index,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "ids": stored_ids,
        "spec_id": spec_id,
        "problem": problem,
        "approaches": approaches_out,
        "comparison_prompt": comparison_prompt,
        "message": format!(
            "Stored {} approaches{}",
            approaches.len(),
            match chosen_index {
                Some(i) => format!(" (chose #{}: '{}')", i, approaches[i].name),
                None => " (no choice recorded yet)".into(),
            }
        ),
    }))
}
