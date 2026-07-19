//! Background tasks that run on a timer for the duration of the server process.

use kleos_lib::db::Database;
use kleos_lib::embeddings::EmbeddingProvider;
use kleos_lib::tenant::TenantRegistry;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

/// Cap for exponential backoff: 5 minutes.
const MAX_BACKOFF: Duration = Duration::from_secs(300);

/// Runs a WAL checkpoint on a 5-minute interval.
/// Uses PASSIVE mode so readers are never blocked.
/// TRUNCATE mode is used once at startup to shrink any large WAL leftover from
/// a previous run.
pub fn start_auto_checkpoint_task(
    db: Arc<Database>,
) -> (CancellationToken, tokio::task::JoinHandle<()>) {
    let token = CancellationToken::new();
    let cancel = token.clone();

    let handle = tokio::spawn(async move {
        // Startup TRUNCATE: flush any WAL accumulated before this process started.
        match kleos_lib::db::backup::wal_checkpoint(
            &db,
            kleos_lib::db::backup::CheckpointMode::Truncate,
        )
        .await
        {
            Ok((busy, log, cp)) => info!(busy, log, checkpointed = cp, "startup WAL checkpoint"),
            Err(e) => warn!(error = %e, "startup WAL checkpoint failed"),
        }

        let interval = Duration::from_secs(300);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("auto-checkpoint task shutting down");
                    break;
                }
                _ = tokio::time::sleep(interval) => {
                    match kleos_lib::db::backup::wal_checkpoint(
                        &db,
                        kleos_lib::db::backup::CheckpointMode::Passive,
                    )
                    .await
                    {
                        Ok((busy, log, cp)) => {
                            info!(busy, log, checkpointed = cp, "periodic WAL checkpoint");
                        }
                        Err(e) => warn!(error = %e, "periodic WAL checkpoint failed"),
                    }
                }
            }
        }
    });

    (token, handle)
}

/// Drains the durable jobs queue. A single worker loops over
/// [`kleos_lib::jobs::process_next_job`], which atomically claims a pending
/// row, runs the registered handler, and records success/failure/retry.
///
/// The worker polls the queue every 200ms when empty and spins tight when
/// work is available. In tenant-sharding mode each shard has its own private
/// `jobs` table that nothing else polls, so every tick also drains the
/// RESIDENT active shards (`snapshot_all_handles` -- no loads, no touches, so
/// polling cannot pin tenants resident or thrash eviction). Every 5 minutes
/// it calls [`kleos_lib::jobs::recover_stuck_jobs`] to unstick rows abandoned
/// by a crashed worker, and in sharded mode sequentially sweeps ALL active
/// tenants (loading them one at a time) to recover and drain backlogs on
/// shards that were evicted with jobs still pending.
pub fn start_job_worker_task(
    db: Arc<Database>,
    registry: Option<Arc<TenantRegistry>>,
) -> (CancellationToken, tokio::task::JoinHandle<()>) {
    let token = CancellationToken::new();
    let cancel = token.clone();

    let handle = tokio::spawn(async move {
        let poll_interval = Duration::from_millis(200);
        let stuck_recovery_interval = Duration::from_secs(300);
        let mut last_stuck_recovery = tokio::time::Instant::now();
        let mut consecutive_errors: u32 = 0;

        loop {
            if cancel.is_cancelled() {
                info!("job-worker task shutting down");
                break;
            }

            if last_stuck_recovery.elapsed() >= stuck_recovery_interval {
                match kleos_lib::jobs::recover_stuck_jobs(&db).await {
                    Ok(n) if n > 0 => {
                        warn!(recovered = n, "job-worker re-queued stuck jobs");
                    }
                    Ok(_) => {}
                    Err(e) => warn!(error = %e, "job-worker recover_stuck_jobs failed"),
                }
                if let Some(ref reg) = registry {
                    sweep_tenant_job_backlogs(reg, &cancel).await;
                }
                last_stuck_recovery = tokio::time::Instant::now();
            }

            // One drain pass: the monolith queue, then every resident shard.
            let mut worked = false;
            match kleos_lib::jobs::process_next_job(&db).await {
                Ok(did_work) => {
                    consecutive_errors = 0;
                    worked |= did_work;
                }
                Err(e) => {
                    consecutive_errors = consecutive_errors.saturating_add(1);
                    error!(
                        error = %e,
                        consecutive_errors,
                        "job-worker process_next_job error"
                    );
                    // Exponential backoff on repeated errors so a broken DB
                    // does not spin the worker at 100% CPU.
                    let backoff =
                        Duration::from_secs(2u64.saturating_pow(consecutive_errors.min(8)))
                            .min(MAX_BACKOFF);
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = tokio::time::sleep(backoff) => {}
                    }
                    continue;
                }
            }
            if let Some(ref reg) = registry {
                for handle in reg.snapshot_all_handles().await {
                    if cancel.is_cancelled() {
                        break;
                    }
                    match kleos_lib::jobs::process_next_job(&handle.db).await {
                        Ok(did_work) => worked |= did_work,
                        // One broken shard must not stall the others or the
                        // monolith: log and move on.
                        Err(e) => warn!(
                            tenant = %handle.tenant_id,
                            error = %e,
                            "job-worker shard process_next_job error"
                        ),
                    }
                }
            }

            if !worked {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tokio::time::sleep(poll_interval) => {}
                }
            }
        }
    });

    (token, handle)
}

