//! Jobs domain -- durable background queue plus recurring schedulers.
//!
//! This module owns the `jobs` table (pending/running/completed/failed rows
//! with retry counts) and exposes:
//! - `enqueue`, `dequeue`, `complete`, `fail` for one-shot work.
//! - A scheduler loop that claims pending rows, runs a registered handler
//!   keyed by `job_type`, and retries with exponential backoff on failure.
//! - [`pagerank_refresh`] -- the canonical recurring job that recomputes
//!   per-user personalized PageRank and writes results back into
//!   `pagerank_cache`.
//! - [`pagerank_refresh_tenant`] (feature `tenant-sharding`) -- per-shard
//!   variant for multi-tenant deployments.
//!
//! Handlers are registered at server startup via [`JobRegistry`]. Silent
//! failures are surfaced via `tracing::warn` / `tracing::error`, never
//! swallowed -- regressions break CI via the swallowed-errors sweep.

pub mod community_detection;
pub mod deprovision;
pub mod disk_sampler;
pub mod pagerank_refresh;
#[cfg(feature = "tenant-sharding")]
pub mod pagerank_refresh_tenant;
pub mod types;
pub use types::*;

// Durable job queue with retries (ported from TS jobs/index.ts + scheduler.ts)
use crate::db::Database;
use crate::Result;
use futures::FutureExt;
use rusqlite::params;
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, error, warn};

/// Boxed future returned by a registered job handler.
type JobFuture = Pin<Box<dyn Future<Output = Result<()>> + Send>>;

/// Shared callable that executes one job payload.
type JobHandler = Arc<dyn Fn(Value) -> JobFuture + Send + Sync>;

/// Return the process-wide registry of job handlers keyed by job type.
fn handlers() -> &'static RwLock<HashMap<String, JobHandler>> {
    static HANDLERS: OnceLock<RwLock<HashMap<String, JobHandler>>> = OnceLock::new();
    HANDLERS.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Return the outer timeout for a job type.
fn job_timeout_for(job_type: &str) -> Duration {
    if job_type == "deprovision_teardown" {
        crate::jobs::deprovision::job_timeout()
    } else {
        Duration::from_millis(120_000)
    }
}

/// Enqueue a pending job and return its row id.
#[tracing::instrument(skip(db, payload), fields(job_type = %job_type))]
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
        )?;
        Ok(conn.last_insert_rowid())
    })
    .await
}

/// Atomically claim the oldest retryable pending job for execution.
#[tracing::instrument(skip(db))]
pub async fn claim_next_job(db: &Database) -> Result<Option<Job>> {
    // Atomic claim using a transaction: SELECT then UPDATE within a transaction
    // ensures only one worker can claim the same pending job.
    db.write(|conn| {
        let tx = conn
            .transaction()?;

        let result: Option<Job> = {
            let mut stmt = tx
                .prepare(
                    "SELECT id, type, payload, attempts, max_attempts, created_at, next_retry_at \
                     FROM jobs \
                     WHERE status = 'pending' \
                       AND (next_retry_at IS NULL OR next_retry_at <= datetime('now')) \
                     ORDER BY created_at ASC \
                     LIMIT 1",
                )?;

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
                    // Read back the stored claimed_at via RETURNING instead of
                    // taking a second clock reading in Rust. Job.claimed_at is
                    // the lease token the finalizers compare against the row
                    // (JOB-2); when the two clock reads straddled a second
                    // boundary, every finalizer for this job silently no-oped
                    // and the job hung at 'running' until recover_stuck_jobs.
                    let claimed_at: String = tx.query_row(
                        "UPDATE jobs SET status = 'running', claimed_at = datetime('now'), attempts = attempts + 1 \
                         WHERE id = ?1 RETURNING claimed_at",
                        params![id],
                        |row| row.get(0),
                    )?;
                    Some(Job {
                        id,
                        job_type: jt,
                        payload: pl,
                        status: JobStatus::Running,
                        attempts: att + 1,
                        max_attempts: ma,
                        error: None,
                        created_at,
                        claimed_at: Some(claimed_at),
                        completed_at: None,
                        next_retry_at,
                    })
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => None,
                Err(e) => return Err(crate::EngError::Database(e)),
            }
        };

        tx.commit()?;
        Ok(result)
    })
    .await
}

