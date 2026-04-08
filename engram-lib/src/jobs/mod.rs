// JOBS - Durable job queue with retries (ported from TS jobs/index.ts + scheduler.ts)
use crate::Result;
use libsql::Connection;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, warn};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum JobStatus { Pending, Running, Completed, Failed }
impl JobStatus {
    pub fn as_str(&self) -> &'static str { match self { Self::Pending => "pending", Self::Running => "running", Self::Completed => "completed", Self::Failed => "failed" } }
    pub fn from_str_loose(s: &str) -> Self { match s { "running" => Self::Running, "completed" => Self::Completed, "failed" => Self::Failed, _ => Self::Pending } }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job { pub id: i64, pub job_type: String, pub payload: String, pub status: JobStatus, pub attempts: i32, pub max_attempts: i32, pub error: Option<String>, pub created_at: String, pub claimed_at: Option<String>, pub completed_at: Option<String>, pub next_retry_at: Option<String> }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JobStats { pub pending: i64, pub running: i64, pub completed: i64, pub failed: i64 }

pub async fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS jobs (id INTEGER PRIMARY KEY AUTOINCREMENT, type TEXT NOT NULL, payload TEXT NOT NULL DEFAULT '{}', status TEXT NOT NULL DEFAULT 'pending', attempts INTEGER NOT NULL DEFAULT 0, max_attempts INTEGER NOT NULL DEFAULT 3, error TEXT, created_at TEXT NOT NULL DEFAULT (datetime('now')), claimed_at TEXT, completed_at TEXT, next_retry_at TEXT); CREATE INDEX IF NOT EXISTS idx_jobs_status ON jobs(status, next_retry_at); CREATE INDEX IF NOT EXISTS idx_jobs_type ON jobs(type, status); CREATE TABLE IF NOT EXISTS scheduler_leases (job_name TEXT PRIMARY KEY, holder_id TEXT NOT NULL, acquired_at TEXT NOT NULL DEFAULT (datetime('now')), expires_at TEXT NOT NULL, last_run_at TEXT);").await?;
    Ok(())
}

pub async fn enqueue_job(conn: &Connection, job_type: &str, payload: &str, max_attempts: i32) -> Result<i64> {
    let mut rows = conn.query("INSERT INTO jobs (type, payload, max_attempts) VALUES (?1, ?2, ?3) RETURNING id", libsql::params![job_type.to_string(), payload.to_string(), max_attempts]).await?;
    let row = rows.next().await?.ok_or_else(|| crate::EngError::Internal("no id returned".into()))?;
    Ok(row.get::<i64>(0)?)
}

pub async fn claim_next_job(conn: &Connection) -> Result<Option<Job>> {
    let mut rows = conn.query("SELECT id, type, payload, attempts, max_attempts FROM jobs WHERE status = 'pending' AND (next_retry_at IS NULL OR next_retry_at <= datetime('now')) ORDER BY created_at ASC LIMIT 1", ()).await?;
    let row = match rows.next().await? { Some(r) => r, None => return Ok(None) };
    let id: i64 = row.get(0)?; let jt: String = row.get(1)?; let pl: String = row.get(2)?; let att: i32 = row.get(3)?; let ma: i32 = row.get(4)?;
    conn.execute("UPDATE jobs SET status = 'running', claimed_at = datetime('now'), attempts = attempts + 1 WHERE id = ?1 AND status = 'pending'", libsql::params![id]).await?;
    Ok(Some(Job { id, job_type: jt, payload: pl, status: JobStatus::Running, attempts: att + 1, max_attempts: ma, error: None, created_at: String::new(), claimed_at: None, completed_at: None, next_retry_at: None }))
}

pub async fn complete_job(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("UPDATE jobs SET status = 'completed', completed_at = datetime('now'), error = NULL WHERE id = ?1", libsql::params![id]).await?;
    debug!(job_id = id, "job completed"); Ok(())
}

pub async fn fail_job(conn: &Connection, id: i64, err_msg: &str) -> Result<()> {
    conn.execute("UPDATE jobs SET status = 'failed', error = ?1, completed_at = datetime('now') WHERE id = ?2", libsql::params![err_msg.to_string(), id]).await?;
    error!(job_id = id, error = err_msg, "job failed permanently"); Ok(())
}

pub async fn retry_job(conn: &Connection, id: i64, err_msg: &str, delay_sec: i64) -> Result<()> {
    let sql = format!("UPDATE jobs SET status = 'pending', error = ?1, next_retry_at = datetime('now', '+{} seconds') WHERE id = ?2", delay_sec);
    conn.execute(&sql, libsql::params![err_msg.to_string(), id]).await?;
    warn!(job_id = id, retry_in = delay_sec, "job scheduled for retry"); Ok(())
}