/// Slow-cadence backlog sweep for tenant shards (sharded mode only).
///
/// Loads each active tenant one at a time (bounded residency churn: at most
/// one extra shard is materialized at any moment, and LRU eviction reclaims
/// it), re-queues rows abandoned at 'running', and drains any pending jobs
/// the fast path missed because the shard was not resident when they were
/// enqueued.
async fn sweep_tenant_job_backlogs(reg: &Arc<TenantRegistry>, cancel: &CancellationToken) {
    let tenants = match reg.list() {
        Ok(t) => t,
        Err(e) => {
            warn!(error = %e, "job-worker sweep: failed to list tenants");
            return;
        }
    };
    for row in tenants {
        if cancel.is_cancelled() {
            return;
        }
        if row.status != kleos_lib::tenant::TenantStatus::Active {
            continue;
        }
        let handle = match reg.get(&row.user_id).await {
            Ok(Some(h)) => h,
            Ok(None) => continue,
            Err(e) => {
                warn!(tenant = %row.tenant_id, error = %e, "job-worker sweep: shard load failed");
                continue;
            }
        };
        match kleos_lib::jobs::recover_stuck_jobs(&handle.db).await {
            Ok(n) if n > 0 => {
                warn!(tenant = %handle.tenant_id, recovered = n, "job-worker re-queued stuck shard jobs");
            }
            Ok(_) => {}
            Err(e) => {
                warn!(tenant = %handle.tenant_id, error = %e, "job-worker sweep: recover_stuck_jobs failed");
            }
        }
        // Drain this shard's backlog before moving on so a shard that gets
        // evicted right after the sweep still had its queue emptied.
        loop {
            match kleos_lib::jobs::process_next_job(&handle.db).await {
                Ok(true) => {}
                Ok(false) => break,
                Err(e) => {
                    warn!(tenant = %handle.tenant_id, error = %e, "job-worker sweep: process_next_job failed");
                    break;
                }
            }
        }
    }
}

/// Deletes completed jobs older than 1 hour on an hourly interval.
/// RB-L5: failures back off exponentially (doubling each time, capped at 5 min).
pub fn start_job_cleanup_task(
    db: Arc<Database>,
) -> (CancellationToken, tokio::task::JoinHandle<()>) {
    let token = CancellationToken::new();
    let cancel = token.clone();

    let handle = tokio::spawn(async move {
        let base_interval = Duration::from_secs(3600);
        let mut consecutive_failures: u32 = 0;
        loop {
            // Backoff sleep replaces the normal interval after failures.
            let sleep_dur = if consecutive_failures > 0 {
                let backoff = Duration::from_secs(2u64.pow(consecutive_failures.min(8)));
                backoff.min(MAX_BACKOFF)
            } else {
                base_interval
            };
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("job-cleanup task shutting down");
                    break;
                }
                _ = tokio::time::sleep(sleep_dur) => {
                    match kleos_lib::jobs::cleanup_completed_jobs(&db).await {
                        Ok(n) => {
                            info!(deleted = n, "job cleanup complete");
                            consecutive_failures = 0;
                        }
                        Err(e) => {
                            consecutive_failures += 1;
                            error!(
                                error = %e,
                                consecutive_failures,
                                "job cleanup failed"
                            );
                        }
                    }

                    // SECURITY: prune expired rate-limit rows to prevent
                    // unbounded table growth from spoofed pre-auth keys.
                    // Grace period of 300s (5 min) keeps rows for a bit
                    // after window expiry to avoid edge-case resets.
                    match kleos_lib::ratelimit::cleanup_expired_rows(&db, 300).await {
                        Ok(n) if n > 0 => info!(deleted = n, "rate-limit row cleanup"),
                        Ok(_) => {}
                        Err(e) => warn!(error = %e, "rate-limit row cleanup failed"),
                    }
                }
            }
        }
    });

    (token, handle)
}