/// Mark a job completed and clear its error state.
///
/// `claimed_at` is the lease token the worker observed when it claimed the job
/// (`Job::claimed_at`). The update is gated on `status='running' AND claimed_at=?`
/// so a slow worker that was requeued by `recover_stuck_jobs` and reclaimed by a
/// newer attempt cannot finalize the job after the new attempt took ownership
/// (JOB-2: stale-worker clobber).
#[tracing::instrument(skip(db))]
pub async fn complete_job(db: &Database, id: i64, claimed_at: &str) -> Result<()> {
    let claimed_at = claimed_at.to_string();
    let affected = db
        .write(move |conn| {
            Ok(conn.execute(
                "UPDATE jobs SET status = 'completed', completed_at = datetime('now'), error = NULL \
                 WHERE id = ?1 AND status = 'running' AND claimed_at = ?2",
                params![id, claimed_at],
            )?)
        })
        .await?;
    if affected == 0 {
        warn!(job_id = id, "complete_job: lease lost (job reclaimed by another worker); terminal state not overwritten");
    } else {
        debug!(job_id = id, "job completed");
    }
    Ok(())
}

/// Mark a job permanently failed with the final error message.
///
/// Lease-gated on `claimed_at` for the same reason as [`complete_job`] (JOB-2).
#[tracing::instrument(skip(db, err_msg))]
pub async fn fail_job(db: &Database, id: i64, claimed_at: &str, err_msg: &str) -> Result<()> {
    let err_msg = err_msg.to_string();
    let claimed_at = claimed_at.to_string();
    let affected = db
        .write(move |conn| {
            Ok(conn.execute(
                "UPDATE jobs SET status = 'failed', error = ?1, completed_at = datetime('now') \
                 WHERE id = ?2 AND status = 'running' AND claimed_at = ?3",
                params![err_msg, id, claimed_at],
            )?)
        })
        .await?;
    if affected == 0 {
        warn!(job_id = id, "fail_job: lease lost (job reclaimed by another worker); terminal state not overwritten");
    } else {
        error!(job_id = id, "job failed permanently");
    }
    Ok(())
}

/// Return a running job to pending state after a retry delay.
///
/// Lease-gated on `claimed_at` for the same reason as [`complete_job`] (JOB-2):
/// a stale worker must not requeue a job a newer attempt now owns.
#[tracing::instrument(skip(db, err_msg))]
pub async fn retry_job(
    db: &Database,
    id: i64,
    claimed_at: &str,
    err_msg: &str,
    delay_sec: i64,
) -> Result<()> {
    let err_msg = err_msg.to_string();
    let claimed_at = claimed_at.to_string();
    let modifier = format!("+{} seconds", delay_sec);
    let affected = db
        .write(move |conn| {
            Ok(conn.execute(
                "UPDATE jobs SET status = 'pending', error = ?1, next_retry_at = datetime('now', ?3), claimed_at = NULL \
                 WHERE id = ?2 AND status = 'running' AND claimed_at = ?4",
                params![err_msg, id, modifier, claimed_at],
            )?)
        })
        .await?;
    if affected == 0 {
        warn!(
            job_id = id,
            "retry_job: lease lost (job reclaimed by another worker); not requeuing"
        );
    } else {
        warn!(job_id = id, retry_in = delay_sec, "job scheduled for retry");
    }
    Ok(())
}

