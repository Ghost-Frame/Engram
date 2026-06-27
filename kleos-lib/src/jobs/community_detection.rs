//! Scheduled community-detection job (L4b auto-run).
//!
//! Periodically re-runs Louvain so `community_id` stays fresh as memories accumulate, which is
//! what keeps the community retrieval channel useful (a one-shot snapshot decays). Handles both
//! deployment shapes, mirroring the dreamer: monolith (`registry = None` -> iterate active
//! owners on the shared DB) and tenant-sharded (`registry = Some` -> iterate active shards, one
//! owner each). Gated by `config.community_detection_enabled` (default off); it is a background
//! job, so owners are processed sequentially rather than fanned out -- detection is infrequent
//! and interval-gated, and this keeps the loop simple and lock-friendly.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::db::Database;
use crate::graph::communities::detect_communities;
use crate::tenant::TenantRegistry;

/// Louvain iteration budget per detection run (re-clamped inside `detect_communities`).
const DETECT_ITERATIONS: u32 = 50;

/// Re-detect only when at least this many visible memories were added since the last successful
/// run for an owner (or on the first run / when nothing is covered yet). Keeps a stable corpus
/// from recomputing Louvain every cycle for no benefit.
const REDETECT_MEMORY_DELTA: i64 = 25;

/// Count an owner's visible (latest, non-forgotten/archived) memories and how many already carry
/// a `community_id`. Drives the "is a re-detection worth it" decision in [`detect_one`].
async fn coverage(db: &Database, user_id: i64) -> crate::Result<(i64, i64)> {
    db.read(move |conn| {
        let total: i64 = conn.query_row(
            "SELECT COUNT(*) FROM memories \
             WHERE is_forgotten = 0 AND is_archived = 0 AND is_latest = 1 AND user_id = ?1",
            rusqlite::params![user_id],
            |r| r.get(0),
        )?;
        let covered: i64 = conn.query_row(
            "SELECT COUNT(*) FROM memories \
             WHERE is_forgotten = 0 AND is_archived = 0 AND is_latest = 1 AND user_id = ?1 \
               AND community_id IS NOT NULL",
            rusqlite::params![user_id],
            |r| r.get(0),
        )?;
        Ok((total, covered))
    })
    .await
}

/// Detect communities for one owner on `db` when there is meaningful new work, updating the
/// in-memory `last_total` delta gate. Returns `false` only on a detection error (a skip or a
/// successful run both return `true`).
async fn detect_one(db: &Database, user_id: i64, last_total: &mut HashMap<i64, i64>) -> bool {
    let (total, covered) = match coverage(db, user_id).await {
        Ok(c) => c,
        Err(e) => {
            warn!(user_id, error = %e, "community: coverage check failed");
            return false;
        }
    };
    if total == 0 {
        return true; // nothing to cluster
    }
    let prev = last_total.get(&user_id).copied().unwrap_or(-1);
    let needs = covered == 0 || (total - prev).abs() >= REDETECT_MEMORY_DELTA;
    if !needs {
        return true; // corpus stable since last run -- skip the recompute
    }
    match detect_communities(db, user_id, DETECT_ITERATIONS).await {
        Ok(res) => {
            info!(
                user_id,
                communities = res.communities,
                memories = res.memories,
                "community detection refreshed"
            );
            last_total.insert(user_id, total);
            true
        }
        Err(e) => {
            warn!(user_id, error = %e, "community detection failed");
            false
        }
    }
}

/// Distinct active memory owners on the shared DB (monolith mode).
async fn active_owners(db: &Database) -> crate::Result<Vec<i64>> {
    db.read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT user_id FROM memories \
             WHERE is_forgotten = 0 AND is_archived = 0 AND is_latest = 1 ORDER BY user_id",
        )?;
        let ids = stmt
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<Vec<i64>, _>>()?;
        Ok(ids)
    })
    .await
}