/// Replays failed LanceDB vector sync operations on a 10-minute interval.
/// Skips silently when no vector index is configured.
/// RB-L5: failures back off exponentially (doubling each time, capped at 5 min).
/// MT-F17: per-tenant round-robin scheduling prevents a single tenant with many
/// pending rows from starving others. A monotonic sequence counter tracks when
/// each tenant was last served; the tenant with the lowest counter (i.e. served
/// least recently, or never) is chosen each tick.
///
/// When `registry` is Some (tenant-sharding mode), iterates over active tenants
/// via `registry.list()` and replays each tenant shard independently. When
/// `registry` is None (single-DB / non-multi-tenant mode), the original monolith
/// path is used: query `vector_sync_pending_users` against `db` and replay there.
pub fn start_vector_sync_replay_task(
    db: Arc<Database>,
    registry: Option<Arc<TenantRegistry>>,
    embedder: Arc<RwLock<Option<Arc<dyn EmbeddingProvider>>>>,
) -> (CancellationToken, tokio::task::JoinHandle<()>) {
    let token = CancellationToken::new();
    let cancel = token.clone();

    let handle = tokio::spawn(async move {
        let base_interval = Duration::from_secs(600);
        let mut consecutive_failures: u32 = 0;
        // MT-F17: last-served sequence number keyed on tenant_id (String).
        // In single-DB mode the key is the stringified user_id for consistency.
        let mut last_served: HashMap<String, u64> = HashMap::new();
        let mut serve_seq: u64 = 0;

        loop {
            let sleep_dur = if consecutive_failures > 0 {
                let backoff = Duration::from_secs(2u64.pow(consecutive_failures.min(8)));
                backoff.min(MAX_BACKOFF)
            } else {
                base_interval
            };
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("vector-sync-replay task shutting down");
                    break;
                }
                _ = tokio::time::sleep(sleep_dur) => {
                    if let Some(ref reg) = registry {
                        // Tenant-sharding mode: enumerate active tenants from registry.
                        let tenants = match reg.list() {
                            Ok(t) => t,
                            Err(e) => {
                                consecutive_failures += 1;
                                error!(error = %e, consecutive_failures, "vector sync: failed to list tenants");
                                continue;
                            }
                        };

                        // Collect (tenant_id, user_id, db) for active tenants only.
                        let mut candidates: Vec<(String, i64, Arc<Database>)> = Vec::new();
                        for tenant_row in tenants {
                            if tenant_row.status != kleos_lib::tenant::TenantStatus::Active {
                                continue;
                            }
                            let user_id = match tenant_row.user_id.parse::<i64>() {
                                Ok(uid) => uid,
                                Err(_) => {
                                    continue;
                                }
                            };
                            let handle = match reg.get(&tenant_row.user_id).await {
                                Ok(Some(h)) => h,
                                Ok(None) => continue,
                                Err(e) => {
                                    warn!(
                                        tenant = %tenant_row.tenant_id,
                                        error = %e,
                                        "vector sync: failed to open tenant shard; skipping"
                                    );
                                    continue;
                                }
                            };
                            candidates.push((tenant_row.tenant_id.clone(), user_id, Arc::clone(&handle.db)));
                        }

                        if candidates.is_empty() {
                            consecutive_failures = 0;
                            continue;
                        }

                        // Round-robin: pick tenant served least recently (lowest sequence).
                        let idx = candidates
                            .iter()
                            .enumerate()
                            .min_by_key(|(_, (tid, _, _))| last_served.get(tid.as_str()).copied().unwrap_or(0))
                            .map(|(i, _)| i)
                            .expect("non-empty vec has a minimum");
                        let (tenant_id, user_id, tenant_db) = candidates.swap_remove(idx);

                        match kleos_lib::memory::replay_vector_sync_pending_for_user(
                            &tenant_db,
                            user_id,
                            100,
                        )
                        .await
                        {
                            Ok(report) => {
                                consecutive_failures = 0;
                                serve_seq += 1;
                                last_served.insert(tenant_id.clone(), serve_seq);
                                if report.processed > 0 {
                                    info!(
                                        tenant = %tenant_id,
                                        user_id,
                                        processed = report.processed,
                                        succeeded = report.succeeded,
                                        failed = report.failed,
                                        skipped = report.skipped,
                                        "vector sync replay complete"
                                    );
                                }
                            }
                            Err(e) => {
                                consecutive_failures += 1;
                                error!(
                                    error = %e,
                                    tenant = %tenant_id,
                                    user_id,
                                    consecutive_failures,
                                    "vector sync replay failed"
                                );
                            }
                        }

                        // Bounded backfill of intelligence/scratchpad-created memories
                        // that were written without an embedding (memory::store path).
                        // 50 per tick keeps a steady-state stream from piling up
                        // without monopolising the sweeper.
                        let embedder_ref = embedder.read().await.as_ref().cloned();
                        if let Some(emb) = embedder_ref {
                            match kleos_lib::memory::backfill_missing_embeddings_limited(
                                &tenant_db,
                                emb.as_ref(),
                                Some(50),
                            )
                            .await
                            {
                                Ok(report) if report.scanned > 0 => info!(
                                    tenant = %tenant_id,
                                    user_id,
                                    scanned = report.scanned,
                                    primary = report.primary_embeddings_filled,
                                    chunks = report.chunk_rows_written,
                                    failures = report.failures,
                                    "background backfill swept"
                                ),
                                Ok(_) => {}
                                Err(e) => warn!(
                                    error = %e,
                                    tenant = %tenant_id,
                                    user_id,
                                    "background backfill sweep failed"
                                ),
                            }
                        }
                    } else {
                        // Single-DB (monolith) mode: query pending users from the monolith db.
                        let user_ids = match kleos_lib::memory::vector_sync_pending_users(&db).await {
                            Ok(ids) => ids,
                            Err(e) => {
                                consecutive_failures += 1;
                                error!(error = %e, consecutive_failures, "vector sync: failed to query pending users");
                                continue;
                            }
                        };

                        if user_ids.is_empty() {
                            consecutive_failures = 0;
                            continue;
                        }

                        // Round-robin: pick user served least recently (lowest sequence).
                        let next_user = user_ids
                            .iter()
                            .copied()
                            .min_by_key(|uid| last_served.get(&uid.to_string()).copied().unwrap_or(0))
                            .expect("non-empty vec has a minimum");

                        match kleos_lib::memory::replay_vector_sync_pending_for_user(
                            &db,
                            next_user,
                            100,
                        )
                        .await
                        {
                            Ok(report) => {
                                consecutive_failures = 0;
                                serve_seq += 1;
                                last_served.insert(next_user.to_string(), serve_seq);
                                if report.processed > 0 {
                                    info!(
                                        user_id = next_user,
                                        processed = report.processed,
                                        succeeded = report.succeeded,
                                        failed = report.failed,
                                        skipped = report.skipped,
                                        "vector sync replay complete"
                                    );
                                }
                            }
                            Err(e) => {
                                consecutive_failures += 1;
                                error!(
                                    error = %e,
                                    user_id = next_user,
                                    consecutive_failures,
                                    "vector sync replay failed"
                                );
                            }
                        }

                        let embedder_ref = embedder.read().await.as_ref().cloned();
                        if let Some(emb) = embedder_ref {
                            match kleos_lib::memory::backfill_missing_embeddings_limited(
                                &db,
                                emb.as_ref(),
                                Some(50),
                            )
                            .await
                            {
                                Ok(report) if report.scanned > 0 => info!(
                                    user_id = next_user,
                                    scanned = report.scanned,
                                    primary = report.primary_embeddings_filled,
                                    chunks = report.chunk_rows_written,
                                    failures = report.failures,
                                    "background backfill swept (monolith)"
                                ),
                                Ok(_) => {}
                                Err(e) => warn!(
                                    error = %e,
                                    user_id = next_user,
                                    "background backfill sweep failed (monolith)"
                                ),
                            }
                        }
                    }
                }
            }
        }
    });

    (token, handle)
}

