pub mod pagerank_refresh;
#[cfg(feature = "tenant-sharding")]
pub mod pagerank_refresh_tenant;

// JOBS - Durable job queue with retries (ported from TS jobs/index.ts + scheduler.ts)
use crate::db::Database;
use crate::Result;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use tokio::sync::RwLock;
use tracing::{debug, error, warn};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum JobStatus {
    Pending,
    Running,
    Completed,
    Failed,
}
impl JobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
    pub fn from_str_loose(s: &str) -> Self {
        match s {
            "running" => Self::Running,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            _ => Self::Pending,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: i64,
    pub job_type: String,
    pub payload: String,
    pub status: JobStatus,
    pub attempts: i32,
    pub max_attempts: i32,
    pub error: Option<String>,
    pub created_at: String,
    pub claimed_at: Option<String>,
    pub completed_at: Option<String>,
    pub next_retry_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JobStats {
    pub pending: i64,
    pub running: i64,
    pub completed: i64,
    pub failed: i64,
}

type JobFuture = Pin<Box<dyn Future<Output = Result<()>> + Send>>;
type JobHandler = Arc<dyn Fn(Value) -> JobFuture + Send + Sync>;

fn handlers() -> &'static RwLock<HashMap<String, JobHandler>> {
    static HANDLERS: OnceLock<RwLock<HashMap<String, JobHandler>>> = OnceLock::new();
    HANDLERS.get_or_init(|| RwLock::new(HashMap::new()))
}

pub async fn ensure_schema(db: &Database) -> Result<()> {
    db.write(|conn| {
        conn.execute_batch("CREATE TABLE IF NOT EXISTS jobs (id INTEGER PRIMARY KEY AUTOINCREMENT, type TEXT NOT NULL, payload TEXT NOT NULL DEFAULT '{}', status TEXT NOT NULL DEFAULT 'pending', attempts INTEGER NOT NULL DEFAULT 0, max_attempts INTEGER NOT NULL DEFAULT 3, error TEXT, created_at TEXT NOT NULL DEFAULT (datetime('now')), claimed_at TEXT, completed_at TEXT, next_retry_at TEXT); CREATE INDEX IF NOT EXISTS idx_jobs_status ON jobs(status, next_retry_at); CREATE INDEX IF NOT EXISTS idx_jobs_type ON jobs(type, status); CREATE TABLE IF NOT EXISTS scheduler_leases (job_name TEXT PRIMARY KEY, holder_id TEXT NOT NULL, acquired_at TEXT NOT NULL DEFAULT (datetime('now')), expires_at TEXT NOT NULL, last_run_at TEXT);")
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        Ok(())
    })
    .await
}

pub async fn enqueue_job(
    db: &Database,
    job_type: &str,
    payload: &str,
    max_attempts: i32,
) -> Result<i64> {
    let job_type = job_type.to_string();
    let payload = payload.to_string();
    db.write(move |conn| {
        conn.execute(
            "INSERT INTO jobs (type, payload, max_attempts) VALUES (?1, ?2, ?3)",
            params![job_type, payload, max_attempts],
        )
        .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        Ok(conn.last_insert_rowid())
    })
    .await
}

pub async fn claim_next_job(db: &Database) -> Result<Option<Job>> {
    // Atomic claim using a transaction: SELECT then UPDATE within a transaction
    // ensures only one worker can claim the same pending job.
    db.write(|conn| {
        let tx = conn
            .transaction()
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;

        let result: Option<Job> = {
            let mut stmt = tx
                .prepare(
                    "SELECT id, type, payload, attempts, max_attempts, created_at, next_retry_at \
                     FROM jobs \
                     WHERE status = 'pending' \
                       AND (next_retry_at IS NULL OR next_retry_at <= datetime('now')) \
                     ORDER BY created_at ASC \
                     LIMIT 1",
                )
                .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;

            let row = stmt
                .query_row([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i32>(3)?,
                        row.get::<_, i32>(4)?,
                        row.get::<_, String>(5).unwrap_or_default(),
                        row.get::<_, Option<String>>(6)?,
                    ))
                });

            match row {
                Ok((id, jt, pl, att, ma, created_at, next_retry_at)) => {
                    tx.execute(
                        "UPDATE jobs SET status = 'running', claimed_at = datetime('now'), attempts = attempts + 1 WHERE id = ?1",
                        params![id],
                    )
                    .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
                    Some(Job {
                        id,
                        job_type: jt,
                        payload: pl,
                        status: JobStatus::Running,
                        attempts: att + 1,
                        max_attempts: ma,
                        error: None,
                        created_at,
                        claimed_at: Some(chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()),
                        completed_at: None,
                        next_retry_at,
                    })
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => None,
                Err(e) => return Err(crate::EngError::DatabaseMessage(e.to_string())),
            }
        };

        tx.commit()
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        Ok(result)
    })
    .await
}

