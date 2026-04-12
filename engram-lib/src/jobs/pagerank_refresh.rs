// Background PageRank refresh job. Runs on a configurable interval and
// recomputes scores for any user whose dirty_count has crossed the threshold
// or whose last_refresh is older than the interval.
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::db::Database;
use crate::graph::pagerank::{
    compute_pagerank_for_user, persist_pagerank_with_snapshot, snapshot_pagerank_dirty,
};
use crate::EngError;

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

/// Query users whose pagerank cache needs refreshing based on dirty_count or
/// elapsed time since last_refresh.
async fn dirty_users(db: &Database, threshold: u32, interval_secs: u64) -> crate::Result<Vec<i64>> {
    let threshold_i64 = threshold as i64;
    let interval_i64 = interval_secs as i64;
    db.read(move |conn| {
        let sql = format!(
            "SELECT user_id FROM pagerank_dirty \
             WHERE dirty_count >= ?1 \
                OR last_refresh <= strftime('%s','now') - {interval_i64}",
        );
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let rows = stmt
            .query_map(rusqlite::params![threshold_i64], |row| row.get(0))
            .map_err(rusqlite_to_eng_error)?;
        rows.collect::<std::result::Result<Vec<i64>, _>>()
            .map_err(rusqlite_to_eng_error)
    })
    .await
}

/// Run a single refresh cycle: find dirty users, recompute + persist (bounded
/// by the concurrency semaphore). Returns per-user (user_id, success) outcomes.
async fn run_once(
    db: &Arc<Database>,
    config: &Config,
    skip_until: &HashMap<i64, Instant>,
) -> crate::Result<Vec<(i64, bool)>> {
    let now = Instant::now();
    let all_users = dirty_users(
        db.as_ref(),
        config.pagerank_dirty_threshold,
        config.pagerank_refresh_interval_secs,
    )
    .await?;

    // Skip users that are still in their backoff window.
    let users: Vec<i64> = all_users
        .into_iter()
        .filter(|uid| skip_until.get(uid).map(|&t| now >= t).unwrap_or(true))
        .collect();

    if users.is_empty() {
        return Ok(Vec::new());
    }

    let sem = Arc::new(Semaphore::new(config.pagerank_max_concurrent));
    let mut handles: Vec<(i64, _)> = Vec::with_capacity(users.len());

    for user_id in users {
        let db_arc = Arc::clone(db);
        let sem_arc = Arc::clone(&sem);
        let handle = tokio::spawn(async move {
            // Acquire before doing work so at most max_concurrent tasks compute at once.
            let _permit = sem_arc.acquire_owned().await;
            // Snapshot dirty_count BEFORE compute. Any mark_pagerank_dirty
            // that fires while compute is in flight will not be cleared by
            // persist_pagerank_with_snapshot below, so the next refresh
            // cycle picks it up instead of silently dropping it.
            let dirty_snapshot = match snapshot_pagerank_dirty(db_arc.as_ref(), user_id).await {
                Ok(n) => n,
                Err(e) => {
                    warn!(user_id, error = %e, "pagerank dirty snapshot failed");
                    return false;
                }
            };
            match compute_pagerank_for_user(db_arc.as_ref(), user_id).await {
                Ok(scores) => {
                    if let Err(e) = persist_pagerank_with_snapshot(
                        db_arc.as_ref(),
                        user_id,
                        &scores,
                        dirty_snapshot,
                    )
                    .await
                    {
                        warn!(user_id, error = %e, "pagerank persist failed");
                        return false;
                    }
                    info!(user_id, scores = scores.len(), "pagerank refreshed");
                    true
                }
                Err(e) => {
                    warn!(user_id, error = %e, "pagerank compute failed");
                    false
                }
            }
        });
        handles.push((user_id, handle));
    }

    let mut outcomes = Vec::with_capacity(handles.len());
    for (user_id, h) in handles {
        match h.await {
            Ok(success) => outcomes.push((user_id, success)),
            Err(e) => {
                error!(user_id, error = %e, "pagerank task panicked");
                outcomes.push((user_id, false));
            }
        }
    }
    Ok(outcomes)
}

