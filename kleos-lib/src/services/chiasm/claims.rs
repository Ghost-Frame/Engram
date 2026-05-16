//! Path-based resource claims for Chiasm task coordination.
//!
//! Agents claim file paths while working on them. Claims have a TTL
//! (default 30 minutes) and can be checked for conflicts before new
//! claims are created. Heartbeats refresh claim expiry.

use crate::db::Database;
use crate::{EngError, Result};
use serde::{Deserialize, Serialize};

/// A single path claim held by an agent for a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathClaim {
    /// Unique claim identifier.
    pub id: i64,
    /// The task that owns this claim.
    pub task_id: i64,
    /// Agent that created this claim.
    pub agent: String,
    /// Project the claimed path belongs to.
    pub project: String,
    /// File path being claimed.
    pub path: String,
    /// When the claim was created.
    pub claimed_at: String,
    /// When the claim expires (datetime string).
    pub expires_at: String,
    /// Whether the claim has been explicitly released.
    pub released: bool,
}

/// Describes a conflict between a requested path and an existing active claim.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathConflict {
    /// The path that is already claimed.
    pub path: String,
    /// Agent holding the conflicting claim.
    pub claimed_by_agent: String,
    /// Task holding the conflicting claim.
    pub claimed_by_task: i64,
    /// When the conflicting claim expires.
    pub expires_at: String,
}

/// Map a rusqlite error to the crate's EngError type.
fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

/// Read a `PathClaim` from an indexed rusqlite row.
///
/// Column order: id, task_id, agent, project, path, claimed_at, expires_at, released
fn row_to_claim(row: &rusqlite::Row<'_>) -> Result<PathClaim> {
    Ok(PathClaim {
        id: row.get(0).map_err(rusqlite_to_eng_error)?,
        task_id: row.get(1).map_err(rusqlite_to_eng_error)?,
        agent: row.get(2).map_err(rusqlite_to_eng_error)?,
        project: row.get(3).map_err(rusqlite_to_eng_error)?,
        path: row.get(4).map_err(rusqlite_to_eng_error)?,
        claimed_at: row.get(5).map_err(rusqlite_to_eng_error)?,
        expires_at: row.get(6).map_err(rusqlite_to_eng_error)?,
        released: row.get::<_, i64>(7).map_err(rusqlite_to_eng_error)? != 0,
    })
}

/// Fetch a single claim by its ID.
fn get_claim(conn: &rusqlite::Connection, id: i64) -> Result<PathClaim> {
    let mut stmt = conn
        .prepare(
            "SELECT id, task_id, agent, project, path, claimed_at, expires_at, released \
             FROM chiasm_path_claims WHERE id = ?1",
        )
        .map_err(rusqlite_to_eng_error)?;
    let mut rows = stmt
        .query(rusqlite::params![id])
        .map_err(rusqlite_to_eng_error)?;
    let row = rows
        .next()
        .map_err(rusqlite_to_eng_error)?
        .ok_or_else(|| EngError::NotFound(format!("path claim {}", id)))?;
    row_to_claim(row)
}

/// Create path claims for a task.
///
/// Inserts one row per path in `paths`. The `expires_at` is computed as
/// `datetime('now', '+N seconds')` using the provided `ttl_seconds`.
/// Returns all newly created claims.
pub async fn create_claims(
    db: &Database,
    task_id: i64,
    agent: &str,
    project: &str,
    paths: &[&str],
    ttl_seconds: i64,
) -> Result<Vec<PathClaim>> {
    let agent_s = agent.to_string();
    let project_s = project.to_string();
    let paths_owned: Vec<String> = paths.iter().map(|s| s.to_string()).collect();

    let ids: Vec<i64> = db
        .write(move |conn| {
            let mut ids = Vec::with_capacity(paths_owned.len());
            for path in &paths_owned {
                conn.execute(
                    "INSERT INTO chiasm_path_claims \
                     (task_id, agent, project, path, expires_at) \
                     VALUES (?1, ?2, ?3, ?4, datetime('now', ?5))",
                    rusqlite::params![
                        task_id,
                        agent_s,
                        project_s,
                        path,
                        format!("+{} seconds", ttl_seconds)
                    ],
                )
                .map_err(rusqlite_to_eng_error)?;
                ids.push(conn.last_insert_rowid());
            }
            Ok(ids)
        })
        .await?;

    let claims = db
        .read(move |conn| {
            let mut claims = Vec::with_capacity(ids.len());
            for id in &ids {
                claims.push(get_claim(conn, *id)?);
            }
            Ok(claims)
        })
        .await?;
    super::emit_chiasm_event(
        db,
        "claim.created",
        serde_json::json!({
            "task_id": task_id,
            "count": claims.len(),
        }),
    )
    .await;
    Ok(claims)
}

