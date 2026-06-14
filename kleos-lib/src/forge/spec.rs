//! Spec lifecycle: create, update, list, and fetch forge task specs.
//!
//! A spec is the mandatory pre-work record that an agent must create before
//! touching any code file. The gate enforcement layer queries `forge_specs`
//! via `spec_covers` to decide whether to allow a Write/Edit tool call.

use crate::db::Database;
use crate::EngError;
use chrono::Utc;
use rusqlite::params;
use serde_json::Value;
use uuid::Uuid;

/// Valid task type values enforced at spec creation time.
const VALID_TASK_TYPES: &[&str] = &[
    "feature",
    "bugfix",
    "refactor",
    "enhancement",
    "test",
    "docs",
];

/// Valid status values a spec can transition to.
const VALID_STATUSES: &[&str] = &["active", "completed", "failed", "blocked"];

/// Create a new task spec row in `forge_specs` and return its ID.
///
/// Requires at least 2 acceptance criteria and at least 3 edge cases.
/// `files_to_touch` is serialised as a JSON array; the gate enforcement
/// function (`spec_covers`) deserialises it at query time.
/// `session_id` ties the spec to a specific agent session for gate queries.
// spec_task mirrors the agent-forge SpecTaskInput shape; the parameters map 1:1
// to spec columns, so a struct would add indirection without clarity.
#[allow(clippy::too_many_arguments)]
pub async fn spec_task(
    db: &Database,
    user_id: i64,
    session_id: Option<&str>,
    task_description: String,
    task_type: String,
    acceptance_criteria: Vec<String>,
    interface_contract: String,
    edge_cases: Vec<String>,
    files_to_touch: Option<Vec<String>>,
    dependencies: Option<String>,
) -> crate::Result<Value> {
    // Validate task type.
    if !VALID_TASK_TYPES.contains(&task_type.as_str()) {
        return Err(EngError::InvalidInput(format!(
            "task_type must be one of: {}",
            VALID_TASK_TYPES.join(", ")
        )));
    }

    // Enforce minimum criteria counts.
    if acceptance_criteria.len() < 2 {
        return Err(EngError::InvalidInput(
            "Minimum 2 acceptance criteria required".into(),
        ));
    }
    if edge_cases.len() < 3 {
        return Err(EngError::InvalidInput(
            "Minimum 3 edge cases required".into(),
        ));
    }

    let id = format!("spec_{}", &Uuid::new_v4().to_string()[..8]);
    let now = Utc::now().timestamp();
    let criteria_json = serde_json::to_string(&acceptance_criteria)?;
    let edge_json = serde_json::to_string(&edge_cases)?;
    let files_json = files_to_touch
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    let session_id = session_id.map(|s| s.to_string());
    let id_clone = id.clone();

    db.write(move |conn| {
        conn.execute(
            "INSERT INTO forge_specs
             (id, user_id, session_id, created_at, task_description, task_type,
              acceptance_criteria, interface_contract, edge_cases, files_to_touch,
              dependencies, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'active')",
            params![
                id_clone,
                user_id,
                session_id,
                now,
                task_description,
                task_type,
                criteria_json,
                interface_contract,
                edge_json,
                files_json,
                dependencies,
            ],
        )?;
        Ok(())
    })
    .await?;

    Ok(serde_json::json!({
        "id": id,
        "message": "Spec created",
    }))
}

/// Transition a spec to a new status, recording an optional human note.
///
/// `completed_at` is set automatically when status is `completed` or `failed`.
/// Returns `NotFound` if `spec_id` does not exist for `user_id`.
pub async fn update_spec(
    db: &Database,
    user_id: i64,
    spec_id: String,
    status: String,
    note: Option<String>,
) -> crate::Result<Value> {
    if !VALID_STATUSES.contains(&status.as_str()) {
        return Err(EngError::InvalidInput(format!(
            "status must be one of: {}",
            VALID_STATUSES.join(", ")
        )));
    }

    let now = Utc::now().timestamp();
    let completed_at: Option<i64> = if status == "completed" || status == "failed" {
        Some(now)
    } else {
        None
    };
    let spec_id_for_err = spec_id.clone();

    let rows = db
        .write(move |conn| {
            let n = conn.execute(
                "UPDATE forge_specs SET status = ?1, status_note = ?2, completed_at = ?3
                 WHERE id = ?4 AND user_id = ?5",
                params![status, note, completed_at, spec_id, user_id],
            )?;
            Ok(n)
        })
        .await?;

    if rows == 0 {
        return Err(EngError::NotFound(format!(
            "Spec not found: {spec_id_for_err}"
        )));
    }

    Ok(serde_json::json!({ "message": format!("Spec marked as {}", rows) }))
}