/// Resolve the backup directory. A relative `backup_dir` resolves under
/// `data_dir`; an absolute path is used as-is.
pub fn resolve_backup_dir(data_dir: &str, backup_dir: &str) -> PathBuf {
    let p = PathBuf::from(backup_dir);
    if p.is_absolute() {
        p
    } else {
        PathBuf::from(data_dir).join(p)
    }
}

/// Format `kleos-backup-YYYYMMDD-HHMMSS.db` timestamp component.
fn backup_filename(now: chrono::DateTime<chrono::Utc>) -> String {
    format!("kleos-backup-{}.db", now.format("%Y%m%d-%H%M%S"))
}

/// List existing backup files in `dir` sorted oldest-first.
pub fn list_backups(dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("kleos-backup-") && n.ends_with(".db"))
            })
            .collect(),
        Err(_) => return Vec::new(),
    };
    entries.sort();
    entries
}

/// Extract the `YYYYMMDD` date component from a backup filename.
/// Returns None if the filename doesn't match `kleos-backup-YYYYMMDD-HHMMSS.db`.
pub fn backup_date_component(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let stem = name.strip_prefix("kleos-backup-")?.strip_suffix(".db")?;
    // stem is "YYYYMMDD-HHMMSS"
    let (date, _) = stem.split_once('-')?;
    if date.len() == 8 && date.chars().all(|c| c.is_ascii_digit()) {
        Some(date.to_string())
    } else {
        None
    }
}

/// Copy `src` to the daily directory if no backup for the UTC date implied by
/// `src`'s filename already exists there. Returns Ok(Some(dest)) when a copy
/// occurred, Ok(None) when skipped. Errors propagate so the caller can log.
///
/// Promotion is idempotent: running the hourly backup several times on the
/// same day produces only one daily entry (the first one verified).
pub fn promote_to_daily(src: &Path, daily_dir: &Path) -> std::io::Result<Option<PathBuf>> {
    std::fs::create_dir_all(daily_dir)?;
    let Some(date) = backup_date_component(src) else {
        return Ok(None);
    };
    let already_promoted = list_backups(daily_dir)
        .iter()
        .any(|p| backup_date_component(p).as_deref() == Some(date.as_str()));
    if already_promoted {
        return Ok(None);
    }
    let filename = src
        .file_name()
        .ok_or_else(|| std::io::Error::other("backup source has no filename"))?;
    let dest = daily_dir.join(filename);
    std::fs::copy(src, &dest)?;
    Ok(Some(dest))
}

/// Prune backups beyond `retention`. Oldest files are deleted first.
/// Returns the count of deleted files.
pub fn prune_backups(dir: &Path, retention: usize) -> usize {
    let backups = list_backups(dir);
    if backups.len() <= retention {
        return 0;
    }
    let to_delete = backups.len() - retention;
    let mut deleted = 0;
    for path in backups.iter().take(to_delete) {
        match std::fs::remove_file(path) {
            Ok(()) => deleted += 1,
            Err(e) => warn!(path = %path.display(), error = %e, "failed to prune backup"),
        }
    }
    deleted
}