/// Check for conflicting active claims on the given paths within a project.
///
/// A claim conflicts when it is not released (`released = 0`) and has not
/// expired (`expires_at > datetime('now')`). Pass `exclude_task_id` to skip
/// claims belonging to a particular task (useful when a task wants to
/// re-claim its own paths without self-blocking).
pub async fn check_conflicts(
    db: &Database,
    project: &str,
    paths: &[&str],
    exclude_task_id: Option<i64>,
) -> Result<Vec<PathConflict>> {
    let project_s = project.to_string();
    let paths_owned: Vec<String> = paths.iter().map(|s| s.to_string()).collect();

    db.read(move |conn| {
        let mut conflicts = Vec::new();
        for path in &paths_owned {
            let mut sql = String::from(
                "SELECT agent, task_id, expires_at \
                 FROM chiasm_path_claims \
                 WHERE project = ?1 \
                   AND path = ?2 \
                   AND released = 0 \
                   AND expires_at > datetime('now')",
            );
            if exclude_task_id.is_some() {
                sql.push_str(" AND task_id != ?3");
            }

            let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;

            let rows_result: Result<Vec<PathConflict>> = if let Some(excl) = exclude_task_id {
                let mut rows = stmt
                    .query(rusqlite::params![project_s, path, excl])
                    .map_err(rusqlite_to_eng_error)?;
                let mut out = Vec::new();
                while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                    out.push(PathConflict {
                        path: path.clone(),
                        claimed_by_agent: row.get(0).map_err(rusqlite_to_eng_error)?,
                        claimed_by_task: row.get(1).map_err(rusqlite_to_eng_error)?,
                        expires_at: row.get(2).map_err(rusqlite_to_eng_error)?,
                    });
                }
                Ok(out)
            } else {
                let mut rows = stmt
                    .query(rusqlite::params![project_s, path])
                    .map_err(rusqlite_to_eng_error)?;
                let mut out = Vec::new();
                while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                    out.push(PathConflict {
                        path: path.clone(),
                        claimed_by_agent: row.get(0).map_err(rusqlite_to_eng_error)?,
                        claimed_by_task: row.get(1).map_err(rusqlite_to_eng_error)?,
                        expires_at: row.get(2).map_err(rusqlite_to_eng_error)?,
                    });
                }
                Ok(out)
            };

            conflicts.extend(rows_result?);
        }
        Ok(conflicts)
    })
    .await
}

/// List all active (non-released, non-expired) claims for a task.
pub async fn get_claims_for_task(db: &Database, task_id: i64) -> Result<Vec<PathClaim>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, task_id, agent, project, path, claimed_at, expires_at, released \
                 FROM chiasm_path_claims \
                 WHERE task_id = ?1 \
                   AND released = 0 \
                   AND expires_at > datetime('now') \
                 ORDER BY id ASC",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![task_id])
            .map_err(rusqlite_to_eng_error)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            out.push(row_to_claim(row)?);
        }
        Ok(out)
    })
    .await
}

/// List all active (non-released, non-expired) claims in a project.
pub async fn get_claims_for_project(db: &Database, project: &str) -> Result<Vec<PathClaim>> {
    let project_s = project.to_string();
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, task_id, agent, project, path, claimed_at, expires_at, released \
                 FROM chiasm_path_claims \
                 WHERE project = ?1 \
                   AND released = 0 \
                   AND expires_at > datetime('now') \
                 ORDER BY id ASC",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![project_s])
            .map_err(rusqlite_to_eng_error)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            out.push(row_to_claim(row)?);
        }
        Ok(out)
    })
    .await
}