/// Count jobs by status for operator and health reporting.
#[tracing::instrument(skip(db))]
pub async fn get_job_stats(db: &Database) -> Result<JobStats> {
    db.read(|conn| {
        let mut stmt =
            conn.prepare("SELECT status, COUNT(*) as count FROM jobs GROUP BY status")?;
        let mut stats = JobStats::default();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let s: String = row.get(0)?;
            let n: i64 = row.get(1)?;
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

/// Delete completed jobs older than one hour, draining in 100-row batches.
#[tracing::instrument(skip(db))]
pub async fn cleanup_completed_jobs(db: &Database) -> Result<u64> {
    db.write(|conn| {
        let mut deleted = 0u64;
        loop {
            let n = conn.execute(
                "DELETE FROM jobs WHERE id IN (SELECT id FROM jobs WHERE status = 'completed' AND completed_at < datetime('now', '-1 hour') LIMIT 100)",
                [],
            )?;
            deleted += n as u64;
            if n == 0 {
                break;
            }
        }
        Ok(deleted)
    })
    .await
}

/// Delete completed jobs older than the specified number of days.
/// Returns the count of deleted jobs.
#[tracing::instrument(skip(db))]
pub async fn cleanup_jobs(db: &Database, older_than_days: i64) -> Result<u64> {
    // Clamp to non-negative to avoid deleting future jobs
    let days = older_than_days.max(0);
    let modifier = format!("-{} days", days);
    db.write(move |conn| {
        let n = conn.execute(
            "DELETE FROM jobs WHERE status = 'completed' AND completed_at < datetime('now', ?1)",
            params![modifier],
        )?;
        Ok(n as u64)
    })
    .await
}

/// Requeue jobs that were claimed but abandoned by a dead worker.
#[tracing::instrument(skip(db))]
pub async fn recover_stuck_jobs(db: &Database) -> Result<u64> {
    db.write(|conn| {
        // Recover a running job only after its OWN type's timeout (plus a grace
        // margin) has elapsed, not a blanket 5 minutes. A long-running type like
        // deprovision_teardown (~30 min) must not be requeued while still
        // executing -- that would run the same destructive teardown twice.
        const GRACE_SECS: u64 = 60;
        let mut stmt = conn.prepare(
            "SELECT id, type, \
             CAST((julianday('now') - julianday(claimed_at)) * 86400 AS INTEGER) \
             FROM jobs WHERE status = 'running' AND claimed_at IS NOT NULL",
        )?;
        let rows: Vec<(i64, String, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<_, _>>()?;
        drop(stmt);

        let mut recovered = 0u64;
        for (id, job_type, elapsed_secs) in rows {
            let lease_secs = job_timeout_for(&job_type).as_secs() + GRACE_SECS;
            if elapsed_secs >= 0 && (elapsed_secs as u64) > lease_secs {
                let n = conn.execute(
                    "UPDATE jobs SET status = 'pending', claimed_at = NULL \
                     WHERE id = ?1 AND status = 'running'",
                    params![id],
                )?;
                recovered += n as u64;
            }
        }
        Ok(recovered)
    })
    .await
}

/// List failed jobs in reverse completion order.
#[tracing::instrument(skip(db))]
pub async fn list_failed_jobs(db: &Database, limit: i64, offset: i64) -> Result<Vec<Job>> {
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT id, type, payload, attempts, max_attempts, error, created_at, completed_at \
                 FROM jobs WHERE status = 'failed' ORDER BY completed_at DESC LIMIT ?1 OFFSET ?2",
        )?;
        let mut rows = stmt.query(params![limit, offset])?;
        let mut jobs = Vec::new();
        while let Some(r) = rows.next()? {
            jobs.push(Job {
                id: r.get(0)?,
                job_type: r.get(1)?,
                payload: r.get(2)?,
                status: JobStatus::Failed,
                attempts: r.get(3)?,
                max_attempts: r.get(4)?,
                error: r.get(5)?,
                created_at: r.get::<_, String>(6).unwrap_or_default(),
                claimed_at: None,
                completed_at: r.get(7)?,
                next_retry_at: None,
            });
        }
        Ok(jobs)
    })
    .await
}

/// List pending jobs in FIFO order.
#[tracing::instrument(skip(db))]
pub async fn list_pending_jobs(db: &Database, limit: i64, offset: i64) -> Result<Vec<Job>> {
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT id, type, payload, attempts, max_attempts, created_at, next_retry_at \
                 FROM jobs WHERE status = 'pending' ORDER BY created_at ASC LIMIT ?1 OFFSET ?2",
        )?;
        let mut rows = stmt.query(params![limit, offset])?;
        let mut jobs = Vec::new();
        while let Some(r) = rows.next()? {
            jobs.push(Job {
                id: r.get(0)?,
                job_type: r.get(1)?,
                payload: r.get(2)?,
                status: JobStatus::Pending,
                attempts: r.get(3)?,
                max_attempts: r.get(4)?,
                error: None,
                created_at: r.get::<_, String>(5).unwrap_or_default(),
                claimed_at: None,
                completed_at: None,
                next_retry_at: r.get(6)?,
            });
        }
        Ok(jobs)
    })
    .await
}