/// Runs `VACUUM INTO <dir>/kleos-backup-<ts>.db` on a configured interval,
/// verifies `PRAGMA integrity_check` AND a read-only restore-test query on
/// the result, then prunes oldest backups beyond retention. After the hourly
/// backup is verified the first time for each UTC date, it's promoted by
/// copy into `<dir>/daily/` and pruned independently (`retention_daily`).
/// Disabled when `backup_enabled` is false.
pub fn start_auto_backup_task(
    db: Arc<Database>,
    data_dir: String,
    backup_dir: String,
    interval_secs: u64,
    retention: usize,
    retention_daily: usize,
    encryption_key: Option<[u8; 32]>,
) -> (CancellationToken, tokio::task::JoinHandle<()>) {
    let token = CancellationToken::new();
    let cancel = token.clone();
    let dir = resolve_backup_dir(&data_dir, &backup_dir);
    let daily_dir = dir.join("daily");

    let handle = tokio::spawn(async move {
        if let Err(e) = tokio::fs::create_dir_all(&dir).await {
            error!(dir = %dir.display(), error = %e, "failed to create backup dir; task exiting");
            return;
        }
        if let Err(e) = tokio::fs::create_dir_all(&daily_dir).await {
            warn!(dir = %daily_dir.display(), error = %e, "failed to create daily backup dir");
        }

        let base_interval = Duration::from_secs(interval_secs.max(60));
        let mut consecutive_failures: u32 = 0;
        loop {
            let sleep_dur = if consecutive_failures > 0 {
                Duration::from_secs(2u64.pow(consecutive_failures.min(8))).min(MAX_BACKOFF)
            } else {
                base_interval
            };
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("auto-backup task shutting down");
                    break;
                }
                _ = tokio::time::sleep(sleep_dur) => {
                    let now = chrono::Utc::now();
                    let dest = dir.join(backup_filename(now));
                    match kleos_lib::db::backup::vacuum_into(&db, &dest).await {
                        Ok(()) => {
                            match verify_backup(&dest, encryption_key).await {
                                Ok(report) => {
                                    let pruned_hourly = prune_backups(&dir, retention);
                                    let (promoted, pruned_daily) =
                                        promote_and_prune_daily(&dest, &daily_dir, retention_daily);
                                    info!(
                                        path = %dest.display(),
                                        pruned_hourly,
                                        retention,
                                        promoted = promoted
                                            .as_ref()
                                            .map(|p| p.display().to_string())
                                            .unwrap_or_else(|| "none".into()),
                                        pruned_daily,
                                        retention_daily,
                                        schema_version = report.schema_version,
                                        table_count = report.table_count,
                                        memory_count = ?report.memory_count,
                                        "scheduled backup verified"
                                    );
                                    consecutive_failures = 0;
                                }
                                Err(e) => {
                                    consecutive_failures += 1;
                                    // Remove the unverified backup: a file that failed
                                    // integrity_check or the restore-test is not a usable
                                    // backup, and leaving it lets bad files accumulate and
                                    // masquerade as recovery points during pruning/restore.
                                    if let Err(rm) = tokio::fs::remove_file(&dest).await {
                                        warn!(
                                            path = %dest.display(),
                                            error = %rm,
                                            "failed to remove unverified backup file"
                                        );
                                    }
                                    error!(
                                        path = %dest.display(),
                                        error = %e,
                                        consecutive_failures,
                                        "backup verification failed"
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            consecutive_failures += 1;
                            error!(error = %e, consecutive_failures, "scheduled backup failed");
                        }
                    }
                }
            }
        }
    });

    (token, handle)
}

/// Run integrity_check + restore_test on a freshly-written backup.
/// Returns the restore report on success, or a descriptive error string.
async fn verify_backup(
    dest: &Path,
    encryption_key: Option<[u8; 32]>,
) -> Result<kleos_lib::db::backup::RestoreReport, String> {
    let errors = kleos_lib::db::backup::integrity_check(dest, encryption_key)
        .await
        .map_err(|e| format!("integrity_check errored: {e}"))?;
    if !errors.is_empty() {
        return Err(format!("integrity_check reported issues: {errors:?}"));
    }
    kleos_lib::db::backup::restore_test(dest, encryption_key)
        .await
        .map_err(|e| format!("restore_test failed: {e}"))
}

/// Promote `src` to `daily_dir` (if no daily backup exists for today) and
/// prune daily backups beyond retention. Returns (promoted_path, pruned_count).
fn promote_and_prune_daily(
    src: &Path,
    daily_dir: &Path,
    retention_daily: usize,
) -> (Option<PathBuf>, usize) {
    let promoted = match promote_to_daily(src, daily_dir) {
        Ok(p) => p,
        Err(e) => {
            warn!(src = %src.display(), error = %e, "daily promotion failed");
            None
        }
    };
    let pruned = prune_backups(daily_dir, retention_daily);
    (promoted, pruned)
}

/// R8 R-010: evict idle SessionBroadcast entries once per minute.
///
/// A tenant can create up to `MAX_SESSIONS_PER_USER=64` sessions; without a
/// time-based reaper they live in the map until the process restarts. We
/// consider an entry stale when it has not been appended to for `ttl_ms` AND
/// has zero live websocket subscribers, so active streams are never evicted
/// out from under a consumer.
///
/// Returns the number of entries removed so callers (and tests) can verify.
pub async fn reap_stale_sessions(
    sessions: &crate::state::SessionMap,
    now_ms: u64,
    ttl_ms: u64,
) -> usize {
    let stale: Vec<(i64, String)> = {
        let map = sessions.read().await;
        let mut out = Vec::new();
        for (key, bcast) in map.iter() {
            let b = bcast.lock().await;
            let idle =
                now_ms.saturating_sub(b.last_activity.load(std::sync::atomic::Ordering::Relaxed));
            if idle > ttl_ms && b.tx.receiver_count() == 0 {
                out.push(key.clone());
            }
        }
        out
    };
    if stale.is_empty() {
        return 0;
    }
    let count = stale.len();
    let mut map = sessions.write().await;
    for k in &stale {
        map.remove(k);
    }
    count
}

/// Reaps expired sessions periodically.
pub fn start_session_reaper_task(
    sessions: crate::state::SessionMap,
) -> (CancellationToken, tokio::task::JoinHandle<()>) {
    let token = CancellationToken::new();
    let cancel = token.clone();

    let handle = tokio::spawn(async move {
        const SCAN_INTERVAL: Duration = Duration::from_secs(60);
        const TTL_MS: u64 = 60 * 60 * 1000; // 1 hour idle

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("session reaper task shutting down");
                    break;
                }
                _ = tokio::time::sleep(SCAN_INTERVAL) => {
                    let now = crate::dreamer::monotonic_millis();
                    let removed = reap_stale_sessions(&sessions, now, TTL_MS).await;
                    if removed > 0 {
                        info!(count = removed, "session reaper evicted stale entries");
                    }
                }
            }
        }
    });

    (token, handle)
}