/// One detection pass over every active owner (monolith) or active shard (tenant). Returns the
/// number of owners that completed without error.
async fn run_cycle(
    db: &Arc<Database>,
    registry: Option<&Arc<TenantRegistry>>,
    last_total: &mut HashMap<i64, i64>,
) -> usize {
    let mut ok = 0usize;
    if let Some(reg) = registry {
        // Tenant-sharded: each active shard is one owner; mirror the dreamer's iteration.
        let tenants = match reg.list() {
            Ok(t) => t,
            Err(e) => {
                error!(error = %e, "community: failed to list tenants");
                return 0;
            }
        };
        for row in tenants {
            if row.status != crate::tenant::TenantStatus::Active {
                continue;
            }
            // The registry stores user_id as the string identifier the API routes by; parse to
            // the numeric form the shard's memories carry (one user per shard).
            let uid: i64 = match row.user_id.parse() {
                Ok(u) => u,
                Err(_) => continue,
            };
            let handle = match reg.get(&row.user_id).await {
                Ok(Some(h)) => h,
                Ok(None) => continue,
                Err(e) => {
                    warn!(tenant = %row.tenant_id, error = %e, "community: failed to load tenant");
                    continue;
                }
            };
            if detect_one(handle.db.as_ref(), uid, last_total).await {
                ok += 1;
            }
        }
    } else {
        // Monolith / shared-DB: iterate distinct owners on the one database.
        let owners = match active_owners(db.as_ref()).await {
            Ok(o) => o,
            Err(e) => {
                error!(error = %e, "community: failed to list owners");
                return 0;
            }
        };
        for uid in owners {
            if detect_one(db.as_ref(), uid, last_total).await {
                ok += 1;
            }
        }
    }
    ok
}

/// Spawn the periodic community-detection loop. Mirrors `start_pagerank_refresh_job`: a
/// cancellation-token-driven interval loop. Sleeps first (no startup CPU spike); an initial
/// population is done out-of-band at enable time. Returns the token + join handle so the server
/// supervisor can cancel/respawn it.
pub fn start_community_detection_job(
    db: Arc<Database>,
    registry: Option<Arc<TenantRegistry>>,
    config: Arc<Config>,
) -> (CancellationToken, tokio::task::JoinHandle<()>) {
    let token = CancellationToken::new();
    let cancel = token.clone();
    let interval = Duration::from_secs(config.community_detection_interval_secs.max(60));

    let handle = tokio::spawn(async move {
        info!(
            interval_secs = config.community_detection_interval_secs,
            "community detection job started"
        );
        // Per-owner visible-memory count at last successful run, for the re-detect delta gate.
        let mut last_total: HashMap<i64, i64> = HashMap::new();

        // Populate community_id promptly after startup (after a short settle so the deferred
        // embedder/reranker loads do not contend), then keep it fresh on the configured interval.
        tokio::select! {
            _ = cancel.cancelled() => {
                info!("community detection job shutting down");
                return;
            }
            _ = tokio::time::sleep(Duration::from_secs(45)) => {
                let n = run_cycle(&db, registry.as_ref(), &mut last_total).await;
                if n > 0 {
                    info!(owners_detected = n, "initial community detection complete");
                }
            }
        }

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("community detection job shutting down");
                    break;
                }
                _ = tokio::time::sleep(interval) => {
                    let n = run_cycle(&db, registry.as_ref(), &mut last_total).await;
                    if n > 0 {
                        info!(owners_detected = n, "community detection cycle complete");
                    }
                }
            }
        }
    });

    (token, handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh owner with memories but no community_id is detected on the first cycle, and a
    /// second cycle with no new memories skips the recompute (delta gate holds).
    #[tokio::test]
    async fn detect_one_runs_then_skips_when_stable() {
        let db = Database::connect_memory().await.expect("connect_memory");
        // Seed two distinct memories for user 1 via the store path (distinct enough that
        // simhash dedup keeps both as separate rows).
        for content in [
            "wireguard builds an encrypted tunnel between nodes",
            "postgres uses a write ahead log for durability",
        ] {
            let req = crate::memory::types::StoreRequest {
                content: content.to_string(),
                category: "test".to_string(),
                source: "test".to_string(),
                importance: 5,
                tags: None,
                embedding: None,
                chunk_embeddings: None,
                session_id: None,
                is_static: Some(false),
                user_id: Some(1),
                space_id: None,
                parent_memory_id: None,
                sync_id: None,
                artifacts: None,
            };
            crate::memory::store(&db, req, None, false)
                .await
                .expect("store");
        }

        let mut last_total: HashMap<i64, i64> = HashMap::new();
        // First pass: covered == 0 -> runs detection, records the total.
        assert!(detect_one(&db, 1, &mut last_total).await);
        assert_eq!(last_total.get(&1).copied(), Some(2));

        // Second pass with no new memories: stable -> skip (still Ok), total unchanged.
        assert!(detect_one(&db, 1, &mut last_total).await);
        assert_eq!(last_total.get(&1).copied(), Some(2));
    }
}
