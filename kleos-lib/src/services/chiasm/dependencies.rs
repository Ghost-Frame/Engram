//! Task dependency management for Chiasm.
//!
//! Implements a DAG (directed acyclic graph) of task dependencies with BFS-based
//! circular dependency detection and automatic unblocking when all dependencies
//! of a blocked task are completed.

use crate::db::Database;
use crate::{EngError, Result};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// A dependency edge: task `task_id` depends on task `depends_on`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    /// Row ID of the dependency edge.
    pub id: i64,
    /// The task that has the dependency.
    pub task_id: i64,
    /// The task that must complete first.
    pub depends_on: i64,
    /// Title of the depended-on task (joined from chiasm_tasks).
    pub depends_on_title: Option<String>,
    /// Current status of the depended-on task.
    pub depends_on_status: Option<String>,
    /// When this dependency was created.
    pub created_at: String,
}

/// Check whether adding `target_id` as a dependency of `task_id` would create
/// a cycle. Uses BFS from `target_id` following existing dependency edges; if
/// the traversal reaches `task_id`, a cycle exists.
pub async fn has_circular_dependency(db: &Database, task_id: i64, target_id: i64) -> Result<bool> {
    let tid = task_id;
    db.read(move |conn| {
        let mut visited = std::collections::HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(target_id);

        while let Some(current) = queue.pop_front() {
            if current == tid {
                return Ok(true);
            }
            if !visited.insert(current) {
                continue;
            }
            let mut stmt =
                conn.prepare("SELECT depends_on FROM chiasm_task_dependencies WHERE task_id = ?1")?;
            let mut rows = stmt.query(rusqlite::params![current])?;
            while let Some(row) = rows.next()? {
                let dep: i64 = row.get(0)?;
                queue.push_back(dep);
            }
        }
        Ok(false)
    })
    .await
}

/// Add one or more dependency edges. Each target in `depends_on` is validated
/// for self-references and circular dependencies before insertion.
///
/// `user_id` is the caller's effective user. Every `depends_on` target must
/// belong to that user: without this check a caller who owns `task_id` could
/// point a dependency at another tenant's task, writing a cross-tenant edge
/// and leaking the target's title and status back through `get_dependencies`
/// in monolith mode. The predicate is the tenant boundary in monolith mode
/// and a no-op in a single-owner shard (where every task shares the owner).
pub async fn add_dependencies(
    db: &Database,
    task_id: i64,
    depends_on: &[i64],
    user_id: i64,
) -> Result<()> {
    for &dep_id in depends_on {
        if dep_id == task_id {
            return Err(EngError::InvalidInput(
                "task cannot depend on itself".into(),
            ));
        }
        // Fail closed (NotFound) when the dependency target is not owned by
        // the caller, mirroring get_task's ownership predicate.
        let owns_target = db
            .read(move |conn| {
                Ok(conn
                    .query_row(
                        "SELECT 1 FROM chiasm_tasks WHERE id = ?1 AND user_id = ?2",
                        rusqlite::params![dep_id, user_id],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some())
            })
            .await?;
        if !owns_target {
            return Err(EngError::NotFound(format!(
                "dependency target task {} not found",
                dep_id
            )));
        }
        if has_circular_dependency(db, task_id, dep_id).await? {
            return Err(EngError::InvalidInput(format!(
                "circular dependency: {} -> {} creates a cycle",
                task_id, dep_id
            )));
        }
    }

    let deps = depends_on.to_vec();
    db.write(move |conn| {
        for dep_id in deps {
            conn.execute(
                "INSERT OR IGNORE INTO chiasm_task_dependencies (task_id, depends_on) \
                 VALUES (?1, ?2)",
                rusqlite::params![task_id, dep_id],
            )?;
        }
        Ok(())
    })
    .await
}

/// List all dependencies for a task, joining the depended-on task's title and status.
pub async fn get_dependencies(db: &Database, task_id: i64) -> Result<Vec<Dependency>> {
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT d.id, d.task_id, d.depends_on, t.title, t.status, d.created_at \
                 FROM chiasm_task_dependencies d \
                 LEFT JOIN chiasm_tasks t ON t.id = d.depends_on \
                 WHERE d.task_id = ?1 \
                 ORDER BY d.id ASC",
        )?;
        let mut rows = stmt.query(rusqlite::params![task_id])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(Dependency {
                id: row.get(0)?,
                task_id: row.get(1)?,
                depends_on: row.get(2)?,
                depends_on_title: row.get(3)?,
                depends_on_status: row.get(4)?,
                created_at: row.get(5)?,
            });
        }
        Ok(out)
    })
    .await
}

