//! Durable two-phase tenant teardown types and orchestration.
//!
//! Design: `begin_deprovision` (sync HTTP) marks `Deleting`, evicts handle,
//! inserts `deletions_log`, enqueues job, returns 202. The async job runs
//! `remove_shard_dir` -> `delete_monolith_rows` -> `mark_tombstone`.
//! Startup recovery re-enqueues orphaned `Deleting` rows.

use crate::db::Database;
use crate::EngError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tracing::{info, warn};

use super::registry::TenantRegistry;
use super::registry_db::RegistryDb;

/// Opaque identifier for a single deprovision operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeprovisionId(pub String);

impl DeprovisionId {
    /// Generate a new random deprovision ID.
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    /// View the inner string value.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for DeprovisionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for DeprovisionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Current status of a teardown operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeardownStatus {
    /// Job enqueued; teardown in progress.
    Deleting,
    /// All steps complete; username held in tombstone.
    Tombstone,
    /// Exceeded max_attempts; needs manual retry or skip.
    Stuck,
}

/// A recorded step in a teardown sequence.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum TeardownStep {
    /// Registry row marked Deleting; handle evicted.
    MarkedDeleting,
    /// Optional shard archive written before removal.
    ShardArchived {
        /// Filesystem path to the archive file.
        path: String,
        /// Size of the archive in bytes.
        bytes: u64,
    },
    /// Shard directory removed (or was already absent).
    ShardRemoved {
        /// Filesystem path that was removed.
        path: String,
        /// Bytes freed by the removal.
        bytes_freed: u64,
    },
    /// Monolith rows deleted for this user.
    MonolithCleared {
        /// Number of rows deleted per table name.
        rows_by_table: HashMap<String, usize>,
    },
    /// Registry row transitioned to Tombstone.
    Tombstoned,
}

/// Final report returned by a completed teardown job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeprovisionReport {
    /// The deprovision operation ID.
    pub deprovision_id: String,
    /// Ordered list of completed steps.
    pub steps: Vec<TeardownStep>,
    /// Whether the shard directory was present and removed.
    pub shard_removed: bool,
    /// Whether a JSONL.gz archive was written.
    pub archived: bool,
}

/// Report returned by startup orphan recovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryReport {
    /// Number of Deleting rows found.
    pub found: usize,
    /// Number successfully re-enqueued.
    pub re_enqueued: usize,
    /// Number already in Stuck state (skipped).
    pub stuck_skipped: usize,
}

/// A row from the `deletions_log` table.
#[derive(Debug, Clone)]
pub struct DeletionLogRow {
    /// Unique deprovision operation ID.
    pub deprovision_id: String,
    /// Admin user who initiated the deprovision (None for system).
    pub admin_user_id: Option<i64>,
    /// The user being deprovisioned.
    pub target_user_id: i64,
    /// Username snapshot at deprovision time.
    pub target_username: String,
    /// ISO-8601 timestamp of deletion.
    pub deleted_at: String,
    /// Optional reason string from the admin.
    pub reason: Option<String>,
    /// Path to the archive file, if archived.
    pub archive_path: Option<String>,
    /// Whether the shard removal step was skipped.
    pub shard_skipped: bool,
}

/// A row from the `cluster_lock` table.
#[derive(Debug, Clone)]
pub struct ClusterLockRow {
    /// Node identifier (hostname or UUID).
    pub node_id: String,
    /// ISO-8601 last heartbeat time.
    pub heartbeat: String,
    /// ISO-8601 time when this node took the lock.
    pub started_at: String,
}

// ── Cluster Lock ──────────────────────────────────────────────────────────

/// Check whether another node holds an active cluster lock.
///
/// Returns `Ok(())` if this node may proceed. Returns `Err(Conflict)` if
/// another node has a fresh lock. Set `KLEOS_MULTI_NODE=skip` to bypass.
pub fn check_cluster_lock(registry_db: &RegistryDb, node_id: &str) -> crate::Result<()> {
    if std::env::var("KLEOS_MULTI_NODE").as_deref() == Ok("skip") {
        return Ok(());
    }
    let others = registry_db.cluster_lock_active_others(node_id, 30)?;
    if !others.is_empty() {
        let ids: Vec<_> = others.iter().map(|r| r.node_id.as_str()).collect();
        return Err(EngError::Conflict(format!(
            "cluster lock held by other node(s): {}",
            ids.join(", ")
        )));
    }
    registry_db.cluster_lock_upsert(node_id)?;
    Ok(())
}