/// Spawn the background refresh loop. Returns a `CancellationToken` that,
/// when cancelled, causes the loop to exit cleanly after its current cycle.
///
/// MT-F16: per-user exponential backoff on persistent failure. A user that
/// fails N consecutive times is skipped for `2^min(N,6)` minutes before the
/// next attempt.
pub fn start_pagerank_refresh_job(db: Arc<Database>, config: Arc<Config>) -> CancellationToken {
    let token = CancellationToken::new();
    let cancel = token.clone();
    let interval = Duration::from_secs(config.pagerank_refresh_interval_secs.max(10));

    tokio::spawn(async move {
        info!(
            interval_secs = config.pagerank_refresh_interval_secs,
            "pagerank refresh job started"
        );
        // per-user failure counts and retry-after instants
        let mut failure_counts: HashMap<i64, u32> = HashMap::new();
        let mut skip_until: HashMap<i64, Instant> = HashMap::new();

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("pagerank refresh job shutting down");
                    break;
                }
                _ = tokio::time::sleep(interval) => {
                    match run_once(&db, &config, &skip_until).await {
                        Ok(outcomes) => {
                            let refreshed = outcomes.iter().filter(|(_, ok)| *ok).count();
                            if refreshed > 0 {
                                info!(users_refreshed = refreshed, "pagerank batch complete");
                            }
                            let now = Instant::now();
                            for (user_id, success) in outcomes {
                                if success {
                                    failure_counts.remove(&user_id);
                                    skip_until.remove(&user_id);
                                } else {
                                    let failures = failure_counts.entry(user_id).or_insert(0);
                                    *failures += 1;
                                    let backoff_mins = 2u64.pow((*failures).min(6));
                                    let retry_at = now + Duration::from_secs(backoff_mins * 60);
                                    skip_until.insert(user_id, retry_at);
                                    warn!(
                                        user_id,
                                        failures = *failures,
                                        backoff_mins,
                                        "pagerank backoff applied"
                                    );
                                }
                            }
                        }
                        Err(e) => error!(error = %e, "pagerank refresh cycle failed"),
                    }
                }
            }
        }
    });

    token
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory;
    use crate::memory::types::StoreRequest;

    fn store_request(content: &str, user_id: i64) -> StoreRequest {
        StoreRequest {
            content: content.to_string(),
            category: "test".to_string(),
            source: "test".to_string(),
            importance: 5,
            tags: None,
            embedding: None,
            session_id: None,
            is_static: None,
            user_id: Some(user_id),
            space_id: None,
            parent_memory_id: None,
        }
    }

    async fn pagerank_count(db: &Database, user_id: i64) -> i64 {
        db.read(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM memory_pagerank WHERE user_id = ?1",
                rusqlite::params![user_id],
                |row| row.get(0),
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))
        })
        .await
        .expect("query pagerank count")
    }

    #[tokio::test]
    async fn run_once_populates_pagerank_for_dirty_user() {
        let db = Arc::new(Database::connect_memory().await.expect("in-memory db"));
        let user_id = 1;
        let mut created = 0_i64;

        for i in 0..100 {
            let content = format!(
                "background refresh node_{i} edge_{} branch_{} ring_{}",
                i * 19,
                i * 29,
                i * 37
            );
            let stored = memory::store(db.as_ref(), store_request(&content, user_id))
                .await
                .expect("store memory");
            if stored.created {
                created += 1;
            }
        }

        let config = Config {
            pagerank_dirty_threshold: 100,
            pagerank_refresh_interval_secs: 300,
            pagerank_max_concurrent: 2,
            ..Config::default()
        };

        let skip_until = std::collections::HashMap::new();
        let outcomes = run_once(&db, &config, &skip_until)
            .await
            .expect("run refresh cycle");
        let refreshed = outcomes.iter().filter(|(_, ok)| *ok).count();

        assert_eq!(refreshed, 1);
        assert_eq!(pagerank_count(db.as_ref(), user_id).await, created);
    }
}