/// List running jobs ordered by claim time.
#[tracing::instrument(skip(db))]
pub async fn list_running_jobs(db: &Database) -> Result<Vec<Job>> {
    db.read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, type, payload, attempts, max_attempts, created_at, claimed_at \
                 FROM jobs WHERE status = 'running' ORDER BY claimed_at ASC",
        )?;
        let mut rows = stmt.query([])?;
        let mut jobs = Vec::new();
        while let Some(r) = rows.next()? {
            jobs.push(Job {
                id: r.get(0)?,
                job_type: r.get(1)?,
                payload: r.get(2)?,
                status: JobStatus::Running,
                attempts: r.get(3)?,
                max_attempts: r.get(4)?,
                error: None,
                created_at: r.get::<_, String>(5).unwrap_or_default(),
                claimed_at: r.get(6)?,
                completed_at: None,
                next_retry_at: None,
            });
        }
        Ok(jobs)
    })
    .await
}

/// Count failed jobs for status summaries.
#[tracing::instrument(skip(db))]
pub async fn count_failed_jobs(db: &Database) -> Result<i64> {
    db.read(|conn| {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM jobs WHERE status = 'failed'",
            [],
            |row| row.get(0),
        )?)
    })
    .await
}

/// Move a failed job back to pending for a manual retry.
#[tracing::instrument(skip(db))]
pub async fn retry_failed_job(db: &Database, id: i64) -> Result<bool> {
    db.write(move |conn| {
        let n = conn
            .execute(
                "UPDATE jobs SET status = 'pending', error = NULL, attempts = 0, next_retry_at = NULL WHERE id = ?1 AND status = 'failed'",
                params![id],
            )?;
        Ok(n > 0)
    })
    .await
}

/// Delete failed jobs older than the requested retention window.
#[tracing::instrument(skip(db))]
pub async fn purge_failed_jobs(db: &Database, older_than_days: i64) -> Result<u64> {
    // Reject negatives defensively so we never expand the purge window to a
    // future timestamp and mass-delete completed jobs.
    let days = older_than_days.max(0);
    let modifier = format!("-{} days", days);
    db.write(move |conn| {
        let n = conn.execute(
            "DELETE FROM jobs WHERE status = 'failed' AND completed_at < datetime('now', ?1)",
            params![modifier],
        )?;
        Ok(n as u64)
    })
    .await
}

/// Register an async handler for a job type.
#[tracing::instrument(skip(handler), fields(job_type = %job_type))]
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