/// Return specs for `user_id` ordered by creation time descending.
///
/// Optionally filtered by `status`. Results are capped at `limit` (default 20).
pub async fn list_specs(
    db: &Database,
    user_id: i64,
    status: Option<String>,
    limit: Option<usize>,
) -> crate::Result<Value> {
    let limit = limit.unwrap_or(20) as i64;

    let specs: Vec<Value> = db
        .read(move |conn| {
            let rows: Vec<Value> = if let Some(ref st) = status {
                let mut stmt = conn.prepare(
                    "SELECT id, task_description, task_type, status, created_at, completed_at,
                            status_note
                     FROM forge_specs
                     WHERE user_id = ?1 AND status = ?2
                     ORDER BY created_at DESC
                     LIMIT ?3",
                )?;
                let collected: Vec<Value> = stmt
                    .query_map(params![user_id, st, limit], |row| {
                        Ok(serde_json::json!({
                            "id": row.get::<_, String>(0)?,
                            "task_description": row.get::<_, String>(1)?,
                            "task_type": row.get::<_, String>(2)?,
                            "status": row.get::<_, String>(3)?,
                            "created_at": row.get::<_, i64>(4)?,
                            "completed_at": row.get::<_, Option<i64>>(5)?,
                            "status_note": row.get::<_, Option<String>>(6)?,
                        }))
                    })?
                    .filter_map(|r| r.ok())
                    .collect();
                collected
            } else {
                let mut stmt = conn.prepare(
                    "SELECT id, task_description, task_type, status, created_at, completed_at,
                            status_note
                     FROM forge_specs
                     WHERE user_id = ?1
                     ORDER BY created_at DESC
                     LIMIT ?2",
                )?;
                let collected: Vec<Value> = stmt
                    .query_map(params![user_id, limit], |row| {
                        Ok(serde_json::json!({
                            "id": row.get::<_, String>(0)?,
                            "task_description": row.get::<_, String>(1)?,
                            "task_type": row.get::<_, String>(2)?,
                            "status": row.get::<_, String>(3)?,
                            "created_at": row.get::<_, i64>(4)?,
                            "completed_at": row.get::<_, Option<i64>>(5)?,
                            "status_note": row.get::<_, Option<String>>(6)?,
                        }))
                    })?
                    .filter_map(|r| r.ok())
                    .collect();
                collected
            };
            Ok(rows)
        })
        .await?;

    Ok(serde_json::json!({ "specs": specs, "count": specs.len() }))
}