/// Sweeps all tenants for stale Chiasm tasks every 60 seconds.
///
/// A task is stale when its last heartbeat exceeds `heartbeat_interval * 2`
/// seconds ago. Stale tasks are moved to the "stale" status and their path
/// claims are released.
pub fn start_stale_task_sweeper(
    db: Arc<Database>,
    registry: Option<Arc<TenantRegistry>>,
) -> (CancellationToken, tokio::task::JoinHandle<()>) {
    let token = CancellationToken::new();
    let cancel = token.clone();

    let handle = tokio::spawn(async move {
        let interval = Duration::from_secs(60);
        // Idle window (seconds) after which a never-heartbeated active task is
        // staled. Read once; default 1 hour. Tasks created via `activity
        // task.started` never heartbeat, so this absolute window is what keeps
        // abandoned ones from accumulating forever in the active set.
        let no_hb_idle: i64 = std::env::var("KLEOS_CHIASM_STALE_IDLE_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3600);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("stale-task sweeper shutting down");
                    break;
                }
                _ = tokio::time::sleep(interval) => {
                    if let Some(ref reg) = registry {
                        let tenants = match reg.list() {
                            Ok(t) => t,
                            Err(e) => {
                                warn!(error = %e, "stale-task sweeper: failed to list tenants");
                                continue;
                            }
                        };
                        for tenant_row in tenants {
                            if tenant_row.status != kleos_lib::tenant::TenantStatus::Active {
                                continue;
                            }
                            let handle = match reg.get(&tenant_row.user_id).await {
                                Ok(Some(h)) => h,
                                _ => continue,
                            };
                            if let Err(e) = kleos_lib::services::chiasm::heartbeat::mark_stale_tasks(&handle.db, 2.0, no_hb_idle).await {
                                warn!(tenant = %tenant_row.tenant_id, error = %e, "stale-task sweep error");
                            }
                        }
                    } else {
                        if let Err(e) = kleos_lib::services::chiasm::heartbeat::mark_stale_tasks(&db, 2.0, no_hb_idle).await {
                            warn!(error = %e, "stale-task sweep error");
                        }
                    }
                }
            }
        }
    });

    (token, handle)
}

/// Prunes expired Axon events hourly based on each channel's retain_hours.
pub fn start_event_retention_task(
    db: Arc<Database>,
    registry: Option<Arc<TenantRegistry>>,
) -> (CancellationToken, tokio::task::JoinHandle<()>) {
    let token = CancellationToken::new();
    let cancel = token.clone();

    let handle = tokio::spawn(async move {
        let interval = Duration::from_secs(3600);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("event-retention task shutting down");
                    break;
                }
                _ = tokio::time::sleep(interval) => {
                    if let Some(ref reg) = registry {
                        let tenants = match reg.list() {
                            Ok(t) => t,
                            Err(e) => {
                                warn!(error = %e, "event-retention: failed to list tenants");
                                continue;
                            }
                        };
                        for tenant_row in tenants {
                            if tenant_row.status != kleos_lib::tenant::TenantStatus::Active {
                                continue;
                            }
                            let handle = match reg.get(&tenant_row.user_id).await {
                                Ok(Some(h)) => h,
                                _ => continue,
                            };
                            match kleos_lib::services::axon::retention::prune_expired_events(&handle.db).await {
                                Ok(n) if n > 0 => info!(tenant = %tenant_row.tenant_id, pruned = n, "axon retention: pruned events"),
                                Err(e) => warn!(tenant = %tenant_row.tenant_id, error = %e, "axon retention error"),
                                _ => {}
                            }
                        }
                    } else {
                        match kleos_lib::services::axon::retention::prune_expired_events(&db).await {
                            Ok(n) if n > 0 => info!(pruned = n, "axon retention: pruned events"),
                            Err(e) => warn!(error = %e, "axon retention error"),
                            _ => {}
                        }
                    }
                }
            }
        }
    });

    (token, handle)
}