/// Claim and execute one pending job if any are ready.
#[tracing::instrument(skip(db))]
pub async fn process_next_job(db: &Database) -> Result<bool> {
    let job = match claim_next_job(db).await? {
        Some(job) => job,
        None => return Ok(false),
    };

    let handler = {
        let registry = handlers().read().await;
        registry.get(&job.job_type).cloned()
    };

    // Lease token observed at claim time; threaded into every finalizer so a
    // newer attempt's terminal state is never clobbered by this one (JOB-2).
    let claimed_at = job.claimed_at.clone().unwrap_or_default();

    let Some(handler) = handler else {
        // Missing handlers can happen during startup or rolling deploys, so
        // they follow the same retry discipline as handler errors.
        let err_msg = format!("No handler registered for job type: {}", job.job_type);
        if job.attempts >= job.max_attempts {
            fail_job(db, job.id, &claimed_at, &err_msg).await?;
            error!(job_id = job.id, job_type = %job.job_type, "job handler missing -- giving up after max attempts");
        } else {
            let delay_sec = 10_i64 * i64::from(job.attempts) * i64::from(job.attempts);
            retry_job(db, job.id, &claimed_at, &err_msg, delay_sec).await?;
            warn!(job_id = job.id, job_type = %job.job_type, "job handler missing -- scheduled for retry");
        }
        return Ok(true);
    };

    let payload: Value = match serde_json::from_str(&job.payload) {
        Ok(v) => v,
        Err(e) => {
            // A payload that does not parse as JSON is deterministically poison:
            // it will never parse on retry. Retrying with delay 0 just hammers
            // CPU and logs until max_attempts. Fail it permanently now (JOB-3).
            let err_msg = format!("invalid job payload JSON: {}", e);
            fail_job(db, job.id, &claimed_at, &err_msg).await?;
            error!(job_id = job.id, job_type = %job.job_type, "poison payload -- failed permanently");
            return Ok(true);
        }
    };
    let timeout = job_timeout_for(&job.job_type);

    // Contain handler panics. An unwinding handler future would otherwise
    // propagate through process_next_job and kill the whole worker loop, and
    // because no finalizer runs, the row stays 'running' until
    // recover_stuck_jobs requeues it -- a deterministically-panicking payload
    // then loops forever, since the attempts >= max_attempts check lives in
    // exactly the code the panic skips. catch_unwind keeps the future
    // directly owned by tokio::time::timeout, preserving cancel-on-timeout
    // semantics (a tokio::spawn-based variant would leave a timed-out
    // handler running detached).
    let handler_fut = std::panic::AssertUnwindSafe(handler(payload)).catch_unwind();
    match tokio::time::timeout(timeout, handler_fut).await {
        Ok(Ok(Ok(()))) => {
            complete_job(db, job.id, &claimed_at).await?;
            debug!(job_id = job.id, job_type = %job.job_type, attempt = job.attempts, "job completed");
        }
        Ok(Ok(Err(err))) => {
            let err_msg = err.to_string();
            if job.attempts >= job.max_attempts {
                fail_job(db, job.id, &claimed_at, &err_msg).await?;
            } else {
                let delay_sec = 10_i64 * i64::from(job.attempts) * i64::from(job.attempts);
                retry_job(db, job.id, &claimed_at, &err_msg, delay_sec).await?;
            }
        }
        Ok(Err(panic_payload)) => {
            let reason = panic_payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic_payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "opaque panic payload".to_string());
            let err_msg = format!("job handler panicked: {reason}");
            if job.attempts >= job.max_attempts {
                fail_job(db, job.id, &claimed_at, &err_msg).await?;
                error!(job_id = job.id, job_type = %job.job_type, "job handler panicked -- giving up after max attempts");
            } else {
                let delay_sec = 10_i64 * i64::from(job.attempts) * i64::from(job.attempts);
                retry_job(db, job.id, &claimed_at, &err_msg, delay_sec).await?;
                warn!(job_id = job.id, job_type = %job.job_type, "job handler panicked -- scheduled for retry");
            }
        }
        Err(_) => {
            let err_msg = format!("Job timed out after {}ms", timeout.as_millis());
            if job.attempts >= job.max_attempts {
                fail_job(db, job.id, &claimed_at, &err_msg).await?;
            } else {
                let delay_sec = 10_i64 * i64::from(job.attempts) * i64::from(job.attempts);
                retry_job(db, job.id, &claimed_at, &err_msg, delay_sec).await?;
            }
        }
    }

    Ok(true)
}