/// Remove a single dependency edge.
pub async fn remove_dependency(db: &Database, task_id: i64, dep_id: i64) -> Result<bool> {
    let n = db
        .write(move |conn| {
            Ok(conn.execute(
                "DELETE FROM chiasm_task_dependencies WHERE task_id = ?1 AND depends_on = ?2",
                rusqlite::params![task_id, dep_id],
            )?)
        })
        .await?;
    Ok(n > 0)
}

/// After a task completes, check all tasks that depend on it. If ALL of a
/// dependent task's dependencies are now completed, auto-unblock it by
/// setting its status to "active".
pub async fn check_and_unblock(
    db: &Database,
    completed_task_id: i64,
) -> Result<Vec<super::tasks::Task>> {
    // Find all tasks that have a dependency on the completed task, along with
    // each dependent task's owner. The owner is needed so the auto-unblock
    // update is scoped to the task's real tenant: update_task gates on user_id,
    // so a hardcoded owner would silently fail to unblock any task not owned by
    // that user. Dependency edges are same-tenant by construction (see
    // add_dependencies), so the owner is well-defined.
    let dependents: Vec<(i64, i64)> = db
        .read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT d.task_id, t.user_id \
                     FROM chiasm_task_dependencies d \
                     JOIN chiasm_tasks t ON t.id = d.task_id \
                     WHERE d.depends_on = ?1",
            )?;
            let mut rows = stmt.query(rusqlite::params![completed_task_id])?;
            let mut ids = Vec::new();
            while let Some(row) = rows.next()? {
                ids.push((row.get(0)?, row.get(1)?));
            }
            Ok(ids)
        })
        .await?;

    let mut unblocked = Vec::new();
    for (task_id, owner_id) in dependents {
        // Check if ALL dependencies of this task are now completed.
        // The just-completed task is excluded from the "not yet done" count
        // because the caller invokes check_and_unblock before (or without)
        // committing the status change to the DB.
        let all_complete: bool = db
            .read(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT COUNT(*) FROM chiasm_task_dependencies d \
                         JOIN chiasm_tasks t ON t.id = d.depends_on \
                         WHERE d.task_id = ?1 \
                           AND d.depends_on != ?2 \
                           AND t.status != 'completed'",
                )?;
                let count: i64 =
                    stmt.query_row(rusqlite::params![task_id, completed_task_id], |r| r.get(0))?;
                Ok(count == 0)
            })
            .await?;

        if all_complete {
            let task = super::tasks::update_task(
                db,
                task_id,
                super::tasks::UpdateTaskRequest {
                    title: None,
                    status: Some("active".into()),
                    summary: Some("auto-unblocked: all dependencies completed".into()),
                    agent: None,
                },
                owner_id,
            )
            .await?;
            super::emit_chiasm_event(
                db,
                "task.unblocked",
                serde_json::json!({
                    "task_id": task.id,
                    "completed_dependency": completed_task_id,
                }),
            )
            .await;
            unblocked.push(task);
        }
    }
    Ok(unblocked)
}

