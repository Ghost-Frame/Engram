//! Background tasks that run on a timer for the duration of the server process.

use engram_lib::db::Database;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

/// Cap for exponential backoff: 5 minutes.
const MAX_BACKOFF: Duration = Duration::from_secs(300);

/// Runs a WAL checkpoint on a 5-minute interval.
/// Uses PASSIVE mode so readers are never blocked.
/// TRUNCATE mode is used once at startup to shrink any large WAL leftover from
/// a previous run.
pub fn start_auto_checkpoint_task(db: Arc<Database>) -> CancellationToken {
    let token = CancellationToken::new();
    let cancel = token.clone();

    tokio::spawn(async move {
        // Startup TRUNCATE: flush any WAL accumulated before this process started.
        match engram_lib::db::backup::wal_checkpoint(
            &db,
            engram_lib::db::backup::CheckpointMode::Truncate,
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
                    match engram_lib::db::backup::wal_checkpoint(
                        &db,
                        engram_lib::db::backup::CheckpointMode::Passive,
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

    token
}

/// Deletes completed jobs older than 1 hour on an hourly interval.
/// RB-L5: failures back off exponentially (doubling each time, capped at 5 min).
pub fn start_job_cleanup_task(db: Arc<Database>) -> CancellationToken {
    let token = CancellationToken::new();
    let cancel = token.clone();

    tokio::spawn(async move {
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
                    match engram_lib::jobs::cleanup_completed_jobs(&db.conn).await {
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
                }
            }
        }
    });

    token
}

/// Replays failed LanceDB vector sync operations on a 10-minute interval.
/// Skips silently when no vector index is configured.
/// RB-L5: failures back off exponentially (doubling each time, capped at 5 min).
/// MT-F17: per-user round-robin scheduling prevents a single user with many
/// pending rows from starving other users. A monotonic sequence counter tracks
/// when each user was last served; the user with the lowest counter (i.e. served
/// least recently, or never) is chosen each tick.
pub fn start_vector_sync_replay_task(db: Arc<Database>) -> CancellationToken {
    let token = CancellationToken::new();
    let cancel = token.clone();

    tokio::spawn(async move {
        let base_interval = Duration::from_secs(600);
        let mut consecutive_failures: u32 = 0;
        // MT-F17: last-served sequence number per user (lower = served longer ago).
        let mut last_served: HashMap<i64, u64> = HashMap::new();
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
                    // Discover which users have pending work this tick.
                    let user_ids = match engram_lib::memory::vector_sync_pending_users(&db).await {
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
                        .into_iter()
                        .min_by_key(|uid| last_served.get(uid).copied().unwrap_or(0))
                        .expect("non-empty vec has a minimum");

                    match engram_lib::memory::replay_vector_sync_pending_for_user(
                        &db,
                        next_user,
                        100,
                    )
                    .await
                    {
                        Ok(report) => {
                            consecutive_failures = 0;
                            serve_seq += 1;
                            last_served.insert(next_user, serve_seq);
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
                }
            }
        }
    });

    token
}
