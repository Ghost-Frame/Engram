//! Heartbeat tracking and stale-task detection for Chiasm.
//!
//! Agents call `record_heartbeat` periodically to signal liveness. The
//! `mark_stale_tasks` sweep finds tasks whose heartbeat has not arrived within
//! the expected window and marks them stale so coordinators can reassign or
//! alert.

use crate::db::Database;
use crate::{EngError, Result};

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
            let n = conn.execute(
                "UPDATE chiasm_tasks \
                     SET last_heartbeat = datetime('now'), \
                         updated_at     = datetime('now') \
                     WHERE id = ?1 AND user_id = ?2",
                rusqlite::params![id, user_id],
            )?;
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
/// A task (status `active` or `paused`) is stale under either condition:
/// - Heartbeat overdue: `last_heartbeat` is set and the elapsed time since it
///   exceeds `heartbeat_interval * grace_multiplier` seconds.
/// - Idle without heartbeat: `last_heartbeat` is NULL (the task never sent one,
///   e.g. created via `activity task.started`) and `updated_at` is older than
///   `no_heartbeat_idle_secs`. Without this branch never-heartbeated tasks were
///   immune to staling and accumulated as permanent ghosts in the active set.
///
/// For each stale task the function:
/// 1. Sets status to `"stale"` with a summary describing the reason
///    (`"marked stale: heartbeat overdue"` or `"marked stale: idle, no heartbeat"`).
/// 2. Releases all path claims held by the task.
///
/// Returns the list of tasks that were transitioned to stale.
pub async fn mark_stale_tasks(
    db: &Database,
    grace_multiplier: f64,
    no_heartbeat_idle_secs: i64,
) -> Result<Vec<super::tasks::Task>> {
    // Collect (id, owner, reason) of every stale task. This is a system-wide
    // maintenance sweep (not user-scoped); the owner is carried so the per-task
    // get_task readback below resolves under the correct user. `reason`
    // distinguishes the heartbeat-overdue path from the never-heartbeated path
    // so each gets an accurate summary.
    let ids: Vec<(i64, i64, String)> = db
        .read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, user_id, \
                        CASE WHEN last_heartbeat IS NULL THEN 'idle' ELSE 'overdue' END \
                     FROM chiasm_tasks \
                     WHERE status IN ('active', 'paused') \
                       AND ( \
                         (last_heartbeat IS NOT NULL \
                          AND julianday('now') - julianday(last_heartbeat) \
                              > (heartbeat_interval * ?1 / 86400.0)) \
                         OR \
                         (last_heartbeat IS NULL \
                          AND julianday('now') - julianday(updated_at) \
                              > (?2 / 86400.0)) \
                       )",
            )?;
            let mut rows =
                stmt.query(rusqlite::params![grace_multiplier, no_heartbeat_idle_secs])?;
            let mut ids = Vec::new();
            while let Some(row) = rows.next()? {
                let id: i64 = row.get(0)?;
                let owner: i64 = row.get(1)?;
                let reason: String = row.get(2)?;
                ids.push((id, owner, reason));
            }
            Ok(ids)
        })
        .await?;

    let mut stale = Vec::with_capacity(ids.len());
    for (task_id, owner, reason) in ids {
        // Re-check the stale condition at update time to prevent TOCTOU: a
        // concurrent heartbeat or update after the read must prevent staling.
        let gm = grace_multiplier;
        let idle = no_heartbeat_idle_secs;
        // Summary reflects which branch matched, set from the read's reason.
        let summary_text = if reason == "idle" {
            "marked stale: idle, no heartbeat"
        } else {
            "marked stale: heartbeat overdue"
        };
        let affected = db
            .write(move |conn| {
                let n = conn.execute(
                    "UPDATE chiasm_tasks SET status = 'stale', \
                         summary = ?2, \
                         updated_at = datetime('now') \
                         WHERE id = ?1 \
                           AND status IN ('active', 'paused') \
                           AND ( \
                             (last_heartbeat IS NOT NULL \
                              AND julianday('now') - julianday(last_heartbeat) \
                                  > (heartbeat_interval * ?3 / 86400.0)) \
                             OR \
                             (last_heartbeat IS NULL \
                              AND julianday('now') - julianday(updated_at) \
                                  > (?4 / 86400.0)) \
                           )",
                    rusqlite::params![task_id, summary_text, gm, idle],
                )?;
                if n > 0 {
                    conn.execute(
                        "INSERT INTO chiasm_task_updates (task_id, agent, status, summary) \
                         VALUES (?1, 'system', 'stale', ?2)",
                        rusqlite::params![task_id, summary_text],
                    )?;
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
            )?;
            Ok(0usize)
        })
        .await
        .unwrap();

        let stale = mark_stale_tasks(&db, 2.0, 3600).await.unwrap();

        assert_eq!(stale.len(), 1, "exactly one task should be marked stale");
        assert_eq!(stale[0].id, task.id);
        assert_eq!(stale[0].status, "stale");
        assert_eq!(
            stale[0].summary.as_deref(),
            Some("marked stale: heartbeat overdue")
        );
    }

    /// A task that never sent a heartbeat (last_heartbeat NULL) and has been
    /// idle past the no-heartbeat window must be staled. This is the case that
    /// was previously immune: `activity task.started` tasks never heartbeat.
    #[tokio::test]
    async fn mark_stale_tasks_detects_idle_no_heartbeat() {
        let db = Database::connect_memory().await.expect("db");
        let task = create_task(&db, req("idle-no-hb")).await.unwrap();
        assert!(task.last_heartbeat.is_none());

        // Backdate updated_at to 2 hours ago; leave last_heartbeat NULL.
        db.write(move |conn| {
            conn.execute(
                "UPDATE chiasm_tasks SET updated_at = datetime('now', '-7200 seconds') \
                 WHERE id = ?1",
                rusqlite::params![task.id],
            )?;
            Ok(0usize)
        })
        .await
        .unwrap();

        // 1-hour idle window: a 2-hour-idle, never-heartbeated task is stale.
        let stale = mark_stale_tasks(&db, 2.0, 3600).await.unwrap();
        assert_eq!(stale.len(), 1, "idle no-heartbeat task should be staled");
        assert_eq!(stale[0].id, task.id);
        assert_eq!(stale[0].status, "stale");
        assert_eq!(
            stale[0].summary.as_deref(),
            Some("marked stale: idle, no heartbeat")
        );
    }

    /// A freshly created task with no heartbeat is within the idle window and
    /// must NOT be staled (otherwise every new task would die immediately).
    #[tokio::test]
    async fn mark_stale_tasks_spares_fresh_no_heartbeat() {
        let db = Database::connect_memory().await.expect("db");
        let task = create_task(&db, req("fresh-no-hb")).await.unwrap();

        let stale = mark_stale_tasks(&db, 2.0, 3600).await.unwrap();
        assert!(
            stale.is_empty(),
            "a just-created no-heartbeat task must not be staled"
        );
        let still = get_task(&db, task.id, 1).await.unwrap();
        assert_eq!(still.status, "active");
    }
}