/// Prunes service dead-letter rows hourly once they age past the retention
/// window. The `service_dead_letters` table lives on the monolith/system DB
/// only (migration 22; tenant shards never create it), so unlike the event
/// retention task there is no per-shard sweep. Retention defaults to
/// [`kleos_lib::resilience::dead_letter::DEFAULT_RETAIN_HOURS`] and can be
/// tuned per deployment via `KLEOS_DEAD_LETTER_RETAIN_HOURS`.
pub fn start_dead_letter_retention_task(
    db: Arc<Database>,
) -> (CancellationToken, tokio::task::JoinHandle<()>) {
    let token = CancellationToken::new();
    let cancel = token.clone();

    let retain_hours = std::env::var("KLEOS_DEAD_LETTER_RETAIN_HOURS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|h| *h > 0)
        .unwrap_or(kleos_lib::resilience::dead_letter::DEFAULT_RETAIN_HOURS);

    let handle = tokio::spawn(async move {
        let interval = Duration::from_secs(3600);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("dead-letter-retention task shutting down");
                    break;
                }
                _ = tokio::time::sleep(interval) => {
                    match kleos_lib::resilience::prune_dead_letters(&db, retain_hours).await {
                        Ok(n) if n > 0 => info!(pruned = n, "dead-letter retention: pruned rows"),
                        Err(e) => warn!(error = %e, "dead-letter retention error"),
                        _ => {}
                    }
                }
            }
        }
    });

    (token, handle)
}

/// Unit tests.
#[cfg(test)]
mod tests {
    use super::*;

    /// Test: an absolute backup dir path is returned unchanged.
    #[test]
    fn resolve_backup_dir_absolute_passes_through() {
        let abs = if cfg!(windows) {
            "C:/tmp/bk"
        } else {
            "/tmp/bk"
        };
        assert_eq!(resolve_backup_dir("/var/data", abs), PathBuf::from(abs));
    }

    /// Test: a relative backup dir path is joined to the data directory.
    #[test]
    fn resolve_backup_dir_relative_joins_data_dir() {
        assert_eq!(
            resolve_backup_dir("/var/data", "backups"),
            PathBuf::from("/var/data/backups")
        );
    }