/// Fetch one full spec by ID together with all related sub-records.
///
/// The returned JSON includes the spec row plus `hypotheses`, `approaches`,
/// `learnings`, and `verifications` arrays so callers see the complete
/// reasoning history in one call. Returns `NotFound` if not found.
pub async fn get_spec(db: &Database, user_id: i64, spec_id: String) -> crate::Result<Value> {
    db.read(move |conn| {
        // Fetch the spec row.
        let spec: Value = conn
            .query_row(
                "SELECT id, task_description, task_type, acceptance_criteria,
                        interface_contract, edge_cases, files_to_touch, dependencies,
                        status, created_at, completed_at, status_note
                 FROM forge_specs
                 WHERE id = ?1 AND user_id = ?2",
                params![spec_id, user_id],
                |row| {
                    Ok(serde_json::json!({
                        "id": row.get::<_, String>(0)?,
                        "task_description": row.get::<_, String>(1)?,
                        "task_type": row.get::<_, String>(2)?,
                        "acceptance_criteria": row.get::<_, String>(3)?,
                        "interface_contract": row.get::<_, Option<String>>(4)?,
                        "edge_cases": row.get::<_, Option<String>>(5)?,
                        "files_to_touch": row.get::<_, Option<String>>(6)?,
                        "dependencies": row.get::<_, Option<String>>(7)?,
                        "status": row.get::<_, String>(8)?,
                        "created_at": row.get::<_, i64>(9)?,
                        "completed_at": row.get::<_, Option<i64>>(10)?,
                        "status_note": row.get::<_, Option<String>>(11)?,
                    }))
                },
            )
            .map_err(|_| EngError::NotFound(format!("Spec not found: {spec_id}")))?;

        // Related hypotheses.
        let hypotheses: Vec<Value> = {
            let mut stmt = conn.prepare(
                "SELECT id, hypothesis, outcome, confidence
                 FROM forge_hypotheses
                 WHERE spec_id = ?1 AND user_id = ?2
                 ORDER BY created_at DESC",
            )?;
            let collected: Vec<Value> = stmt
                .query_map(params![spec_id, user_id], |row| {
                    Ok(serde_json::json!({
                        "id": row.get::<_, String>(0)?,
                        "hypothesis": row.get::<_, String>(1)?,
                        "outcome": row.get::<_, Option<String>>(2)?,
                        "confidence": row.get::<_, f64>(3)?,
                    }))
                })?
                .filter_map(|r| r.ok())
                .collect();
            collected
        };

        // Related approaches.
        let approaches: Vec<Value> = {
            let mut stmt = conn.prepare(
                "SELECT id, name, score, chosen
                 FROM forge_approaches
                 WHERE spec_id = ?1 AND user_id = ?2
                 ORDER BY created_at DESC",
            )?;
            let collected: Vec<Value> = stmt
                .query_map(params![spec_id, user_id], |row| {
                    Ok(serde_json::json!({
                        "id": row.get::<_, String>(0)?,
                        "name": row.get::<_, String>(1)?,
                        "score": row.get::<_, Option<f64>>(2)?,
                        "chosen": row.get::<_, i64>(3)?,
                    }))
                })?
                .filter_map(|r| r.ok())
                .collect();
            collected
        };

        // Related session learnings.
        let learnings: Vec<Value> = {
            let mut stmt = conn.prepare(
                "SELECT id, discovery
                 FROM forge_session_learns
                 WHERE spec_id = ?1 AND user_id = ?2
                 ORDER BY created_at DESC",
            )?;
            let collected: Vec<Value> = stmt
                .query_map(params![spec_id, user_id], |row| {
                    Ok(serde_json::json!({
                        "id": row.get::<_, String>(0)?,
                        "discovery": row.get::<_, String>(1)?,
                    }))
                })?
                .filter_map(|r| r.ok())
                .collect();
            collected
        };

        // Related verification records.
        let verifications: Vec<Value> = {
            let mut stmt = conn.prepare(
                "SELECT id, command, success, duration_ms, criteria_index
                 FROM forge_verifications
                 WHERE spec_id = ?1 AND user_id = ?2
                 ORDER BY created_at DESC",
            )?;
            let collected: Vec<Value> = stmt
                .query_map(params![spec_id, user_id], |row| {
                    Ok(serde_json::json!({
                        "id": row.get::<_, String>(0)?,
                        "command": row.get::<_, String>(1)?,
                        "success": row.get::<_, bool>(2)?,
                        "duration_ms": row.get::<_, Option<i64>>(3)?,
                        "criteria_index": row.get::<_, Option<i64>>(4)?,
                    }))
                })?
                .filter_map(|r| r.ok())
                .collect();
            collected
        };

        Ok(serde_json::json!({
            "spec": spec,
            "hypotheses": hypotheses,
            "approaches": approaches,
            "learnings": learnings,
            "verifications": verifications,
        }))
    })
    .await
}

