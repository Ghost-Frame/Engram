// Background PageRank refresh job. Runs on a configurable interval and
// recomputes scores for any user whose dirty_count has crossed the threshold
// or whose last_refresh is older than the interval.
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::db::Database;
use crate::graph::pagerank::{
    compute_pagerank_for_user, persist_pagerank_with_snapshot, snapshot_pagerank_dirty,
};

/// Query users whose pagerank cache needs refreshing based on dirty_count or
/// elapsed time since last_refresh.
async fn dirty_users(
    db: &Database,
    threshold: u32,
    interval_secs: u64,
) -> crate::Result<Vec<i64>> {
    let threshold_i64 = threshold as i64;
    let interval_i64 = interval_secs as i64;
    let sql = format!(
        "SELECT user_id FROM pagerank_dirty \
         WHERE dirty_count >= ?1 \
            OR last_refresh <= strftime('%s','now') - {interval_i64}",
    );
    let mut rows = db
        .connection()
        .query(&sql, libsql::params![threshold_i64])
        .await?;
    let mut user_ids = Vec::new();
    while let Some(row) = rows.next().await? {
        user_ids.push(row.get::<i64>(0)?);
    }
    Ok(user_ids)
}

/// Run a single refresh cycle: find dirty users, recompute + persist (bounded
/// by the concurrency semaphore).
async fn run_once(db: &Arc<Database>, config: &Config) -> crate::Result<usize> {
    let users = dirty_users(
        db.as_ref(),
        config.pagerank_dirty_threshold,
        config.pagerank_refresh_interval_secs,
    )
    .await?;
    if users.is_empty() {
        return Ok(0);
    }

    let sem = Arc::new(Semaphore::new(config.pagerank_max_concurrent));
    let mut handles = Vec::with_capacity(users.len());

    for user_id in users {
        let db_arc = Arc::clone(db);
        let sem_arc = Arc::clone(&sem);
        handles.push(tokio::spawn(async move {
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
        }));
    }

    let mut refreshed = 0usize;
    for h in handles {
        match h.await {
            Ok(true) => refreshed += 1,
            Ok(false) => {}
            Err(e) => error!(error = %e, "pagerank task panicked"),
        }
    }
    Ok(refreshed)
}

/// Spawn the background refresh loop. Returns a `CancellationToken` that,
/// when cancelled, causes the loop to exit cleanly after its current cycle.
pub fn start_pagerank_refresh_job(db: Arc<Database>, config: Arc<Config>) -> CancellationToken {
    let token = CancellationToken::new();
    let cancel = token.clone();
    let interval =
        std::time::Duration::from_secs(config.pagerank_refresh_interval_secs.max(10));

    tokio::spawn(async move {
        info!(
            interval_secs = config.pagerank_refresh_interval_secs,
            "pagerank refresh job started"
        );
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("pagerank refresh job shutting down");
                    break;
                }
                _ = tokio::time::sleep(interval) => {
                    match run_once(&db, &config).await {
                        Ok(n) if n > 0 => info!(users_refreshed = n, "pagerank batch complete"),
                        Ok(_) => {}
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
        let mut rows = db
            .connection()
            .query(
                "SELECT COUNT(*) FROM memory_pagerank WHERE user_id = ?1",
                libsql::params![user_id],
            )
            .await
            .expect("query pagerank count");
        rows.next()
            .await
            .expect("read count row")
            .expect("count row exists")
            .get(0)
            .expect("count value")
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

        let refreshed = run_once(&db, &config).await.expect("run refresh cycle");

        assert_eq!(refreshed, 1);
        assert_eq!(pagerank_count(db.as_ref(), user_id).await, created);
    }
}