/// Unit tests for durable jobs.
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Verify loose status parsing and stable status serialization.
    #[test]
    fn test_job_status_roundtrip() {
        assert_eq!(JobStatus::from_str_loose("pending"), JobStatus::Pending);
        assert_eq!(JobStatus::from_str_loose("running"), JobStatus::Running);
        assert_eq!(JobStatus::from_str_loose("failed"), JobStatus::Failed);
        assert_eq!(JobStatus::Pending.as_str(), "pending");
    }

    /// Verify the default stats object starts at zero.
    #[test]
    fn test_job_stats_default() {
        let s = JobStats::default();
        assert_eq!(s.pending, 0);
    }

    /// Verify the deprovision job timeout stays above the generic 120s cap.
    #[serial_test::serial(deprovision_timeout_env)]
    #[test]
    fn job_timeout_for_deprovision_exceeds_generic_timeout() {
        std::env::remove_var("KLEOS_DEPROVISION_JOB_TIMEOUT_SECS");
        assert_eq!(job_timeout_for("generic").as_millis(), 120_000);
        assert!(
            job_timeout_for("deprovision_teardown").as_secs() >= 1800,
            "deprovision teardown should use its long timeout default"
        );
    }

    /// Seed completed jobs with an old timestamp so cleanup can drain them.
    async fn seed_completed_jobs(db: &Database, count: usize) {
        db.write(move |conn| {
            for idx in 0..count {
                conn.execute(
                    "INSERT INTO jobs (type, payload, status, attempts, max_attempts, created_at, completed_at) VALUES (?1, ?2, 'completed', 1, 1, datetime('now', '-2 hours'), datetime('now', '-2 hours'))",
                    params![format!("cleanup.{idx}"), "{}"],
                )?;
            }
            Ok(())
        })
        .await
        .expect("seed completed jobs");
    }

    /// Verify cleanup drains more than one 100-row batch in a single call.
    #[tokio::test]
    async fn cleanup_completed_jobs_drains_multiple_batches() {
        let db = Database::connect_memory().await.expect("in-memory db");
        seed_completed_jobs(&db, 205).await;

        let deleted = cleanup_completed_jobs(&db).await.expect("cleanup");
        let stats = get_job_stats(&db).await.expect("stats");

        assert_eq!(deleted, 205);
        assert_eq!(stats.completed, 0);
    }

    // End-to-end: enqueue a job, register a handler, run the worker once,
    // verify the handler ran and the row is marked completed. This is the
    // proof that the jobs queue is an actually wired pipeline rather than
    // just a table full of pending rows.
    #[tokio::test]
    async fn enqueue_process_next_runs_registered_handler() {
        let db = Database::connect_memory().await.expect("in-memory db");

        static CALLS: AtomicUsize = AtomicUsize::new(0);
        CALLS.store(0, Ordering::SeqCst);

        register_job_handler("test.counter", |payload| async move {
            let delta = payload.get("delta").and_then(|v| v.as_u64()).unwrap_or(0);
            CALLS.fetch_add(delta as usize, Ordering::SeqCst);
            Ok(())
        })
        .await;

        let job_id = enqueue_job(&db, "test.counter", r#"{"delta":3}"#, 3)
            .await
            .expect("enqueue");
        assert!(job_id > 0);

        let processed = process_next_job(&db).await.expect("process");
        assert!(processed, "worker should have claimed the pending job");
        assert_eq!(CALLS.load(Ordering::SeqCst), 3);

        let stats = get_job_stats(&db).await.expect("stats");
        assert_eq!(stats.completed, 1, "job should be marked completed");
        assert_eq!(stats.pending, 0);
        assert_eq!(stats.running, 0);
        assert_eq!(stats.failed, 0);
    }

    // Jobs with temporarily missing handlers should retry rather than fail.
    #[tokio::test]
    async fn missing_handler_retries_instead_of_failing() {
        let db = Database::connect_memory().await.expect("in-memory db");

        let job_id = enqueue_job(&db, "unknown.no_handler_retry", "{}", 2)
            .await
            .expect("enqueue");
        assert!(job_id > 0);

        let processed = process_next_job(&db).await.expect("process");
        assert!(processed, "worker should have claimed the pending job");

        let stats = get_job_stats(&db).await.expect("stats");
        assert_eq!(
            stats.failed, 0,
            "missing handler must not fail permanently while attempts remain"
        );
        assert_eq!(
            stats.pending, 1,
            "missing-handler job should be rescheduled for retry"
        );
        assert_eq!(stats.running, 0);
    }

    // Jobs with permanently missing handlers should fail after max attempts.
    #[tokio::test]
    async fn missing_handler_fails_after_max_attempts() {
        let db = Database::connect_memory().await.expect("in-memory db");

        let job_id = enqueue_job(&db, "unknown.no_handler_terminal", "{}", 1)
            .await
            .expect("enqueue");
        assert!(job_id > 0);

        let processed = process_next_job(&db).await.expect("process");
        assert!(processed, "worker should have claimed the pending job");

        let stats = get_job_stats(&db).await.expect("stats");
        assert_eq!(
            stats.failed, 1,
            "missing handler must fail after exhausting max_attempts"
        );
        assert_eq!(stats.pending, 0);
        assert_eq!(stats.running, 0);
    }

    /// JOB-2: a finalizer must not clobber a job whose lease has changed.
    /// A stale worker holding the old `claimed_at` cannot complete/fail/retry a
    /// row that a newer attempt has reclaimed.
    #[tokio::test]
    async fn finalizers_are_lease_gated_on_claimed_at() {
        let db = Database::connect_memory().await.expect("in-memory db");
        // Insert a running job with a known lease token.
        let id = db
            .write(|conn| {
                conn.execute(
                    "INSERT INTO jobs (type, payload, status, attempts, max_attempts, created_at, claimed_at) \
                     VALUES ('lease.test', '{}', 'running', 1, 3, datetime('now'), 'LEASE_A')",
                    [],
                )?;
                Ok(conn.last_insert_rowid())
            })
            .await
            .expect("seed running job");

        // Wrong lease token -> no-op, job stays running.
        complete_job(&db, id, "LEASE_B")
            .await
            .expect("complete (stale)");
        let stats = get_job_stats(&db).await.expect("stats");
        assert_eq!(stats.running, 1, "stale-lease complete must not finalize");
        assert_eq!(stats.completed, 0);

        // fail_job with the wrong token is equally a no-op.
        fail_job(&db, id, "LEASE_B", "boom")
            .await
            .expect("fail (stale)");
        let stats = get_job_stats(&db).await.expect("stats");
        assert_eq!(stats.running, 1, "stale-lease fail must not finalize");
        assert_eq!(stats.failed, 0);

        // Correct lease token -> completes.
        complete_job(&db, id, "LEASE_A")
            .await
            .expect("complete (owner)");
        let stats = get_job_stats(&db).await.expect("stats");
        assert_eq!(stats.completed, 1, "owner lease must finalize");
        assert_eq!(stats.running, 0);
    }

    /// JOB-3: a payload that does not parse as JSON is deterministically poison
    /// and must fail permanently on the first attempt, not retry in a hot loop
    /// (which would re-enter pending with delay 0 until max_attempts).
    #[tokio::test]
    async fn poison_payload_fails_permanently_not_retried() {
        let db = Database::connect_memory().await.expect("in-memory db");

        register_job_handler("test.poison", |_payload| async move { Ok(()) }).await;

        // max_attempts deliberately high: the old code would have requeued.
        let job_id = enqueue_job(&db, "test.poison", "this is not json", 5)
            .await
            .expect("enqueue");
        assert!(job_id > 0);

        let processed = process_next_job(&db).await.expect("process");
        assert!(processed, "worker should have claimed the poison job");

        let stats = get_job_stats(&db).await.expect("stats");
        assert_eq!(stats.failed, 1, "poison payload must fail permanently");
        assert_eq!(stats.pending, 0, "poison payload must not be requeued");
        assert_eq!(stats.running, 0);
    }

    /// The Job returned by claim_next_job must carry the exact claimed_at the
    /// UPDATE stored. Pre-fix, the struct took a second clock reading in Rust;
    /// when the two reads straddled a second boundary, every lease-gated
    /// finalizer for that job silently no-oped (JOB-2) and the row hung at
    /// 'running' until recover_stuck_jobs.
    #[tokio::test]
    async fn claimed_at_in_struct_matches_stored_row() {
        let db = Database::connect_memory().await.expect("in-memory db");

        let job_id = enqueue_job(&db, "test.lease_token", "{}", 3)
            .await
            .expect("enqueue");

        let job = claim_next_job(&db)
            .await
            .expect("claim")
            .expect("one pending job");
        assert_eq!(job.id, job_id);

        let stored: String = db
            .read(move |conn| {
                Ok(conn.query_row(
                    "SELECT claimed_at FROM jobs WHERE id = ?1",
                    params![job_id],
                    |row| row.get(0),
                )?)
            })
            .await
            .expect("read claimed_at");
        assert_eq!(
            job.claimed_at.as_deref(),
            Some(stored.as_str()),
            "struct lease token must equal the stored value"
        );
    }

    /// A panicking handler must feed the normal retry path instead of
    /// unwinding through process_next_job: first attempt of a two-attempt job
    /// lands back at pending.
    #[tokio::test]
    async fn panicking_handler_is_retried_not_stuck() {
        let db = Database::connect_memory().await.expect("in-memory db");

        register_job_handler("test.panics_retry", |_payload| async move {
            panic!("boom");
        })
        .await;

        enqueue_job(&db, "test.panics_retry", "{}", 2)
            .await
            .expect("enqueue");

        let processed = process_next_job(&db)
            .await
            .expect("panic must not propagate out of process_next_job");
        assert!(processed);

        let stats = get_job_stats(&db).await.expect("stats");
        assert_eq!(stats.pending, 1, "first panic schedules a retry");
        assert_eq!(stats.running, 0, "row must not hang at running");
        assert_eq!(stats.failed, 0);
    }

    /// A panicking handler that has exhausted max_attempts must reach the
    /// terminal failed state. Pre-fix this was impossible: the attempts check
    /// lived in exactly the code the panic skipped, so a deterministically
    /// panicking payload looped forever via recover_stuck_jobs.
    #[tokio::test]
    async fn panicking_handler_fails_after_max_attempts() {
        let db = Database::connect_memory().await.expect("in-memory db");

        register_job_handler("test.panics_final", |_payload| async move {
            panic!("boom");
        })
        .await;

        enqueue_job(&db, "test.panics_final", "{}", 1)
            .await
            .expect("enqueue");

        let processed = process_next_job(&db)
            .await
            .expect("panic must not propagate out of process_next_job");
        assert!(processed);

        let stats = get_job_stats(&db).await.expect("stats");
        assert_eq!(stats.failed, 1, "exhausted attempts must fail terminally");
        assert_eq!(stats.pending, 0);
        assert_eq!(stats.running, 0);

        let error: Option<String> = db
            .read(|conn| {
                Ok(conn.query_row(
                    "SELECT error FROM jobs WHERE type = 'test.panics_final'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await
            .expect("read error");
        assert!(
            error.unwrap_or_default().contains("panicked: boom"),
            "panic reason must be recorded on the row"
        );
    }
}