pub async fn complete_job(db: &Database, id: i64) -> Result<()> {
    db.write(move |conn| {
        conn.execute(
            "UPDATE jobs SET status = 'completed', completed_at = datetime('now'), error = NULL WHERE id = ?1",
            params![id],
        )
        .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        Ok(())
    })
    .await?;
    debug!(job_id = id, "job completed");
    Ok(())
}

pub async fn fail_job(db: &Database, id: i64, err_msg: &str) -> Result<()> {
    let err_msg = err_msg.to_string();
    db.write(move |conn| {
        conn.execute(
            "UPDATE jobs SET status = 'failed', error = ?1, completed_at = datetime('now') WHERE id = ?2",
            params![err_msg, id],
        )
        .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        Ok(())
    })
    .await?;
    error!(job_id = id, "job failed permanently");
    Ok(())
}

pub async fn retry_job(db: &Database, id: i64, err_msg: &str, delay_sec: i64) -> Result<()> {
    let err_msg = err_msg.to_string();
    let sql = format!(
        "UPDATE jobs SET status = 'pending', error = ?1, next_retry_at = datetime('now', '+{} seconds') WHERE id = ?2",
        delay_sec
    );
    db.write(move |conn| {
        conn.execute(&sql, params![err_msg, id])
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        Ok(())
    })
    .await?;
    warn!(job_id = id, retry_in = delay_sec, "job scheduled for retry");
    Ok(())
}

pub async fn get_job_stats(db: &Database) -> Result<JobStats> {
    db.read(|conn| {
        let mut stmt = conn
            .prepare("SELECT status, COUNT(*) as count FROM jobs GROUP BY status")
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        let mut stats = JobStats::default();
        let mut rows = stmt
            .query([])
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        while let Some(row) = rows.next().map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))? {
            let s: String = row.get(0).map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
            let n: i64 = row.get(1).map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
            match s.as_str() {
                "pending" => stats.pending = n,
                "running" => stats.running = n,
                "completed" => stats.completed = n,
                "failed" => stats.failed = n,
                _ => {}
            }
        }
        Ok(stats)
    })
    .await
}

pub async fn cleanup_completed_jobs(db: &Database) -> Result<u64> {
    db.write(|conn| {
        let n = conn
            .execute(
                "DELETE FROM jobs WHERE id IN (SELECT id FROM jobs WHERE status = 'completed' AND completed_at < datetime('now', '-1 hour') LIMIT 100)",
                [],
            )
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        Ok(n as u64)
    })
    .await
}

/// Delete completed jobs older than the specified number of days.
/// Returns the count of deleted jobs.
pub async fn cleanup_jobs(db: &Database, older_than_days: i64) -> Result<u64> {
    // Clamp to non-negative to avoid deleting future jobs
    let days = older_than_days.max(0);
    let modifier = format!("-{} days", days);
    db.write(move |conn| {
        let n = conn
            .execute(
                "DELETE FROM jobs WHERE status = 'completed' AND completed_at < datetime('now', ?1)",
                params![modifier],
            )
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        Ok(n as u64)
    })
    .await
}

pub async fn recover_stuck_jobs(db: &Database) -> Result<u64> {
    db.write(|conn| {
        let n = conn
            .execute(
                "UPDATE jobs SET status = 'pending', claimed_at = NULL WHERE status = 'running' AND claimed_at < datetime('now', '-5 minutes')",
                [],
            )
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        Ok(n as u64)
    })
    .await
}

