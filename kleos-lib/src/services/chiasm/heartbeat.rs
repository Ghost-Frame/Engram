//! Heartbeat tracking and stale-task detection for Chiasm.
//!
//! Agents call `record_heartbeat` periodically to signal liveness. The
//! `mark_stale_tasks` sweep finds tasks whose heartbeat has not arrived within
//! the expected window and marks them stale so coordinators can reassign or
//! alert.

use crate::db::Database;
use crate::{EngError, Result};

/// Map a rusqlite error to the crate's EngError type.
fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

/// Record a heartbeat for the given task.
///
/// Updates `last_heartbeat` and `updated_at` to the current UTC time.
/// Also refreshes the expiry of any active path claims held by the task
/// (fire-and-forget -- claim refresh errors are silently ignored).
///
/// Returns `EngError::NotFound` if no task with the given ID exists.
pub async fn record_heartbeat(db: &Database, id: i64, user_id: i64) -> Result<()> {
    let changed = db
        .write(move |conn| {
            let n = conn
                .execute(
                    "UPDATE chiasm_tasks \
                     SET last_heartbeat = datetime('now'), \
                         updated_at     = datetime('now') \
                     WHERE id = ?1 AND user_id = ?2",
                    rusqlite::params![id, user_id],
                )
                .map_err(rusqlite_to_eng_error)?;
            Ok(n)
        })
        .await?;

    if changed == 0 {
        return Err(EngError::NotFound(format!("task {}", id)));
    }

    // Refresh claim expiry for this task. Errors here are intentionally ignored
    // so a missing-claims table or other transient issue does not fail the heartbeat.
    let _ = db
        .write(move |conn| {
            let _ = conn.execute(
                "UPDATE chiasm_path_claims \
                 SET expires_at = datetime('now', '+600 seconds') \
                 WHERE task_id = ?1 AND released = 0",
                rusqlite::params![id],
            );
            Ok(0usize)
        })
        .await;

    Ok(())
}

/// Scan for overdue tasks and mark them stale.
///
/// A task is considered overdue when all of the following are true:
/// - Its status is `active` or `paused`.
/// - `last_heartbeat` is not NULL (i.e. the task has sent at least one heartbeat).
/// - The time elapsed since `last_heartbeat` exceeds `heartbeat_interval *
///   grace_multiplier` seconds.
///
/// For each overdue task the function:
/// 1. Updates the task status to `"stale"` with the summary
///    `"marked stale: heartbeat overdue"`.
/// 2. Releases all path claims held by the task.
///
/// Returns the list of tasks that were transitioned to stale.
pub async fn mark_stale_tasks(
    db: &Database,
    grace_multiplier: f64,
) -> Result<Vec<super::tasks::Task>> {
    // Collect (id, owner) of every overdue task. This is a system-wide
    // maintenance sweep (not user-scoped); the owner is carried so the per-task
    // get_task readback below resolves under the correct user.
    let ids: Vec<(i64, i64)> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, user_id FROM chiasm_tasks \
                     WHERE status IN ('active', 'paused') \
                       AND last_heartbeat IS NOT NULL \
                       AND julianday('now') - julianday(last_heartbeat) \
                           > (heartbeat_interval * ?1 / 86400.0)",
                )
                .map_err(rusqlite_to_eng_error)?;
            let mut rows = stmt
                .query(rusqlite::params![grace_multiplier])
                .map_err(rusqlite_to_eng_error)?;
            let mut ids = Vec::new();
            while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                let id: i64 = row.get(0).map_err(rusqlite_to_eng_error)?;
                let owner: i64 = row.get(1).map_err(rusqlite_to_eng_error)?;
                ids.push((id, owner));
            }
            Ok(ids)
        })
        .await?;

    let mut stale = Vec::with_capacity(ids.len());
    for (task_id, owner) in ids {
        // Re-check the heartbeat condition at update time to prevent TOCTOU:
        // a concurrent heartbeat after the read must prevent staling.
        let gm = grace_multiplier;
        let affected = db
            .write(move |conn| {
                let n = conn
                    .execute(
                        "UPDATE chiasm_tasks SET status = 'stale', \
                         summary = 'marked stale: heartbeat overdue', \
                         updated_at = datetime('now') \
                         WHERE id = ?1 \
                           AND status IN ('active', 'paused') \
                           AND julianday('now') - julianday(last_heartbeat) \
                               > (heartbeat_interval * ?2 / 86400.0)",
                        rusqlite::params![task_id, gm],
                    )
                    .map_err(rusqlite_to_eng_error)?;
                if n > 0 {
                    conn.execute(
                        "INSERT INTO chiasm_task_updates (task_id, agent, status, summary) \
                         VALUES (?1, 'system', 'stale', 'marked stale: heartbeat overdue')",
                        rusqlite::params![task_id],
                    )
                    .map_err(rusqlite_to_eng_error)?;
                }
                Ok(n)
            })
            .await?;

        if affected > 0 {
            // Release path claims. Errors here are ignored -- the status change is
            // the authoritative signal; claim cleanup is best-effort.
            let _ = super::claims::release_claims(db, task_id).await;
            super::emit_chiasm_event(db, "task.stale", serde_json::json!({"task_id": task_id}))
                .await;
            if let Ok(task) = super::tasks::get_task(db, task_id, owner).await {
                stale.push(task);
            }
        }
    }

    Ok(stale)
}

