// Background PageRank refresh job for tenant-sharded architecture.
// Iterates over all tenants and recomputes scores based on dirty state.

use crate::config::Config;
use crate::db::Database;
use crate::tenant::TenantRegistry;
use crate::{EngError, Result};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::validation::MAX_PAGERANK_ITERATIONS;

const DAMPING: f64 = 0.85;
const MAX_ITERATIONS: u32 = MAX_PAGERANK_ITERATIONS as u32;
const CONVERGENCE_THRESHOLD: f64 = 1e-6;

/// Compute PageRank scores for a tenant database.
/// Since tenants are isolated, no user_id scoping is needed.
async fn compute_pagerank_for_tenant(db: &Database) -> Result<Vec<(i64, f64)>> {
    let memories: Vec<i64> = db
        .read(|conn| {
            let mut stmt = conn
                .prepare("SELECT id FROM memories WHERE is_forgotten = 0 AND is_latest = 1")
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            let rows = stmt
                .query_map([], |r| r.get::<_, i64>(0))
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))
        })
        .await?;

    if memories.is_empty() {
        return Ok(Vec::new());
    }

    let memory_count = memories.len();
    let base_score = 1.0 / memory_count as f64;

    let links: Vec<(i64, i64)> = db
        .read(|conn| {
            let mut stmt = conn
                .prepare("SELECT source_id, target_id FROM memory_links")
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))
        })
        .await?;

    let id_to_idx: std::collections::HashMap<i64, usize> = memories
        .iter()
        .enumerate()
        .map(|(i, &id)| (id, i))
        .collect();

    let mut out_degree = vec![0usize; memory_count];
    let mut incoming: Vec<Vec<usize>> = vec![Vec::new(); memory_count];

    for (src, tgt) in &links {
        if let (Some(&src_idx), Some(&tgt_idx)) = (id_to_idx.get(src), id_to_idx.get(tgt)) {
            out_degree[src_idx] += 1;
            incoming[tgt_idx].push(src_idx);
        }
    }

    let mut scores = vec![base_score; memory_count];
    let mut next_scores = vec![0.0; memory_count];

    for _ in 0..MAX_ITERATIONS {
        let teleport = (1.0 - DAMPING) / memory_count as f64;

        let dangling_sum: f64 = scores
            .iter()
            .enumerate()
            .filter(|(i, _)| out_degree[*i] == 0)
            .map(|(_, s)| s)
            .sum();
        let dangling_contrib = DAMPING * dangling_sum / memory_count as f64;

        for i in 0..memory_count {
            let link_contrib: f64 = incoming[i]
                .iter()
                .map(|&j| scores[j] / out_degree[j] as f64)
                .sum();

            next_scores[i] = teleport + dangling_contrib + DAMPING * link_contrib;
        }

        let delta: f64 = scores
            .iter()
            .zip(next_scores.iter())
            .map(|(a, b)| (a - b).abs())
            .sum();

        std::mem::swap(&mut scores, &mut next_scores);

        if delta < CONVERGENCE_THRESHOLD {
            break;
        }
    }

    Ok(memories.into_iter().zip(scores).collect())
}

/// Persist PageRank scores to a tenant database.
async fn persist_pagerank_for_tenant(db: &Database, scores: Vec<(i64, f64)>) -> Result<()> {
    db.transaction(move |tx| {
        for (memory_id, score) in &scores {
            let in_degree: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM memory_links WHERE target_id = ?1",
                    [memory_id],
                    |row| row.get(0),
                )
                .unwrap_or(0);

            let out_degree: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM memory_links WHERE source_id = ?1",
                    [memory_id],
                    |row| row.get(0),
                )
                .unwrap_or(0);

            tx.execute(
                "INSERT INTO memory_pagerank (memory_id, score, in_degree, out_degree, computed_at)
                 VALUES (?1, ?2, ?3, ?4, datetime('now'))
                 ON CONFLICT(memory_id) DO UPDATE SET
                     score = excluded.score,
                     in_degree = excluded.in_degree,
                     out_degree = excluded.out_degree,
                     computed_at = excluded.computed_at",
                rusqlite::params![memory_id, score, in_degree, out_degree],
            )
            .map_err(|e| EngError::Internal(format!("pagerank upsert failed: {}", e)))?;

            tx.execute(
                "UPDATE memories SET pagerank_score = ?1 WHERE id = ?2",
                rusqlite::params![score, memory_id],
            )
            .map_err(|e| EngError::Internal(format!("memory pagerank update failed: {}", e)))?;
        }
        Ok(())
    })
    .await
}

async fn get_memory_count(db: &Database) -> Result<i64> {
    db.read(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM memories WHERE is_forgotten = 0 AND is_latest = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))
    })
    .await
}

async fn get_pagerank_count(db: &Database) -> Result<i64> {
    db.read(|conn| {
        conn.query_row("SELECT COUNT(*) FROM memory_pagerank", [], |row| row.get(0))
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))
    })
    .await
}

async fn needs_refresh(db: &Database, threshold: u32) -> Result<bool> {
    let memory_count = get_memory_count(db).await?;
    let pagerank_count = get_pagerank_count(db).await?;

    let diff = (memory_count - pagerank_count).unsigned_abs();
    Ok(diff >= threshold as u64)
}