/// Unit tests.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::chiasm::tasks::{create_task, CreateTaskRequest};

    /// Creates an in-memory database for testing.
    async fn setup() -> Database {
        Database::connect_memory().await.expect("db")
    }

    /// Helper to build a minimal CreateTaskRequest for tests.
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

    /// Test: dependencies can be added and retrieved for a task.
    #[tokio::test]
    async fn add_and_list_dependencies() {
        let db = setup().await;
        let t1 = create_task(&db, req("task-1")).await.unwrap();
        let t2 = create_task(&db, req("task-2")).await.unwrap();
        let t3 = create_task(&db, req("task-3")).await.unwrap();

        add_dependencies(&db, t3.id, &[t1.id, t2.id], 1)
            .await
            .unwrap();

        let deps = get_dependencies(&db, t3.id).await.unwrap();
        assert_eq!(deps.len(), 2);
        let dep_ids: Vec<i64> = deps.iter().map(|d| d.depends_on).collect();
        assert!(dep_ids.contains(&t1.id));
        assert!(dep_ids.contains(&t2.id));
    }

    /// Test: a dependency target owned by another user is rejected, so a
    /// caller cannot create a cross-tenant dependency edge.
    #[tokio::test]
    async fn cross_tenant_dependency_target_rejected() {
        let db = setup().await;
        let mut mine = req("mine");
        mine.user_id = Some(1);
        let t_mine = create_task(&db, mine).await.unwrap();
        let mut theirs = req("theirs");
        theirs.user_id = Some(2);
        let t_theirs = create_task(&db, theirs).await.unwrap();

        // User 1 tries to depend on user 2's task.
        let result = add_dependencies(&db, t_mine.id, &[t_theirs.id], 1).await;
        assert!(
            matches!(result, Err(EngError::NotFound(_))),
            "cross-tenant dependency target must be rejected, got: {:?}",
            result
        );

        // No edge was written.
        let deps = get_dependencies(&db, t_mine.id).await.unwrap();
        assert!(deps.is_empty(), "no cross-tenant edge should be inserted");
    }

    /// Test: adding a dependency that would create a cycle is rejected with an error.
    #[tokio::test]
    async fn circular_dependency_rejected() {
        let db = setup().await;
        let t1 = create_task(&db, req("task-1")).await.unwrap();
        let t2 = create_task(&db, req("task-2")).await.unwrap();

        add_dependencies(&db, t2.id, &[t1.id], 1).await.unwrap();

        let result = add_dependencies(&db, t1.id, &[t2.id], 1).await;
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("circular"),
            "expected circular error, got: {}",
            err_msg
        );
    }

    /// Test: a task cannot depend on itself.
    #[tokio::test]
    async fn self_dependency_rejected() {
        let db = setup().await;
        let t1 = create_task(&db, req("task-1")).await.unwrap();
        let result = add_dependencies(&db, t1.id, &[t1.id], 1).await;
        assert!(result.is_err());
    }

    /// Test: a dependency can be removed and the list is updated accordingly.
    #[tokio::test]
    async fn remove_dependency_works() {
        let db = setup().await;
        let t1 = create_task(&db, req("task-1")).await.unwrap();
        let t2 = create_task(&db, req("task-2")).await.unwrap();
        add_dependencies(&db, t2.id, &[t1.id], 1).await.unwrap();

        let removed = remove_dependency(&db, t2.id, t1.id).await.unwrap();
        assert!(removed);

        let deps = get_dependencies(&db, t2.id).await.unwrap();
        assert!(deps.is_empty());
    }

    /// Test: completing a blocking task automatically unblocks its dependents.
    #[tokio::test]
    async fn auto_unblock_on_completion() {
        let db = setup().await;
        let t1 = create_task(&db, req("blocker")).await.unwrap();
        let t2 = create_task(&db, req("blocked")).await.unwrap();

        add_dependencies(&db, t2.id, &[t1.id], 1).await.unwrap();

        // Mark t2 as blocked
        crate::services::chiasm::tasks::update_task(
            &db,
            t2.id,
            crate::services::chiasm::tasks::UpdateTaskRequest {
                title: None,
                status: Some("blocked".into()),
                summary: None,
                agent: None,
            },
            1,
        )
        .await
        .unwrap();

        // Complete the blocker
        let unblocked = check_and_unblock(&db, t1.id).await.unwrap();
        assert_eq!(unblocked.len(), 1);
        assert_eq!(unblocked[0].id, t2.id);
        assert_eq!(unblocked[0].status, "active");
    }

    /// Test: auto-unblock works for a tenant other than user 1. Regression for
    /// the hardcoded user_id=1 that silently skipped the update_task ownership
    /// gate for every other tenant.
    #[tokio::test]
    async fn auto_unblock_respects_task_owner() {
        let db = setup().await;
        let mut blocker = req("blocker");
        blocker.user_id = Some(7);
        let t1 = create_task(&db, blocker).await.unwrap();
        let mut blocked = req("blocked");
        blocked.user_id = Some(7);
        let t2 = create_task(&db, blocked).await.unwrap();

        add_dependencies(&db, t2.id, &[t1.id], 7).await.unwrap();
        crate::services::chiasm::tasks::update_task(
            &db,
            t2.id,
            crate::services::chiasm::tasks::UpdateTaskRequest {
                title: None,
                status: Some("blocked".into()),
                summary: None,
                agent: None,
            },
            7,
        )
        .await
        .unwrap();

        let unblocked = check_and_unblock(&db, t1.id).await.unwrap();
        assert_eq!(unblocked.len(), 1, "user 7's task must be auto-unblocked");
        assert_eq!(unblocked[0].id, t2.id);
        assert_eq!(unblocked[0].status, "active");
    }
}