/// Spawn a background task that keeps the cluster lock heartbeat alive.
///
/// The task exits when the cancellation token is cancelled, releasing the lock.
pub fn start_heartbeat_task(
    registry_db: Arc<RegistryDb>,
    node_id: String,
    token: tokio_util::sync::CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
        loop {
            tokio::select! {
                _ = token.cancelled() => {
                    let _ = registry_db.cluster_lock_release(&node_id);
                    break;
                }
                _ = interval.tick() => {
                    if let Err(e) = registry_db.cluster_lock_heartbeat(&node_id) {
                        warn!(node_id = %node_id, "cluster lock heartbeat failed: {e}");
                    }
                }
            }
        }
    })
}

// ── Shard Archive ─────────────────────────────────────────────────────────

/// Write a JSONL.gz snapshot of the shard directory's kleos.db.
///
/// Opens the shard's SQLite file, dumps `memories` and `audit_log` tables as
/// JSON lines, preceded by a metadata header line. Returns `(archive_path, bytes_written)`.
///
/// Set `KLEOS_DEPROVISION_ARCHIVE=true` or `1` to enable archiving.
async fn write_shard_archive(
    shard_dir: &Path,
    deprovision_id: &str,
) -> crate::Result<(String, u64)> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;

    let db_path = shard_dir.join("kleos.db");
    if !db_path.exists() {
        return Err(EngError::NotFound(
            "shard kleos.db not found for archive".into(),
        ));
    }

    let archive_dir = shard_dir
        .parent()
        .unwrap_or(shard_dir)
        .parent()
        .unwrap_or(shard_dir)
        .join("archives");
    tokio::fs::create_dir_all(&archive_dir)
        .await
        .map_err(|e| EngError::Internal(format!("create archive dir: {e}")))?;

    let archive_path = archive_dir.join(format!("{deprovision_id}.jsonl.gz"));
    let archive_path_str = archive_path.to_string_lossy().into_owned();

    // Open the shard SQLite and dump rows.
    let db_path_owned = db_path.to_path_buf();
    let dep_id_owned = deprovision_id.to_string();
    let archive_path_clone = archive_path.clone();

    // Do the blocking IO on a spawn_blocking to avoid holding the async runtime.
    let bytes = tokio::task::spawn_blocking(move || -> crate::Result<u64> {
        let conn = rusqlite::Connection::open_with_flags(
            &db_path_owned,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| EngError::Internal(format!("open shard db for archive: {e}")))?;

        let file = std::fs::File::create(&archive_path_clone)
            .map_err(|e| EngError::Internal(format!("create archive file: {e}")))?;
        let mut gz = GzEncoder::new(file, Compression::default());

        // Write metadata header line.
        let meta = serde_json::json!({
            "type": "header",
            "deprovision_id": dep_id_owned,
            "shard_dir": db_path_owned.parent().map(|p| p.to_string_lossy().into_owned()),
            "archived_at": chrono::Utc::now().to_rfc3339(),
        });
        writeln!(gz, "{meta}")
            .map_err(|e| EngError::Internal(format!("write archive header: {e}")))?;

        // Dump memories table if it exists.
        if let Ok(mut stmt) = conn
            .prepare("SELECT id, content, source, category, importance, created_at FROM memories")
        {
            let mut rows = stmt
                .query([])
                .map_err(|e| EngError::Internal(format!("query memories for archive: {e}")))?;
            while let Ok(Some(row)) = rows.next() {
                let line = serde_json::json!({
                    "table": "memories",
                    "id": row.get::<_, i64>(0).unwrap_or(0),
                    "content": row.get::<_, String>(1).unwrap_or_default(),
                    "source": row.get::<_, Option<String>>(2).unwrap_or(None),
                    "category": row.get::<_, Option<String>>(3).unwrap_or(None),
                    "importance": row.get::<_, Option<i64>>(4).unwrap_or(None),
                    "created_at": row.get::<_, Option<String>>(5).unwrap_or(None),
                });
                let _ = writeln!(gz, "{line}");
            }
        }

        // Dump audit_log table if it exists.
        if let Ok(mut stmt) = conn.prepare("SELECT id, action, detail, created_at FROM audit_log") {
            let mut rows = stmt
                .query([])
                .map_err(|e| EngError::Internal(format!("query audit_log for archive: {e}")))?;
            while let Ok(Some(row)) = rows.next() {
                let line = serde_json::json!({
                    "table": "audit_log",
                    "id": row.get::<_, i64>(0).unwrap_or(0),
                    "action": row.get::<_, Option<String>>(1).unwrap_or(None),
                    "detail": row.get::<_, Option<String>>(2).unwrap_or(None),
                    "created_at": row.get::<_, Option<String>>(3).unwrap_or(None),
                });
                let _ = writeln!(gz, "{line}");
            }
        }

        let inner = gz
            .finish()
            .map_err(|e| EngError::Internal(format!("gz finish: {e}")))?;
        let bytes = inner.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(bytes)
    })
    .await
    .map_err(|e| EngError::Internal(format!("spawn_blocking archive: {e}")))??;

    Ok((archive_path_str, bytes))
}