    /// Test: list_backups returns only prefixed backup files sorted by name.
    #[test]
    fn list_backups_filters_by_prefix_and_sorts() {
        let dir = std::env::temp_dir().join(format!("kleos-backups-list-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        // Create files out of order; list_backups should sort them.
        for name in [
            "kleos-backup-20260101-000000.db",
            "kleos-backup-20260103-000000.db",
            "kleos-backup-20260102-000000.db",
            "junk.db",
            "not-a-backup.txt",
        ] {
            std::fs::write(dir.join(name), b"").unwrap();
        }
        let listed = list_backups(&dir);
        assert_eq!(listed.len(), 3);
        assert!(listed[0].ends_with("kleos-backup-20260101-000000.db"));
        assert!(listed[2].ends_with("kleos-backup-20260103-000000.db"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Test: date component is correctly parsed from valid backup filenames.
    #[test]
    fn backup_date_component_parses_valid_names() {
        assert_eq!(
            backup_date_component(Path::new("kleos-backup-20260101-120000.db")),
            Some("20260101".into())
        );
        assert_eq!(
            backup_date_component(Path::new("/x/kleos-backup-20261231-235959.db")),
            Some("20261231".into())
        );
    }

    /// Test: malformed backup filenames return None from backup_date_component.
    #[test]
    fn backup_date_component_rejects_malformed_names() {
        assert_eq!(
            backup_date_component(Path::new("kleos-backup-notadate-xxxx.db")),
            None
        );
        assert_eq!(backup_date_component(Path::new("junk.db")), None);
        assert_eq!(backup_date_component(Path::new("kleos-backup-.db")), None);
    }

    /// Test: the first backup of a date is copied into the daily directory.
    #[test]
    fn promote_to_daily_copies_first_backup_of_date() {
        let dir = std::env::temp_dir().join(format!("kleos-promote-{}", uuid::Uuid::new_v4()));
        let daily = dir.join("daily");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("kleos-backup-20260415-120000.db");
        std::fs::write(&src, b"hello").unwrap();
        let result = promote_to_daily(&src, &daily).expect("promote");
        let promoted = result.expect("first promotion copies");
        assert!(promoted.exists());
        assert!(promoted.ends_with("kleos-backup-20260415-120000.db"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Test: a date with an existing daily entry is not promoted again.
    #[test]
    fn promote_to_daily_skips_when_date_already_promoted() {
        let dir = std::env::temp_dir().join(format!("kleos-promote2-{}", uuid::Uuid::new_v4()));
        let daily = dir.join("daily");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&daily).unwrap();
        // An earlier hourly from the same UTC date already sits in daily/
        std::fs::write(daily.join("kleos-backup-20260415-060000.db"), b"earlier").unwrap();
        let src = dir.join("kleos-backup-20260415-180000.db");
        std::fs::write(&src, b"later").unwrap();
        let result = promote_to_daily(&src, &daily).expect("promote");
        assert!(result.is_none(), "should not re-promote same date");
        // The earlier file is still there; the later one was NOT copied.
        assert!(daily.join("kleos-backup-20260415-060000.db").exists());
        assert!(!daily.join("kleos-backup-20260415-180000.db").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Test: prune_backups deletes the oldest files when count exceeds retention limit.
    #[test]
    fn prune_backups_removes_oldest_beyond_retention() {
        let dir =
            std::env::temp_dir().join(format!("kleos-backups-prune-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in [
            "kleos-backup-20260101-000000.db",
            "kleos-backup-20260102-000000.db",
            "kleos-backup-20260103-000000.db",
            "kleos-backup-20260104-000000.db",
        ] {
            std::fs::write(dir.join(name), b"").unwrap();
        }
        let deleted = prune_backups(&dir, 2);
        assert_eq!(deleted, 2);
        let remaining = list_backups(&dir);
        assert_eq!(remaining.len(), 2);
        assert!(remaining[0].ends_with("kleos-backup-20260103-000000.db"));
        assert!(remaining[1].ends_with("kleos-backup-20260104-000000.db"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// R8 R-010: reaper must evict idle, zero-subscriber entries and leave
    /// both fresh entries and entries with live subscribers alone.
    #[tokio::test]
    async fn reap_stale_sessions_evicts_only_idle_and_unsubscribed() {
        use crate::state::SessionBroadcast;
        use std::collections::HashMap;
        use std::sync::atomic::Ordering;
        use std::sync::Arc;
        use tokio::sync::{Mutex, RwLock};

        let sessions: crate::state::SessionMap = Arc::new(RwLock::new(HashMap::new()));

        // Stale + no subscribers -> should be removed.
        let stale_idle = SessionBroadcast::new();
        stale_idle.last_activity.store(0, Ordering::Relaxed);

        // Stale + live subscriber -> keep (someone is streaming).
        let stale_busy = SessionBroadcast::new();
        stale_busy.last_activity.store(0, Ordering::Relaxed);
        let subscriber = stale_busy.tx.subscribe();
        assert_eq!(stale_busy.tx.receiver_count(), 1);

        // Fresh entry -> keep. last_activity is 30s ago, well under the 1h TTL.
        let fresh = SessionBroadcast::new();
        fresh.last_activity.store(9_970_000, Ordering::Relaxed);

        {
            let mut map = sessions.write().await;
            map.insert(
                (1, "stale-idle".to_string()),
                Arc::new(Mutex::new(stale_idle)),
            );
            map.insert(
                (1, "stale-busy".to_string()),
                Arc::new(Mutex::new(stale_busy)),
            );
            map.insert((1, "fresh".to_string()), Arc::new(Mutex::new(fresh)));
        }

        let now_ms = 10_000_000;
        let ttl_ms = 60 * 60 * 1000;
        let removed = reap_stale_sessions(&sessions, now_ms, ttl_ms).await;
        assert_eq!(removed, 1);

        let map = sessions.read().await;
        assert!(!map.contains_key(&(1, "stale-idle".to_string())));
        assert!(map.contains_key(&(1, "stale-busy".to_string())));
        assert!(map.contains_key(&(1, "fresh".to_string())));
        // Keep the subscriber alive past the reaper scan so its presence is
        // actually reflected in tx.receiver_count().
        drop(subscriber);
    }

    /// Sharded mode: jobs enqueued into a tenant shard's private jobs table
    /// must be drained by the worker. Pre-fix, only the monolith queue was
    /// polled and shard jobs (e.g. ingestion.fact_extract enqueued via the
    /// per-request ResolvedDb) sat pending forever.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn worker_drains_resident_tenant_shard_jobs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = Arc::new(
            kleos_lib::tenant::TenantRegistry::new(
                dir.path(),
                kleos_lib::tenant::TenantConfig::default(),
                128,
                false,
                None,
            )
            .expect("registry"),
        );
        let tenant = registry.get_or_create("4242").await.expect("tenant");

        kleos_lib::jobs::register_job_handler("test.shard_drain", |_payload| async move { Ok(()) })
            .await;
        kleos_lib::jobs::enqueue_job(&tenant.db, "test.shard_drain", "{}", 3)
            .await
            .expect("enqueue into shard");

        let monolith = Arc::new(
            kleos_lib::db::Database::connect_memory()
                .await
                .expect("monolith db"),
        );
        let (cancel, handle) = start_job_worker_task(monolith, Some(Arc::clone(&registry)));

        // The resident-shard drain runs on the 200ms poll cycle; allow a
        // generous deadline for slow CI machines.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            let stats = kleos_lib::jobs::get_job_stats(&tenant.db)
                .await
                .expect("shard job stats");
            if stats.completed >= 1 {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "shard job was never drained: {stats:?}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        cancel.cancel();
        let _ = handle.await;
    }
}