/// Return true if there is an active spec for `(user_id, session_id)` that
/// covers `file_path`.
///
/// Coverage check logic (fail-open within session, fail-closed without spec):
///   - If no active spec exists for (user_id, session_id): returns false.
///   - If an active spec exists AND its `files_to_touch` is NULL, empty JSON
///     array `[]`, or an empty string: returns true (spec covers all files --
///     the agent declared the task without enumerating touched files, which is
///     valid for exploratory tasks).
///   - If an active spec exists AND `files_to_touch` is a non-empty JSON array:
///     returns true only if `file_path` appears in that array.
///
/// This is the primary gate query used by `kleos-lib::gate` to block Write/Edit
/// calls when no spec covers the target file.
pub async fn spec_covers(
    db: &Database,
    user_id: i64,
    session_id: &str,
    file_path: &str,
) -> crate::Result<bool> {
    let session_id = session_id.to_string();
    let file_path = file_path.to_string();

    db.read(move |conn| {
        // Fetch all active specs for this (user, session).
        let mut stmt = conn.prepare(
            "SELECT files_to_touch FROM forge_specs
             WHERE user_id = ?1 AND session_id = ?2 AND status = 'active'",
        )?;

        let rows: Vec<Option<String>> = stmt
            .query_map(params![user_id, session_id], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        if rows.is_empty() {
            // No active spec for this (user, session) -- gate must deny.
            return Ok(false);
        }

        // At least one active spec exists. Check whether any of them cover the
        // target file. A spec with NULL/empty files_to_touch covers all files.
        for files_json in rows {
            let covered = match files_json.as_deref() {
                // NULL or empty string -- spec covers all files.
                None | Some("") | Some("[]") => true,
                Some(json) => {
                    // Parse the JSON array and check membership.
                    serde_json::from_str::<Vec<String>>(json)
                        .map(|arr| arr.iter().any(|f| f == &file_path))
                        .unwrap_or(false)
                }
            };
            if covered {
                return Ok(true);
            }
        }

        // Active specs exist but none covers this file.
        Ok(false)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    /// Open an in-memory database with full migrations applied (including migration 91
    /// which creates all forge_* tables).
    async fn setup_db() -> Database {
        Database::connect_memory().await.expect("in-memory db")
    }

    /// Convenience: create a valid spec with the given files_to_touch and return its id string.
    async fn make_spec(
        db: &Database,
        user_id: i64,
        session_id: Option<&str>,
        files: Option<Vec<String>>,
    ) -> String {
        let result = spec_task(
            db,
            user_id,
            session_id,
            "Add a new feature to the auth module".to_string(),
            "feature".to_string(),
            vec![
                "New API endpoint returns 200 for valid input".to_string(),
                "Invalid input returns 400 with error message".to_string(),
            ],
            "fn new_endpoint(input: Input) -> Result<Output, Error>".to_string(),
            vec![
                "Empty input fields are rejected".to_string(),
                "Oversized payload returns 413".to_string(),
                "Concurrent requests do not corrupt state".to_string(),
            ],
            files,
            None,
        )
        .await
        .expect("spec_task should succeed");

        result
            .get("id")
            .and_then(|v| v.as_str())
            .expect("result must have an 'id' string field")
            .to_string()
    }

    /// TEST 1a: forge_specs table exists after migrations.
    #[tokio::test]
    async fn forge_specs_table_exists_after_migrations() {
        let db = setup_db().await;

        // Query sqlite_master to verify the table was created.
        let exists: bool = db
            .read(|conn| {
                Ok(conn
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='forge_specs'",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .map(|n| n > 0)
                    .unwrap_or(false))
            })
            .await
            .expect("sqlite_master query should not fail");

        assert!(exists, "forge_specs table must exist after migrations");
    }

    /// TEST 1b: spec_task returns a spec id for a well-formed input.
    #[tokio::test]
    async fn spec_task_creates_spec_and_returns_id() {
        let db = setup_db().await;

        let result = spec_task(
            &db,
            1,
            Some("S1"),
            "Implement rate-limiting middleware".to_string(),
            "feature".to_string(),
            vec![
                "Rate limit applies to all /api/* routes".to_string(),
                "Returns 429 with Retry-After header when limit exceeded".to_string(),
            ],
            "fn rate_limit_middleware(req: Request) -> Response".to_string(),
            vec![
                "First request within window is allowed".to_string(),
                "Request exactly at limit boundary is allowed".to_string(),
                "Request exceeding limit is rejected with 429".to_string(),
            ],
            Some(vec!["src/lib.rs".to_string()]),
            None,
        )
        .await
        .expect("spec_task with valid input should succeed");

        // Must return an id field with the "spec_" prefix.
        let id = result
            .get("id")
            .and_then(|v| v.as_str())
            .expect("result must contain 'id' string");
        assert!(
            id.starts_with("spec_"),
            "id must start with 'spec_', got: {id}"
        );
    }

    /// TEST 1c: spec_covers returns true for a file declared in files_to_touch.
    #[tokio::test]
    async fn spec_covers_true_for_declared_file() {
        let db = setup_db().await;

        make_spec(&db, 1, Some("S1"), Some(vec!["src/lib.rs".to_string()])).await;

        let covered = spec_covers(&db, 1, "S1", "src/lib.rs")
            .await
            .expect("spec_covers should not error");

        assert!(
            covered,
            "spec_covers must be true for declared file src/lib.rs"
        );
    }

    /// TEST 1d: spec_covers returns false for a file not declared in files_to_touch.
    #[tokio::test]
    async fn spec_covers_false_for_undeclared_file() {
        let db = setup_db().await;

        make_spec(&db, 1, Some("S1"), Some(vec!["src/lib.rs".to_string()])).await;

        let covered = spec_covers(&db, 1, "S1", "src/other.rs")
            .await
            .expect("spec_covers should not error");

        assert!(
            !covered,
            "spec_covers must be false for undeclared file src/other.rs"
        );
    }

    /// TEST 1e: spec_covers returns false when session_id does not match.
    #[tokio::test]
    async fn spec_covers_false_for_different_session() {
        let db = setup_db().await;

        // Spec is created for session S1.
        make_spec(&db, 1, Some("S1"), Some(vec!["src/lib.rs".to_string()])).await;

        // Query with session S2 -- different session, no spec.
        let covered = spec_covers(&db, 1, "S2", "src/lib.rs")
            .await
            .expect("spec_covers should not error");

        assert!(
            !covered,
            "spec_covers must be false for session S2 (spec is for S1)"
        );
    }

    /// TEST 1f: spec_covers returns false when user_id does not match (tenant isolation).
    #[tokio::test]
    async fn spec_covers_false_for_different_user() {
        let db = setup_db().await;

        // Spec is created for user_id=1.
        make_spec(&db, 1, Some("S1"), Some(vec!["src/lib.rs".to_string()])).await;

        // Query with user_id=2 -- different tenant.
        let covered = spec_covers(&db, 2, "S1", "src/lib.rs")
            .await
            .expect("spec_covers should not error");

        assert!(
            !covered,
            "spec_covers must be false for user_id=2 (tenant isolation)"
        );
    }

    /// TEST 1g: after update_spec marks a spec "completed", spec_covers returns false.
    #[tokio::test]
    async fn spec_covers_false_after_completed() {
        let db = setup_db().await;

        let spec_id = make_spec(&db, 1, Some("S1"), Some(vec!["src/lib.rs".to_string()])).await;

        // Confirm coverage is true before completing.
        let before = spec_covers(&db, 1, "S1", "src/lib.rs")
            .await
            .expect("spec_covers before completion");
        assert!(
            before,
            "precondition: spec_covers must be true before completing"
        );

        // Mark as completed.
        update_spec(&db, 1, spec_id, "completed".to_string(), None)
            .await
            .expect("update_spec should succeed");

        // Coverage must now be false (no active spec).
        let after = spec_covers(&db, 1, "S1", "src/lib.rs")
            .await
            .expect("spec_covers after completion");
        assert!(!after, "spec_covers must be false after spec is completed");
    }

    /// TEST 1h (empty-files fallback): a spec with empty files_to_touch covers any path.
    ///
    /// Per the spec_covers doc comment: "NULL, empty JSON array `[]`, or an empty
    /// string" all mean the spec covers all files for the session.
    #[tokio::test]
    async fn spec_covers_true_for_empty_files_to_touch() {
        let db = setup_db().await;

        // files_to_touch = None -> stored as NULL -> covers all files.
        make_spec(&db, 1, Some("S1"), None).await;

        let covered = spec_covers(&db, 1, "S1", "src/arbitrary/path/foo.rs")
            .await
            .expect("spec_covers should not error");

        assert!(
            covered,
            "spec with NULL files_to_touch must cover any arbitrary file path"
        );
    }
}