pub async fn list_failed_jobs(db: &Database, limit: i64, offset: i64) -> Result<Vec<Job>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, type, payload, attempts, max_attempts, error, created_at, completed_at \
                 FROM jobs WHERE status = 'failed' ORDER BY completed_at DESC LIMIT ?1 OFFSET ?2",
            )
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        let mut rows = stmt
            .query(params![limit, offset])
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        let mut jobs = Vec::new();
        while let Some(r) = rows.next().map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))? {
            jobs.push(Job {
                id: r.get(0).map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?,
                job_type: r.get(1).map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?,
                payload: r.get(2).map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?,
                status: JobStatus::Failed,
                attempts: r.get(3).map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?,
                max_attempts: r.get(4).map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?,
                error: r.get(5).map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?,
                created_at: r.get::<_, String>(6).unwrap_or_default(),
                claimed_at: None,
                completed_at: r.get(7).map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?,
                next_retry_at: None,
            });
        }
        Ok(jobs)
    })
    .await
}

pub async fn list_pending_jobs(db: &Database, limit: i64, offset: i64) -> Result<Vec<Job>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, type, payload, attempts, max_attempts, created_at, next_retry_at \
                 FROM jobs WHERE status = 'pending' ORDER BY created_at ASC LIMIT ?1 OFFSET ?2",
            )
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        let mut rows = stmt
            .query(params![limit, offset])
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        let mut jobs = Vec::new();
        while let Some(r) = rows.next().map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))? {
            jobs.push(Job {
                id: r.get(0).map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?,
                job_type: r.get(1).map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?,
                payload: r.get(2).map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?,
                status: JobStatus::Pending,
                attempts: r.get(3).map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?,
                max_attempts: r.get(4).map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?,
                error: None,
                created_at: r.get::<_, String>(5).unwrap_or_default(),
                claimed_at: None,
                completed_at: None,
                next_retry_at: r.get(6).map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?,
            });
        }
        Ok(jobs)
    })
    .await
}

pub async fn list_running_jobs(db: &Database) -> Result<Vec<Job>> {
    db.read(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, type, payload, attempts, max_attempts, created_at, claimed_at \
                 FROM jobs WHERE status = 'running' ORDER BY claimed_at ASC",
            )
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        let mut jobs = Vec::new();
        while let Some(r) = rows.next().map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))? {
            jobs.push(Job {
                id: r.get(0).map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?,
                job_type: r.get(1).map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?,
                payload: r.get(2).map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?,
                status: JobStatus::Running,
                attempts: r.get(3).map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?,
                max_attempts: r.get(4).map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?,
                error: None,
                created_at: r.get::<_, String>(5).unwrap_or_default(),
                claimed_at: r.get(6).map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?,
                completed_at: None,
                next_retry_at: None,
            });
        }
        Ok(jobs)
    })
    .await
}

pub async fn count_failed_jobs(db: &Database) -> Result<i64> {
    db.read(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM jobs WHERE status = 'failed'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))
    })
    .await
}

pub async fn retry_failed_job(db: &Database, id: i64) -> Result<bool> {
    db.write(move |conn| {
        let n = conn
            .execute(
                "UPDATE jobs SET status = 'pending', error = NULL, attempts = 0, next_retry_at = NULL WHERE id = ?1 AND status = 'failed'",
                params![id],
            )
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        Ok(n > 0)
    })
    .await
}

pub async fn purge_failed_jobs(db: &Database, older_than_days: i64) -> Result<u64> {
    // Reject negatives defensively so we never expand the purge window to a
    // future timestamp and mass-delete completed jobs.
    let days = older_than_days.max(0);
    let modifier = format!("-{} days", days);
    db.write(move |conn| {
        let n = conn
            .execute(
                "DELETE FROM jobs WHERE status = 'failed' AND completed_at < datetime('now', ?1)",
                params![modifier],
            )
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        Ok(n as u64)
    })
    .await
}

pub async fn register_job_handler<F, Fut>(job_type: &str, handler: F)
where
    F: Fn(Value) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    handlers().write().await.insert(
        job_type.to_string(),
        Arc::new(move |payload| Box::pin(handler(payload))),
    );
}