/// Unit tests.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::chiasm::tasks::{create_task, get_task, CreateTaskRequest};

    /// Build a minimal `CreateTaskRequest` for test setup.
    fn req(title: &str) -> CreateTaskRequest {
        CreateTaskRequest {
            agent: "a".into(),
            project: "p".into(),
            title: title.into(),
            status: None,
            summary: None,
            user_id: Some(1),
            expected_output: None,
            output_format: None,
            condition: None,
            guardrail_url: None,
            heartbeat_interval: None,
        }
    }

    /// After calling `record_heartbeat`, `last_heartbeat` must be `Some`.
    #[tokio::test]
    async fn record_heartbeat_sets_timestamp() {
        let db = Database::connect_memory().await.expect("db");
        let task = create_task(&db, req("heartbeat-test")).await.unwrap();

        // Freshly created task has no heartbeat.
        assert!(task.last_heartbeat.is_none());

        record_heartbeat(&db, task.id, 1).await.unwrap();

        let updated = get_task(&db, task.id, 1).await.unwrap();
        assert!(
            updated.last_heartbeat.is_some(),
            "last_heartbeat should be set after record_heartbeat"
        );
    }

    /// A task whose heartbeat is overdue must be detected and marked stale.
    ///
    /// Setup: create a task with `heartbeat_interval = 60`, then manually
    /// backdate its `last_heartbeat` to 600 seconds ago. Calling
    /// `mark_stale_tasks(2.0)` requires the heartbeat to arrive within
    /// 60 * 2.0 = 120 seconds. Since 600 > 120 the task is overdue.
    #[tokio::test]
    async fn mark_stale_tasks_detects_overdue() {
        let db = Database::connect_memory().await.expect("db");

        let task = create_task(
            &db,
            CreateTaskRequest {
                heartbeat_interval: Some(60),
                ..req("stale-test")
            },
        )
        .await
        .unwrap();

        // Backdate heartbeat to 600 seconds ago so it is clearly overdue.
        db.write(move |conn| {
            conn.execute(
                "UPDATE chiasm_tasks \
                 SET last_heartbeat = datetime('now', '-600 seconds') \
                 WHERE id = ?1",
                rusqlite::params![task.id],
            )
            .map_err(rusqlite_to_eng_error)?;
            Ok(0usize)
        })
        .await
        .unwrap();

        let stale = mark_stale_tasks(&db, 2.0).await.unwrap();

        assert_eq!(stale.len(), 1, "exactly one task should be marked stale");
        assert_eq!(stale[0].id, task.id);
        assert_eq!(stale[0].status, "stale");
        assert_eq!(
            stale[0].summary.as_deref(),
            Some("marked stale: heartbeat overdue")
        );
    }
}