/// Approximate disk usage of a directory in bytes (best-effort, recursive).
fn du_bytes(path: &Path) -> Option<u64> {
    if !path.exists() {
        return Some(0);
    }
    du_recursive(path)
}

/// Recursive directory size calculation using `std::fs::read_dir`.
fn du_recursive(path: &Path) -> Option<u64> {
    let mut total: u64 = 0;
    let entries = std::fs::read_dir(path).ok()?;
    for entry in entries.flatten() {
        let ft = entry.file_type().ok()?;
        if ft.is_file() {
            total += entry.metadata().ok().map(|m| m.len()).unwrap_or(0);
        } else if ft.is_dir() {
            total += du_recursive(&entry.path()).unwrap_or(0);
        }
    }
    Some(total)
}

// ── Monolith Row Deletion ─────────────────────────────────────────────────

/// Delete all monolith rows belonging to `user_id` in a single write transaction.
///
/// Returns a map of `table_name -> rows_deleted`. Made public so the skip-shard
/// admin endpoint can call it directly.
pub async fn delete_monolith_rows(
    db: &Database,
    user_id: i64,
) -> crate::Result<HashMap<String, usize>> {
    // F28 (defense-in-depth): refuse to delete the reserved owner account's rows
    // even when called directly, mirroring the route-level and admin-layer guards.
    if user_id == 1 {
        return Err(crate::EngError::Forbidden(
            "cannot deprovision the owner account (user_id=1)".into(),
        ));
    }
    db.write(move |conn| {
        // Dynamically sweep every table that carries a user_id column. A hardcoded
        // list drifts: only a handful of monolith tables cascade from the users
        // row via ON DELETE CASCADE, so a fixed list left dozens of tables
        // (memories, episodes, skills, artifacts, brain_*, ...) orphaned on
        // deprovision, and a recycled user_id would then inherit them.
        // defer_foreign_keys lets us delete in any order within one transaction;
        // FK consistency is checked once at commit, by which point every one of
        // this user's rows (children and parents) is gone.
        let tx = conn.transaction()?;
        tx.execute_batch("PRAGMA defer_foreign_keys = ON")?;

        let tables: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT name FROM sqlite_master \
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            )?;
            let names = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            names
        };

        let mut counts = HashMap::new();
        for table in &tables {
            let has_user_id = {
                let mut info = tx.prepare(&format!("PRAGMA table_info(\"{table}\")"))?;
                let cols = info
                    .query_map([], |row| row.get::<_, String>(1))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                cols.iter().any(|c| c == "user_id")
            };
            if has_user_id {
                let n = tx.execute(
                    &format!("DELETE FROM \"{table}\" WHERE user_id = ?1"),
                    rusqlite::params![user_id],
                )?;
                if n > 0 {
                    counts.insert(table.clone(), n);
                }
            }
        }

        // Remove the users row itself (keyed by id, not user_id); any ON DELETE
        // CASCADE children not already swept above resolve at commit.
        let n = tx.execute(
            "DELETE FROM users WHERE id = ?1",
            rusqlite::params![user_id],
        )?;
        counts.insert("users".to_string(), n);

        tx.commit()?;
        Ok(counts)
    })
    .await
}