/// Release all claims for a task by setting `released = 1`.
///
/// Returns the number of claims that were updated.
pub async fn release_claims(db: &Database, task_id: i64) -> Result<usize> {
    let count = db
        .write(move |conn| {
            let count = conn
                .execute(
                    "UPDATE chiasm_path_claims SET released = 1 WHERE task_id = ?1",
                    rusqlite::params![task_id],
                )
                .map_err(rusqlite_to_eng_error)?;
            Ok(count)
        })
        .await?;
    super::emit_chiasm_event(
        db,
        "claim.released",
        serde_json::json!({"task_id": task_id}),
    )
    .await;
    Ok(count)
}

/// Unit tests.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::chiasm::tasks::{create_task, CreateTaskRequest};

    /// Minimal task request for test setup.
    fn test_task(title: &str) -> CreateTaskRequest {
        CreateTaskRequest {
            agent: "test-agent".into(),
            project: "test-project".into(),
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

    /// Create two claims and verify they are returned by `get_claims_for_task`.
    #[tokio::test]
    async fn create_and_list_claims() {
        let db = Database::connect_memory().await.expect("db");
        let task = create_task(&db, test_task("list-claims")).await.unwrap();

        let claims = create_claims(
            &db,
            task.id,
            "test-agent",
            "test-project",
            &["a.rs", "b.rs"],
            1800,
        )
        .await
        .unwrap();
        assert_eq!(claims.len(), 2);

        let listed = get_claims_for_task(&db, task.id).await.unwrap();
        assert_eq!(listed.len(), 2);
        let paths: Vec<&str> = listed.iter().map(|c| c.path.as_str()).collect();
        assert!(paths.contains(&"a.rs"));
        assert!(paths.contains(&"b.rs"));
    }

    /// Claims held by task A conflict when task B tries to claim the same path.
    #[tokio::test]
    async fn conflict_detection() {
        let db = Database::connect_memory().await.expect("db");
        let task_a = create_task(&db, test_task("owner")).await.unwrap();
        let task_b = create_task(&db, test_task("requester")).await.unwrap();

        create_claims(&db, task_a.id, "agent-a", "proj", &["src/lib.rs"], 1800)
            .await
            .unwrap();

        let conflicts = check_conflicts(&db, "proj", &["src/lib.rs"], Some(task_b.id))
            .await
            .unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].path, "src/lib.rs");
        assert_eq!(conflicts[0].claimed_by_task, task_a.id);
    }

    /// After releasing claims for a task, no conflicts are reported.
    #[tokio::test]
    async fn release_claims_test() {
        let db = Database::connect_memory().await.expect("db");
        let task_a = create_task(&db, test_task("holder")).await.unwrap();
        let task_b = create_task(&db, test_task("waiter")).await.unwrap();

        create_claims(&db, task_a.id, "agent-a", "proj", &["main.rs"], 1800)
            .await
            .unwrap();

        let released = release_claims(&db, task_a.id).await.unwrap();
        assert_eq!(released, 1);

        let conflicts = check_conflicts(&db, "proj", &["main.rs"], Some(task_b.id))
            .await
            .unwrap();
        assert!(conflicts.is_empty(), "no conflicts after release");
    }

    /// Claims with a TTL of 0 seconds (or already expired) do not conflict.
    #[tokio::test]
    async fn expired_claims_not_conflicting() {
        let db = Database::connect_memory().await.expect("db");
        let task_a = create_task(&db, test_task("expired-holder")).await.unwrap();
        let task_b = create_task(&db, test_task("new-requester")).await.unwrap();

        // TTL of 0 means expires_at = datetime('now', '+0 seconds') which is
        // immediately expired (or at best same-second -- but the check uses
        // `expires_at > datetime('now')` so it will not conflict).
        create_claims(&db, task_a.id, "agent-a", "proj", &["old.rs"], 0)
            .await
            .unwrap();

        let conflicts = check_conflicts(&db, "proj", &["old.rs"], Some(task_b.id))
            .await
            .unwrap();
        assert!(conflicts.is_empty(), "expired claim should not conflict");
    }
}