async fn refresh_tenant(handle: Arc<crate::tenant::TenantHandle>, threshold: u32) -> bool {
    let db = &handle.db;

    let should_refresh = match needs_refresh(db.as_ref(), threshold).await {
        Ok(true) => true,
        Ok(false) => return true,
        Err(e) => {
            warn!(tenant_id = %handle.tenant_id, error = %e, "failed to check refresh state");
            return false;
        }
    };

    if !should_refresh {
        return true;
    }

    match compute_pagerank_for_tenant(db.as_ref()).await {
        Ok(scores) if scores.is_empty() => true,
        Ok(scores) => {
            let count = scores.len();
            if let Err(e) = persist_pagerank_for_tenant(db.as_ref(), scores).await {
                warn!(tenant_id = %handle.tenant_id, error = %e, "pagerank persist failed");
                return false;
            }
            info!(tenant_id = %handle.tenant_id, scores = count, "pagerank refreshed");
            true
        }
        Err(e) => {
            warn!(tenant_id = %handle.tenant_id, error = %e, "pagerank compute failed");
            false
        }
    }
}

async fn run_once(registry: &Arc<TenantRegistry>, config: &Config) -> Result<usize> {
    let tenants = registry.list()?;
    if tenants.is_empty() {
        return Ok(0);
    }

    let sem = Arc::new(Semaphore::new(config.pagerank_max_concurrent));
    let mut handles = Vec::with_capacity(tenants.len());

    for tenant_row in tenants {
        if tenant_row.status != crate::tenant::TenantStatus::Active {
            continue;
        }

        let registry_arc = Arc::clone(registry);
        let sem_arc = Arc::clone(&sem);
        let threshold = config.pagerank_dirty_threshold;
        let user_id = tenant_row.user_id.clone();

        handles.push(tokio::spawn(async move {
            let _permit = sem_arc.acquire_owned().await;

            let handle = match registry_arc.get(&user_id).await {
                Ok(Some(h)) => h,
                Ok(None) => {
                    warn!(user_id = %user_id, "tenant not found during refresh");
                    return false;
                }
                Err(e) => {
                    warn!(user_id = %user_id, error = %e, "failed to load tenant");
                    return false;
                }
            };

            refresh_tenant(handle, threshold).await
        }));
    }

    let mut refreshed = 0usize;
    for h in handles {
        match h.await {
            Ok(true) => refreshed += 1,
            Ok(false) => {}
            Err(e) => error!(error = %e, "pagerank tenant task panicked"),
        }
    }

    Ok(refreshed)
}

pub fn start_pagerank_refresh_job_tenant(
    registry: Arc<TenantRegistry>,
    config: Arc<Config>,
) -> CancellationToken {
    let token = CancellationToken::new();
    let cancel = token.clone();
    let interval = std::time::Duration::from_secs(config.pagerank_refresh_interval_secs.max(10));

    tokio::spawn(async move {
        info!(
            interval_secs = config.pagerank_refresh_interval_secs,
            "pagerank tenant refresh job started"
        );
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("pagerank tenant refresh job shutting down");
                    break;
                }
                _ = tokio::time::sleep(interval) => {
                    match run_once(&registry, &config).await {
                        Ok(n) if n > 0 => info!(tenants_refreshed = n, "pagerank tenant batch complete"),
                        Ok(_) => {}
                        Err(e) => error!(error = %e, "pagerank tenant refresh cycle failed"),
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

    #[tokio::test]
    async fn test_compute_pagerank_empty() {
        let db = Database::open_tenant_memory().await.unwrap();
        let scores = compute_pagerank_for_tenant(&db).await.unwrap();
        assert!(scores.is_empty());
    }

    #[tokio::test]
    async fn test_compute_pagerank_single_memory() {
        let db = Database::open_tenant_memory().await.unwrap();

        db.write(|conn| {
            conn.execute(
                "INSERT INTO memories (content, category) VALUES (?1, ?2)",
                rusqlite::params!["test content", "test"],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            Ok(())
        })
        .await
        .unwrap();

        let scores = compute_pagerank_for_tenant(&db).await.unwrap();
        assert_eq!(scores.len(), 1);
        assert!((scores[0].1 - 1.0).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_persist_pagerank() {
        let db = Database::open_tenant_memory().await.unwrap();

        db.write(|conn| {
            conn.execute(
                "INSERT INTO memories (content, category) VALUES (?1, ?2)",
                rusqlite::params!["test content", "test"],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            Ok(())
        })
        .await
        .unwrap();

        let scores = vec![(1i64, 0.75f64)];
        persist_pagerank_for_tenant(&db, scores).await.unwrap();

        let stored_score: f64 = db
            .read(|conn| {
                conn.query_row(
                    "SELECT score FROM memory_pagerank WHERE memory_id = 1",
                    [],
                    |row| row.get(0),
                )
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))
            })
            .await
            .unwrap();

        assert!((stored_score - 0.75).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_needs_refresh() {
        let db = Database::open_tenant_memory().await.unwrap();

        assert!(!needs_refresh(&db, 10).await.unwrap());

        for i in 0..15 {
            let content = format!("content {}", i);
            db.write(move |conn| {
                conn.execute(
                    "INSERT INTO memories (content, category) VALUES (?1, ?2)",
                    rusqlite::params![content, "test"],
                )
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
                Ok(())
            })
            .await
            .unwrap();
        }

        assert!(needs_refresh(&db, 10).await.unwrap());
        assert!(!needs_refresh(&db, 20).await.unwrap());
    }
}