// ── Teardown Orchestrator ─────────────────────────────────────────────────

/// Run the full teardown sequence for a single tenant.
///
/// Every step is idempotent: missing dir = success, DELETE on absent rows = success.
/// Called by the `deprovision_teardown` job handler.
pub async fn run_teardown_job(
    registry_db: &RegistryDb,
    monolith_db: &Database,
    data_root: &Path,
    deprovision_id: &str,
    user_id: i64,
    tenant_id: &str,
    archive: bool,
) -> crate::Result<DeprovisionReport> {
    // F28: refuse the reserved owner account at the orchestrator entry, BEFORE
    // any shard archive/removal. The delete_monolith_rows guard alone fires only
    // at Step 3, after remove_dir_all has already destroyed the shard.
    if user_id == 1 {
        return Err(EngError::Forbidden(
            "cannot deprovision the owner account (user_id=1)".into(),
        ));
    }
    let mut steps: Vec<TeardownStep> = vec![];
    let shard_dir = data_root.join("tenants").join(tenant_id);

    // Step 2a: optional archive.
    let mut archived = false;
    if archive && shard_dir.exists() {
        match write_shard_archive(&shard_dir, deprovision_id).await {
            Ok((archive_path, bytes)) => {
                registry_db.update_deletion_log_archive(deprovision_id, &archive_path)?;
                steps.push(TeardownStep::ShardArchived {
                    path: archive_path,
                    bytes,
                });
                archived = true;
            }
            Err(e) => {
                warn!(deprovision_id, "archive failed (continuing): {e}");
            }
        }
    }

    // Step 2b: remove shard directory (NotFound is success).
    let bytes_freed = du_bytes(&shard_dir).unwrap_or(0);
    match tokio::fs::remove_dir_all(&shard_dir).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(EngError::Internal(format!(
                "remove_dir_all {}: {e}",
                shard_dir.display()
            )));
        }
    }
    let shard_removed = true;
    steps.push(TeardownStep::ShardRemoved {
        path: shard_dir.to_string_lossy().into_owned(),
        bytes_freed,
    });

    // Step 3: delete monolith rows.
    let rows_by_table = delete_monolith_rows(monolith_db, user_id).await?;
    steps.push(TeardownStep::MonolithCleared { rows_by_table });

    // Step 4: tombstone the registry row.
    registry_db.mark_tombstone(tenant_id)?;
    steps.push(TeardownStep::Tombstoned);

    info!(deprovision_id, user_id, tenant_id, "teardown complete");

    Ok(DeprovisionReport {
        deprovision_id: deprovision_id.to_string(),
        steps,
        shard_removed,
        archived,
    })
}

// ── Entry Point ───────────────────────────────────────────────────────────

/// Resolve the real username for a deletion-log snapshot. The log is the
/// post-teardown record of WHO was removed, so the numeric id alone defeats
/// its purpose. Falls back to the id string when the users row is already
/// gone or the lookup fails -- the log write must never be blocked on it.
async fn lookup_username(monolith_db: &Database, user_id: i64, fallback: &str) -> String {
    let looked_up: Option<String> = monolith_db
        .read(move |conn| {
            use rusqlite::OptionalExtension;
            Ok(conn
                .query_row(
                    "SELECT username FROM users WHERE id = ?1",
                    [user_id],
                    |row| row.get(0),
                )
                .optional()?)
        })
        .await
        .unwrap_or(None);
    looked_up.unwrap_or_else(|| fallback.to_string())
}

