//! Background tasks that run on a timer for the duration of the server process.

use engram_lib::db::Database;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

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
pub fn start_job_cleanup_task(db: Arc<Database>) -> CancellationToken {
    let token = CancellationToken::new();
    let cancel = token.clone();

    tokio::spawn(async move {
        let interval = Duration::from_secs(3600);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("job-cleanup task shutting down");
                    break;
                }
                _ = tokio::time::sleep(interval) => {
                    match engram_lib::jobs::cleanup_completed_jobs(&db.conn).await {
                        Ok(n) => info!(deleted = n, "job cleanup complete"),
                        Err(e) => error!(error = %e, "job cleanup failed"),
                    }
                }
            }
        }
    });

    token
}

/// Replays failed LanceDB vector sync operations on a 10-minute interval.
/// Skips silently when no vector index is configured.
pub fn start_vector_sync_replay_task(db: Arc<Database>) -> CancellationToken {
    let token = CancellationToken::new();
    let cancel = token.clone();

    tokio::spawn(async move {
        let interval = Duration::from_secs(600);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("vector-sync-replay task shutting down");
                    break;
                }
                _ = tokio::time::sleep(interval) => {
                    match engram_lib::memory::replay_vector_sync_pending(&db, 100).await {
                        Ok(report) => {
                            if report.processed > 0 {
                                info!(
                                    processed = report.processed,
                                    succeeded = report.succeeded,
                                    failed = report.failed,
                                    skipped = report.skipped,
                                    "vector sync replay complete"
                                );
                            }
                        }
                        Err(e) => error!(error = %e, "vector sync replay failed"),
                    }
                }
            }
        }
    });

    token
}