pub async fn get_job_stats(conn: &Connection) -> Result<JobStats> {
    let mut stats = JobStats::default();
    let mut rows = conn.query("SELECT status, COUNT(*) as count FROM jobs GROUP BY status", ()).await?;
    while let Some(row) = rows.next().await? { let s: String = row.get(0)?; let n: i64 = row.get(1)?; match s.as_str() { "pending" => stats.pending = n, "running" => stats.running = n, "completed" => stats.completed = n, "failed" => stats.failed = n, _ => {} } }
    Ok(stats)
}

pub async fn cleanup_completed_jobs(conn: &Connection) -> Result<u64> {
    Ok(conn.execute("DELETE FROM jobs WHERE id IN (SELECT id FROM jobs WHERE status = 'completed' AND completed_at < datetime('now', '-1 hour') LIMIT 100)", ()).await?)
}

pub async fn recover_stuck_jobs(conn: &Connection) -> Result<u64> {
    Ok(conn.execute("UPDATE jobs SET status = 'pending', claimed_at = NULL WHERE status = 'running' AND claimed_at < datetime('now', '-5 minutes')", ()).await?)
}

pub async fn list_failed_jobs(conn: &Connection, limit: i64, offset: i64) -> Result<Vec<Job>> {
    let mut rows = conn.query("SELECT id, type, payload, attempts, max_attempts, error, created_at, completed_at FROM jobs WHERE status = 'failed' ORDER BY completed_at DESC LIMIT ?1 OFFSET ?2", libsql::params![limit, offset]).await?;
    let mut jobs = Vec::new();
    while let Some(r) = rows.next().await? { jobs.push(Job { id: r.get(0)?, job_type: r.get(1)?, payload: r.get(2)?, status: JobStatus::Failed, attempts: r.get(3)?, max_attempts: r.get(4)?, error: r.get(5)?, created_at: r.get::<String>(6).unwrap_or_default(), claimed_at: None, completed_at: r.get(7)?, next_retry_at: None }); }
    Ok(jobs)
}

pub async fn retry_failed_job(conn: &Connection, id: i64) -> Result<bool> {
    let n = conn.execute("UPDATE jobs SET status = 'pending', error = NULL, attempts = 0, next_retry_at = NULL WHERE id = ?1 AND status = 'failed'", libsql::params![id]).await?;
    Ok(n > 0)
}

pub async fn purge_failed_jobs(conn: &Connection, older_than_days: i64) -> Result<u64> {
    let sql = format!("DELETE FROM jobs WHERE status = 'failed' AND completed_at < datetime('now', '-{} days')", older_than_days);
    Ok(conn.execute(&sql, ()).await?)
}

// -- Scheduler leases (ported from TS jobs/scheduler.ts) --
pub async fn acquire_lease(conn: &Connection, job_name: &str, holder_id: &str, ttl_sec: i64) -> Result<bool> {
    let sql = format!("INSERT INTO scheduler_leases (job_name, holder_id, expires_at) VALUES (?1, ?2, datetime('now', '+{0} seconds')) ON CONFLICT(job_name) DO UPDATE SET holder_id = ?2, acquired_at = datetime('now'), expires_at = datetime('now', '+{0} seconds') WHERE expires_at < datetime('now') OR holder_id = ?2", ttl_sec);
    let n = conn.execute(&sql, libsql::params![job_name.to_string(), holder_id.to_string()]).await?;
    Ok(n > 0)
}

pub async fn release_lease(conn: &Connection, job_name: &str, holder_id: &str) -> Result<()> {
    conn.execute("DELETE FROM scheduler_leases WHERE job_name = ?1 AND holder_id = ?2", libsql::params![job_name.to_string(), holder_id.to_string()]).await?;
    Ok(())
}

pub async fn touch_lease(conn: &Connection, job_name: &str, holder_id: &str, ttl_sec: i64) -> Result<bool> {
    let sql = format!("UPDATE scheduler_leases SET expires_at = datetime('now', '+{} seconds'), last_run_at = datetime('now') WHERE job_name = ?1 AND holder_id = ?2", ttl_sec);
    let n = conn.execute(&sql, libsql::params![job_name.to_string(), holder_id.to_string()]).await?;
    Ok(n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn test_job_status_roundtrip() {
        assert_eq!(JobStatus::from_str_loose("pending"), JobStatus::Pending);
        assert_eq!(JobStatus::from_str_loose("running"), JobStatus::Running);
        assert_eq!(JobStatus::from_str_loose("failed"), JobStatus::Failed);
        assert_eq!(JobStatus::Pending.as_str(), "pending");
    }
    #[test] fn test_job_stats_default() { let s = JobStats::default(); assert_eq!(s.pending, 0); }
}