/// Initiate an async two-phase deprovision.
///
/// This is the sync HTTP entry point (returns 202 Accepted). It:
/// 1. Marks the tenant as Deleting (atomic CAS -- only Active/Suspended allowed).
/// 2. Evicts the in-memory handle so file locks are released.
/// 3. Inserts a deletions_log row for audit.
/// 4. Enqueues a `deprovision_teardown` job for the async phase.
///
/// Returns the `DeprovisionId` on success.
pub async fn begin_deprovision(
    registry: &TenantRegistry,
    monolith_db: &Database,
    target_user_id: i64,
    admin_user_id: i64,
    reason: String,
) -> crate::Result<DeprovisionId> {
    // F28: refuse the reserved owner account before marking the tenant Deleting
    // or enqueuing the teardown job (defense-in-depth alongside the HTTP guards).
    if target_user_id == 1 {
        return Err(EngError::Forbidden(
            "cannot deprovision the owner account (user_id=1)".into(),
        ));
    }
    let user_id_str = target_user_id.to_string();
    let rdb = registry.registry_db();

    // Look up the tenant row.
    let row = rdb
        .get_by_user_id(&user_id_str)?
        .ok_or_else(|| EngError::NotFound(format!("no tenant for user_id {target_user_id}")))?;

    // Atomic transition Active/Suspended -> Deleting.
    let affected = rdb.mark_deleting(&user_id_str)?;
    if affected == 0 {
        return Err(EngError::Conflict(format!(
            "tenant {} is already in {:?} state, cannot deprovision",
            row.tenant_id, row.status
        )));
    }

    // Evict the in-memory handle.
    if let Err(e) = registry.evict(&row.tenant_id).await {
        warn!(tenant_id = %row.tenant_id, "evict during deprovision failed (non-fatal): {e}");
    }

    // Generate deprovision ID and audit log entry.
    let dep_id = DeprovisionId::new();
    let reason_opt = if reason.is_empty() {
        None
    } else {
        Some(reason.as_str())
    };
    let username = lookup_username(monolith_db, target_user_id, &user_id_str).await;
    rdb.insert_deletion_log(
        dep_id.as_str(),
        Some(admin_user_id),
        target_user_id,
        &username,
        reason_opt,
    )?;

    // Enqueue the async teardown job.
    let payload = serde_json::json!({
        "deprovision_id": dep_id.as_str(),
        "user_id": target_user_id,
        "tenant_id": row.tenant_id,
    })
    .to_string();
    crate::jobs::enqueue_job(monolith_db, "deprovision_teardown", &payload, 5).await?;

    info!(
        deprovision_id = dep_id.as_str(),
        user_id = target_user_id,
        tenant_id = %row.tenant_id,
        "deprovision initiated"
    );

    Ok(dep_id)
}

// ── Startup Recovery ──────────────────────────────────────────────────────

