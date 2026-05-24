//! Work queue for Chiasm tasks -- enqueue and atomic claim-next operations.
//!
//! Provides a simple FIFO work queue on top of the Chiasm task table. Tasks
//! are enqueued with status `queued` and `assigned = 0`, then claimed
//! atomically by an agent with `claim_next_task`. Claiming sets status to
//! `active`, flips `assigned` to 1, and records the initial heartbeat
//! timestamp -- all in a single write transaction to prevent double-claiming.

use crate::db::Database;
use crate::{EngError, Result};

/// Map a rusqlite error to the crate's EngError type.
fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

/// Enqueue a new task for any agent to pick up.
///
/// Creates a task with status `"queued"` and `assigned = 0` (unassigned).
/// The `agent` field is set to `"unassigned"` as a sentinel value that will
/// be replaced when `claim_next_task` claims the task.
///
/// Returns the newly created task record.
#[tracing::instrument(skip(db), fields(project, title, user_id))]
pub async fn enqueue_task(
    db: &Database,
    project: &str,
    title: &str,
    summary: Option<&str>,
    user_id: i64,
) -> Result<super::tasks::Task> {
    // Single-write INSERT with assigned = 0 to prevent a TOCTOU window where
    // another agent could claim the task between INSERT and UPDATE.
    let project_s = project.to_string();
    let title_s = title.to_string();
    let summary_s = summary.map(|s| s.to_string());
    let task_id = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO chiasm_tasks (agent, project, title, status, summary, \
                 output_format, heartbeat_interval, assigned, user_id) \
                 VALUES ('unassigned', ?1, ?2, 'queued', ?3, 'raw', 300, 0, ?4)",
                rusqlite::params![project_s, title_s, summary_s, user_id],
            )
            .map_err(rusqlite_to_eng_error)?;
            Ok(conn.last_insert_rowid())
        })
        .await?;
    let task = super::tasks::get_task(db, task_id, user_id).await?;
    super::emit_chiasm_event(
        db,
        "task.queued",
        serde_json::json!({
            "task_id": task.id,
            "project": task.project,
        }),
    )
    .await;
    Ok(task)
}

/// Atomically claim the oldest queued, unassigned task for an agent.
///
/// Finds the task with the earliest `created_at` among tasks where
/// `status = 'queued'` and `assigned = 0`, optionally filtered by project.
/// If a task is found, it is updated in the same write call to set:
/// - `agent` to the provided agent string
/// - `status` to `'active'`
/// - `assigned` to `1`
/// - `last_heartbeat` to the current UTC time
/// - `updated_at` to the current UTC time
///
/// Returns `None` if no queued, unassigned task is available.
/// Returns `Some(Task)` with the claimed task on success.
#[tracing::instrument(skip(db), fields(agent, project = ?project, user_id))]
pub async fn claim_next_task(
    db: &Database,
    agent: &str,
    project: Option<&str>,
    user_id: i64,
) -> Result<Option<super::tasks::Task>> {
    let agent_s = agent.to_string();
    let project_s = project.map(|p| p.to_string());

    let maybe_id: Option<i64> = db
        .write(move |conn| {
            // Build SELECT with optional project filter. Scoped to user_id so an
            // agent only claims tasks queued by its own user in single-DB mode.
            let id: Option<i64> = if let Some(ref proj) = project_s {
                let sql = "SELECT id FROM chiasm_tasks \
                           WHERE status = 'queued' AND assigned = 0 AND user_id = ?2 AND project = ?1 \
                           ORDER BY created_at ASC LIMIT 1";
                let mut stmt = conn.prepare(sql).map_err(rusqlite_to_eng_error)?;
                let mut rows = stmt
                    .query(rusqlite::params![proj, user_id])
                    .map_err(rusqlite_to_eng_error)?;
                rows.next()
                    .map_err(rusqlite_to_eng_error)?
                    .map(|row| row.get::<_, i64>(0).map_err(rusqlite_to_eng_error))
                    .transpose()?
            } else {
                let sql = "SELECT id FROM chiasm_tasks \
                           WHERE status = 'queued' AND assigned = 0 AND user_id = ?1 \
                           ORDER BY created_at ASC LIMIT 1";
                let mut stmt = conn.prepare(sql).map_err(rusqlite_to_eng_error)?;
                let mut rows = stmt
                    .query(rusqlite::params![user_id])
                    .map_err(rusqlite_to_eng_error)?;
                rows.next()
                    .map_err(rusqlite_to_eng_error)?
                    .map(|row| row.get::<_, i64>(0).map_err(rusqlite_to_eng_error))
                    .transpose()?
            };

            // If a task was found, claim it atomically within the same write call.
            if let Some(task_id) = id {
                conn.execute(
                    "UPDATE chiasm_tasks \
                     SET agent = ?1, status = 'active', assigned = 1, \
                         last_heartbeat = datetime('now'), updated_at = datetime('now') \
                     WHERE id = ?2 AND user_id = ?3",
                    rusqlite::params![agent_s, task_id, user_id],
                )
                .map_err(rusqlite_to_eng_error)?;
            }

            Ok(id)
        })
        .await?;

    match maybe_id {
        None => Ok(None),
        Some(id) => {
            let task = super::tasks::get_task(db, id, user_id).await?;
            super::emit_chiasm_event(
                db,
                "task.claimed",
                serde_json::json!({
                    "task_id": task.id,
                    "agent": task.agent,
                }),
            )
            .await;
            Ok(Some(task))
        }
    }
}

/// Unit tests.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    /// Initialize an in-memory database for testing.
    async fn setup() -> Database {
        Database::connect_memory().await.expect("db")
    }

    /// Enqueue a task, verify its initial state, claim it, verify the claim,
    /// then confirm a second claim attempt returns None (queue empty).
    #[tokio::test]
    async fn enqueue_and_claim() {
        let db = setup().await;

        // Enqueue a task.
        let queued = enqueue_task(&db, "test-project", "test-task", Some("do a thing"), 1)
            .await
            .expect("enqueue_task should succeed");

        // Freshly enqueued task must be queued and unassigned.
        assert_eq!(
            queued.status, "queued",
            "status should be queued after enqueue"
        );
        assert!(!queued.assigned, "assigned should be false after enqueue");
        assert_eq!(queued.agent, "unassigned");

        // Claim the task.
        let claimed = claim_next_task(&db, "agent-smith", None, 1)
            .await
            .expect("claim_next_task should succeed")
            .expect("should return Some when a queued task exists");

        // Claimed task must reflect the agent claim.
        assert_eq!(
            claimed.id, queued.id,
            "claimed task id should match enqueued id"
        );
        assert_eq!(
            claimed.status, "active",
            "status should be active after claim"
        );
        assert!(claimed.assigned, "assigned should be true after claim");
        assert_eq!(
            claimed.agent, "agent-smith",
            "agent should be updated to claiming agent"
        );

        // A second claim attempt must return None -- queue is empty.
        let none = claim_next_task(&db, "agent-smith", None, 1)
            .await
            .expect("second claim_next_task should not error");
        assert!(
            none.is_none(),
            "second claim should return None when queue is empty"
        );
    }
}