pub async fn process_next_job(db: &Database) -> Result<bool> {
    let job = match claim_next_job(db).await? {
        Some(job) => job,
        None => return Ok(false),
    };

    let handler = {
        let registry = handlers().read().await;
        registry.get(&job.job_type).cloned()
    };

    let Some(handler) = handler else {
        let err_msg = format!("No handler registered for job type: {}", job.job_type);
        fail_job(db, job.id, &err_msg).await?;
        error!(job_id = job.id, job_type = %job.job_type, "job handler missing");
        return Ok(true);
    };

    let payload: Value = serde_json::from_str(&job.payload)
        .map_err(|e| crate::EngError::InvalidInput(format!("invalid job payload JSON: {}", e)))?;
    let timeout = tokio::time::Duration::from_millis(120_000);

    match tokio::time::timeout(timeout, handler(payload)).await {
        Ok(Ok(())) => {
            complete_job(db, job.id).await?;
            debug!(job_id = job.id, job_type = %job.job_type, attempt = job.attempts, "job completed");
        }
        Ok(Err(err)) => {
            let err_msg = err.to_string();
            if job.attempts >= job.max_attempts {
                fail_job(db, job.id, &err_msg).await?;
            } else {
                let delay_sec = 10_i64 * i64::from(job.attempts) * i64::from(job.attempts);
                retry_job(db, job.id, &err_msg, delay_sec).await?;
            }
        }
        Err(_) => {
            let err_msg = format!("Job timed out after {}ms", timeout.as_millis());
            if job.attempts >= job.max_attempts {
                fail_job(db, job.id, &err_msg).await?;
            } else {
                let delay_sec = 10_i64 * i64::from(job.attempts) * i64::from(job.attempts);
                retry_job(db, job.id, &err_msg, delay_sec).await?;
            }
        }
    }

    Ok(true)
}

pub async fn drain_jobs(db: &Database, limit: usize) -> Result<usize> {
    let mut processed = 0;
    while processed < limit {
        if !process_next_job(db).await? {
            break;
        }
        processed += 1;
    }
    Ok(processed)
}

// -- Scheduler leases (ported from TS jobs/scheduler.ts) --
pub async fn acquire_lease(
    db: &Database,
    job_name: &str,
    holder_id: &str,
    ttl_sec: i64,
) -> Result<bool> {
    let job_name = job_name.to_string();
    let holder_id = holder_id.to_string();
    let sql = format!("INSERT INTO scheduler_leases (job_name, holder_id, expires_at) VALUES (?1, ?2, datetime('now', '+{0} seconds')) ON CONFLICT(job_name) DO UPDATE SET holder_id = ?2, acquired_at = datetime('now'), expires_at = datetime('now', '+{0} seconds') WHERE expires_at < datetime('now') OR holder_id = ?2", ttl_sec);
    db.write(move |conn| {
        let n = conn
            .execute(&sql, params![job_name, holder_id])
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        Ok(n > 0)
    })
    .await
}

pub async fn release_lease(db: &Database, job_name: &str, holder_id: &str) -> Result<()> {
    let job_name = job_name.to_string();
    let holder_id = holder_id.to_string();
    db.write(move |conn| {
        conn.execute(
            "DELETE FROM scheduler_leases WHERE job_name = ?1 AND holder_id = ?2",
            params![job_name, holder_id],
        )
        .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        Ok(())
    })
    .await
}

pub async fn touch_lease(
    db: &Database,
    job_name: &str,
    holder_id: &str,
    ttl_sec: i64,
) -> Result<bool> {
    let job_name = job_name.to_string();
    let holder_id = holder_id.to_string();
    let sql = format!(
        "UPDATE scheduler_leases SET expires_at = datetime('now', '+{} seconds'), last_run_at = datetime('now') WHERE job_name = ?1 AND holder_id = ?2",
        ttl_sec
    );
    db.write(move |conn| {
        let n = conn
            .execute(&sql, params![job_name, holder_id])
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        Ok(n > 0)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_job_status_roundtrip() {
        assert_eq!(JobStatus::from_str_loose("pending"), JobStatus::Pending);
        assert_eq!(JobStatus::from_str_loose("running"), JobStatus::Running);
        assert_eq!(JobStatus::from_str_loose("failed"), JobStatus::Failed);
        assert_eq!(JobStatus::Pending.as_str(), "pending");
    }
    #[test]
    fn test_job_stats_default() {
        let s = JobStats::default();
        assert_eq!(s.pending, 0);
    }
}