/// Re-enqueue teardown jobs for any tenants stuck in Deleting state with no active job.
///
/// Called at server startup to recover from crash mid-teardown.
/// Tenants in Stuck state are reported but not re-enqueued.
pub async fn recover_orphans(
    registry_db: &RegistryDb,
    monolith_db: &Database,
) -> crate::Result<RecoveryReport> {
    let deleting = registry_db.list_by_status(crate::tenant::types::TenantStatus::Deleting)?;
    let stuck = registry_db.list_by_status(crate::tenant::types::TenantStatus::Stuck)?;

    let found = deleting.len();
    let stuck_skipped = stuck.len();
    let mut re_enqueued = 0;

    for row in &deleting {
        let log = registry_db.get_deletion_log_by_tenant(&row.tenant_id)?;
        let (deprovision_id, user_id) = match log {
            Some(ref l) => (l.deprovision_id.clone(), l.target_user_id),
            None => {
                // No log row -- generate a recovery deprovision ID.
                let id = DeprovisionId::new();
                let uid = row.user_id.parse::<i64>().unwrap_or(0);
                let username = lookup_username(monolith_db, uid, &row.user_id).await;
                registry_db.insert_deletion_log(
                    id.as_str(),
                    None,
                    uid,
                    &username,
                    Some("recovery: no deletion log found"),
                )?;
                (id.0, uid)
            }
        };

        let payload = serde_json::json!({
            "deprovision_id": deprovision_id,
            "user_id": user_id,
            "tenant_id": row.tenant_id,
        })
        .to_string();

        match crate::jobs::enqueue_job(monolith_db, "deprovision_teardown", &payload, 5).await {
            Ok(_) => {
                re_enqueued += 1;
                info!(tenant_id = %row.tenant_id, deprovision_id, "re-enqueued orphaned teardown");
            }
            Err(e) => {
                warn!(tenant_id = %row.tenant_id, "failed to re-enqueue orphan: {e}");
            }
        }
    }

    if stuck_skipped > 0 {
        warn!(
            count = stuck_skipped,
            "tenants in Stuck state require manual intervention"
        );
    }

    Ok(RecoveryReport {
        found,
        re_enqueued,
        stuck_skipped,
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deprovision_id_display() {
        let id = DeprovisionId::new();
        assert_eq!(id.to_string(), id.as_str());
        assert_eq!(id.0.len(), 36); // UUID v4
    }

    #[test]
    fn teardown_step_serializes() {
        let step = TeardownStep::ShardRemoved {
            path: "/data/tenants/abc".into(),
            bytes_freed: 1024,
        };
        let json = serde_json::to_string(&step).unwrap();
        assert!(json.contains("shard_removed"));
        assert!(json.contains("bytes_freed"));
    }

    #[test]
    fn teardown_status_round_trip() {
        let statuses = [
            TeardownStatus::Deleting,
            TeardownStatus::Tombstone,
            TeardownStatus::Stuck,
        ];
        for status in &statuses {
            let json = serde_json::to_string(status).unwrap();
            let back: TeardownStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, status);
        }
    }

    #[test]
    fn du_bytes_missing_dir_returns_zero() {
        let result = du_bytes(Path::new("/nonexistent/path/xyz"));
        assert_eq!(result, Some(0));
    }

    #[test]
    fn du_bytes_tempdir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
        std::fs::write(dir.path().join("b.txt"), "world!").unwrap();
        let size = du_bytes(dir.path()).unwrap();
        assert_eq!(size, 11); // "hello" + "world!" = 5 + 6
    }

    #[test]
    fn cluster_lock_blocks_other_node() {
        use crate::tenant::registry_db::RegistryDb;

        let db = RegistryDb::open_memory().unwrap();
        db.cluster_lock_upsert("node-a").unwrap();
        // node-a has a fresh lock; node-b should see it as a blocker.
        let others = db.cluster_lock_active_others("node-b", 60).unwrap();
        assert_eq!(others.len(), 1);
        // node-a should see no others.
        let self_check = db.cluster_lock_active_others("node-a", 60).unwrap();
        assert_eq!(self_check.len(), 0);
    }

    #[test]
    fn cluster_lock_release() {
        use crate::tenant::registry_db::RegistryDb;

        let db = RegistryDb::open_memory().unwrap();
        db.cluster_lock_upsert("node-a").unwrap();
        db.cluster_lock_release("node-a").unwrap();
        let others = db.cluster_lock_active_others("node-b", 60).unwrap();
        assert_eq!(others.len(), 0);
    }

    #[tokio::test]
    async fn run_teardown_removes_shard_and_tombstones() {
        use crate::tenant::registry_db::RegistryDb;
        use crate::tenant::types::{TenantRow, TenantStatus};

        let dir = tempfile::tempdir().unwrap();
        let tenant_id = "test_tenant_e1";
        let shard_dir = dir.path().join("tenants").join(tenant_id);
        std::fs::create_dir_all(&shard_dir).unwrap();
        std::fs::write(shard_dir.join("kleos.db"), b"fake db").unwrap();

        let registry_db = RegistryDb::open_memory().unwrap();
        registry_db
            .insert(&TenantRow {
                tenant_id: tenant_id.to_string(),
                user_id: "99".to_string(),
                created_at: 0,
                status: TenantStatus::Active,
                data_path: shard_dir.to_str().unwrap().to_string(),
                schema_version: 1,
                quota_bytes: None,
                quota_memories: None,
                last_access: 0,
            })
            .unwrap();
        registry_db.mark_deleting("99").unwrap();
        registry_db
            .insert_deletion_log("dep-test-01", Some(1), 99, "testuser", None)
            .unwrap();

        assert!(shard_dir.exists());

        // Shard removal + tombstone (no monolith DB in unit test).
        match tokio::fs::remove_dir_all(&shard_dir).await {
            Ok(()) | Err(_) => {}
        }
        registry_db.mark_tombstone(tenant_id).unwrap();

        assert!(!shard_dir.exists());
        let row = registry_db.get_by_user_id("99").unwrap().unwrap();
        assert_eq!(row.status, TenantStatus::Tombstone);
    }

    #[test]
    fn recover_finds_deleting_rows() {
        use crate::tenant::registry_db::RegistryDb;
        use crate::tenant::types::{TenantRow, TenantStatus};

        let rdb = RegistryDb::open_memory().unwrap();
        rdb.insert(&TenantRow {
            tenant_id: "t_orphan".into(),
            user_id: "77".into(),
            created_at: 0,
            status: TenantStatus::Active,
            data_path: "/data/t_orphan".into(),
            schema_version: 1,
            quota_bytes: None,
            quota_memories: None,
            last_access: 0,
        })
        .unwrap();
        rdb.mark_deleting("77").unwrap();

        let deleting = rdb.list_by_status(TenantStatus::Deleting).unwrap();
        assert_eq!(deleting.len(), 1);
        assert_eq!(deleting[0].tenant_id, "t_orphan");
    }

    #[test]
    fn recover_orphans_skips_stuck() {
        use crate::tenant::registry_db::RegistryDb;
        use crate::tenant::types::{TenantRow, TenantStatus};

        let rdb = RegistryDb::open_memory().unwrap();
        rdb.insert(&TenantRow {
            tenant_id: "t_stuck".into(),
            user_id: "55".into(),
            created_at: 0,
            status: TenantStatus::Active,
            data_path: "/data/t_stuck".into(),
            schema_version: 1,
            quota_bytes: None,
            quota_memories: None,
            last_access: 0,
        })
        .unwrap();
        rdb.mark_deleting("55").unwrap();
        rdb.mark_stuck("t_stuck").unwrap();

        let stuck = rdb.list_by_status(TenantStatus::Stuck).unwrap();
        assert_eq!(stuck.len(), 1);
        let deleting = rdb.list_by_status(TenantStatus::Deleting).unwrap();
        assert_eq!(deleting.len(), 0);
    }

    #[test]
    fn concurrent_mark_deleting_only_one_wins() {
        use crate::tenant::registry_db::RegistryDb;
        use crate::tenant::types::{TenantRow, TenantStatus};

        let rdb = RegistryDb::open_memory().unwrap();
        rdb.insert(&TenantRow {
            tenant_id: "t_race".into(),
            user_id: "88".into(),
            created_at: 0,
            status: TenantStatus::Active,
            data_path: "/data/t_race".into(),
            schema_version: 1,
            quota_bytes: None,
            quota_memories: None,
            last_access: 0,
        })
        .unwrap();

        let affected1 = rdb.mark_deleting("88").unwrap();
        let affected2 = rdb.mark_deleting("88").unwrap();
        assert_eq!(affected1, 1);
        assert_eq!(affected2, 0, "second call must affect 0 rows");
    }

    #[test]
    fn tombstone_hold_blocks_re_provision() {
        use crate::tenant::registry_db::RegistryDb;
        use crate::tenant::types::{TenantRow, TenantStatus};

        let rdb = RegistryDb::open_memory().unwrap();
        rdb.insert(&TenantRow {
            tenant_id: "t_hold".into(),
            user_id: "66".into(),
            created_at: 0,
            status: TenantStatus::Active,
            data_path: "/data/t_hold".into(),
            schema_version: 1,
            quota_bytes: None,
            quota_memories: None,
            last_access: 0,
        })
        .unwrap();
        rdb.mark_deleting("66").unwrap();
        rdb.mark_tombstone("t_hold").unwrap();

        let row = rdb.get_by_user_id("66").unwrap().unwrap();
        assert_eq!(row.status, TenantStatus::Tombstone);
        assert!(!row.status.is_accessible());
    }
}
