//! Lazy tenant loading and LRU eviction.
//!
//! Tenants are loaded on first access and evicted when:
//! - They've been idle longer than `idle_timeout`
//! - The number of resident tenants exceeds `max_resident`

use super::types::{QuotaConfig, TenantConfig, TenantHandle, TenantRow, TenantStatus};
use crate::db::Database;
use crate::vector::LanceIndex;
use crate::{EngError, Result};
use std::collections::HashMap;
use std::path::PathBuf;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Instant, SystemTime};
use tokio::sync::{Mutex as AsyncMutex, RwLock};
use tracing::{debug, info, warn};

/// Manages lazy loading and eviction of tenant handles.
pub struct TenantLoader {
    /// Root data directory.
    data_root: PathBuf,

    /// Configuration for loading/eviction.
    config: TenantConfig,

    /// Currently loaded tenant handles.
    handles: RwLock<HashMap<String, Arc<TenantHandle>>>,

    /// Per-tenant locks that collapse concurrent first-touch loads into one open path.
    load_guards: AsyncMutex<HashMap<String, Arc<AsyncMutex<()>>>>,

    /// Dimension of embedding vectors.
    vector_dimensions: usize,

    /// Whether to enable chunk-level vector search on tenant databases.
    use_chunk_vector_search: bool,

    /// Encryption key for tenant databases (None = unencrypted).
    encryption_key: Option<[u8; 32]>,

    /// Counts how many times tests enter the slow tenant load path.
    #[cfg(test)]
    test_load_count: Arc<AtomicUsize>,

    /// Adds a deterministic delay before tests open tenant resources.
    #[cfg(test)]
    test_load_delay: Option<std::time::Duration>,
}

/// Implements tenant loader cache lookup, single-flight loading, and eviction.
impl TenantLoader {
    /// Create a new tenant loader.
    pub fn new(
        data_root: PathBuf,
        config: TenantConfig,
        vector_dimensions: usize,
        use_chunk_vector_search: bool,
        encryption_key: Option<[u8; 32]>,
    ) -> Self {
        Self {
            data_root,
            config,
            handles: RwLock::new(HashMap::new()),
            load_guards: AsyncMutex::new(HashMap::new()),
            vector_dimensions,
            use_chunk_vector_search,
            encryption_key,
            #[cfg(test)]
            test_load_count: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            test_load_delay: None,
        }
    }

    /// Get a tenant handle, loading it if not resident.
    ///
    /// Returns the existing handle if already loaded, otherwise loads from disk.
    pub async fn get_or_load(&self, tenant_id: &str, row: &TenantRow) -> Result<Arc<TenantHandle>> {
        // Fast path: check if already loaded
        {
            let handles = self.handles.read().await;
            if let Some(handle) = handles.get(tenant_id) {
                handle.touch();
                return Ok(handle.clone());
            }
        }

        // Slow path: serialize first-touch loads for this tenant only.
        let load_guard = {
            let mut guards = self.load_guards.lock().await;
            guards
                .entry(tenant_id.to_string())
                .or_insert_with(|| Arc::new(AsyncMutex::new(())))
                .clone()
        };
        let _load_guard = load_guard.lock().await;

        // Another task may have loaded the tenant while we waited.
        {
            let handles = self.handles.read().await;
            if let Some(handle) = handles.get(tenant_id) {
                handle.touch();
                return Ok(handle.clone());
            }
        }

        // Load the tenant once the per-tenant gate is held.
        self.load_tenant(tenant_id, row).await
    }

    /// Load a tenant from disk.
    async fn load_tenant(&self, tenant_id: &str, row: &TenantRow) -> Result<Arc<TenantHandle>> {
        // Check status
        if row.status == TenantStatus::Suspended {
            return Err(EngError::Auth("tenant is suspended".to_string()));
        }
        if row.status == TenantStatus::Deleting {
            return Err(EngError::NotFound("tenant is being deleted".to_string()));
        }

        #[cfg(test)]
        {
            self.test_load_count.fetch_add(1, Ordering::SeqCst);
            if let Some(delay) = self.test_load_delay {
                tokio::time::sleep(delay).await;
            }
        }

        // Evict if necessary before loading
        self.maybe_evict().await?;

        // Ensure tenant directory exists before opening pools.
        let tenant_dir = self.data_root.join("tenants").join(tenant_id);
        std::fs::create_dir_all(&tenant_dir)
            .map_err(|e| EngError::Internal(format!("failed to create tenant directory: {}", e)))?;

        // Load the vector index first so it can be handed to the Database.
        let lance_path = tenant_dir.join("hnsw").join("memories.lance");
        if let Some(parent) = lance_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                EngError::Internal(format!("failed to create lance directory: {}", e))
            })?;
        }

        let vector_index: Arc<dyn crate::vector::VectorIndex> = Arc::new(
            LanceIndex::open(
                lance_path.to_string_lossy().as_ref(),
                self.vector_dimensions,
            )
            .await
            .map_err(|e| EngError::Internal(format!("failed to open vector index: {}", e)))?,
        );

        let chunk_vector_index: Option<Arc<dyn crate::vector::VectorIndex>> = if self
            .use_chunk_vector_search
        {
            match LanceIndex::open_with_table(
                lance_path.to_string_lossy().as_ref(),
                self.vector_dimensions,
                crate::vector::lance::CHUNK_TABLE_NAME,
            )
            .await
            {
                Ok(idx) => Some(Arc::new(idx)),
                Err(e) => {
                    debug!(
                            "chunk vector index unavailable for tenant {}: {} (falling back to centroid)",
                            tenant_id, e
                        );
                    None
                }
            }
        } else {
            None
        };

        // Open the tenant's SQLite pool. The existing deployment path is
        // `tenants/<id>/kleos.db`; migration (tenant chain v1+) runs inside
        // `Database::open_tenant`.
        let db_path = tenant_dir.join("kleos.db").to_string_lossy().into_owned();
        // The registry user_id is `auth.user_id.to_string()` for real user
        // shards, so it parses back to the integer owner; the reserved handoffs
        // shard and similar use non-numeric ids and resolve to None.
        let owner_user_id = row.user_id.parse::<i64>().ok();
        let mut db = Database::open_tenant(
            &db_path,
            Some(Arc::clone(&vector_index)),
            self.encryption_key,
            owner_user_id,
        )
        .await?;
        db.use_chunk_vector_search = self.use_chunk_vector_search;
        db.chunk_vector_index = chunk_vector_index;
        let db = Arc::new(db);

        let initial_quota = QuotaConfig {
            content_bytes: row.quota_bytes,
            memory_count: row.quota_memories,
            disk_bytes: None,
        };
        let handle = Arc::new(TenantHandle {
            tenant_id: tenant_id.to_string(),
            user_id: row.user_id.clone(),
            db,
            vector_index,
            created_at: SystemTime::UNIX_EPOCH
                + std::time::Duration::from_secs(row.created_at as u64),
            last_access: std::sync::Mutex::new(Instant::now()),
            quota: arc_swap::ArcSwap::from_pointee(initial_quota),
            dirty: std::sync::atomic::AtomicBool::new(false),
            read_only: std::sync::atomic::AtomicBool::new(false),
            shard_path: tenant_dir.clone(),
        });

        // Store in cache
        {
            let mut handles = self.handles.write().await;
            handles.insert(tenant_id.to_string(), handle.clone());
        }

        info!("loaded tenant: {}", tenant_id);
        Ok(handle)
    }

    /// Check if a tenant is currently loaded.
    pub async fn is_loaded(&self, tenant_id: &str) -> bool {
        let handles = self.handles.read().await;
        handles.contains_key(tenant_id)
    }

    /// Get the number of currently loaded tenants.
    pub async fn resident_count(&self) -> usize {
        let handles = self.handles.read().await;
        handles.len()
    }

    /// Evict a specific tenant.
    pub async fn evict(&self, tenant_id: &str) -> Result<()> {
        let handle = {
            let mut handles = self.handles.write().await;
            handles.remove(tenant_id)
        };

        if let Some(handle) = handle {
            if let Err(e) = handle.db.checkpoint().await {
                warn!(
                    "failed to checkpoint tenant {} before eviction: {}",
                    tenant_id, e
                );
            }
            info!("evicted tenant: {}", tenant_id);
        }

        Ok(())
    }

    /// Evict idle tenants if we're over the limit.
    async fn maybe_evict(&self) -> Result<()> {
        let current_count = self.resident_count().await;
        if current_count < self.config.max_resident {
            return Ok(());
        }

        // Find idle tenants to evict
        let to_evict = {
            let handles = self.handles.read().await;
            let mut candidates: Vec<_> = handles
                .iter()
                .filter(|(_, h)| h.idle_duration() > self.config.idle_timeout)
                .map(|(id, h)| (id.clone(), h.idle_duration()))
                .collect();

            // Sort by idle duration (longest idle first)
            candidates.sort_by_key(|b| std::cmp::Reverse(b.1));

            // Take enough to get under the limit
            let excess = current_count.saturating_sub(self.config.max_resident) + 1;
            candidates
                .into_iter()
                .take(excess)
                .map(|(id, _)| id)
                .collect::<Vec<_>>()
        };

        for tenant_id in to_evict {
            self.evict(&tenant_id).await?;
        }

        Ok(())
    }

    /// Run a full eviction pass for all idle tenants.
    pub async fn evict_idle(&self) -> Result<usize> {
        let to_evict = {
            let handles = self.handles.read().await;
            handles
                .iter()
                .filter(|(_, h)| h.idle_duration() > self.config.idle_timeout)
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>()
        };

        let count = to_evict.len();
        for tenant_id in to_evict {
            self.evict(&tenant_id).await?;
        }

        if count > 0 {
            debug!("evicted {} idle tenants", count);
        }

        Ok(count)
    }

    /// Return the handle for `user_id` if currently resident, without loading.
    ///
    /// Scans resident handles by user_id (O(n) over max_resident=512).
    /// Used by admin quota update to refresh the ArcSwap without forcing a load.
    pub async fn get_if_loaded(&self, user_id: &str) -> Option<Arc<TenantHandle>> {
        let handles = self.handles.read().await;
        handles.values().find(|h| h.user_id == user_id).cloned()
    }

    /// Get all currently loaded tenant IDs.
    pub async fn loaded_tenant_ids(&self) -> Vec<String> {
        let handles = self.handles.read().await;
        handles.keys().cloned().collect()
    }

    /// Return all currently resident handles as a snapshot Vec.
    ///
    /// Used by the disk sampler to iterate loaded tenants without re-loading
    /// evicted ones. Snapshot is taken under a read lock; handles are Arcs so
    /// the snapshot is cheap.
    pub async fn snapshot_all_handles(&self) -> Vec<Arc<TenantHandle>> {
        let handles = self.handles.read().await;
        handles.values().cloned().collect()
    }
}

/// Exercises tenant loader residency and first-touch concurrency behavior.
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::tempdir;

    /// Builds a small tenant-loader configuration for fast tests.
    fn test_config() -> TenantConfig {
        TenantConfig {
            max_resident: 3,
            idle_timeout: Duration::from_millis(100),
            preload_on_start: false,
        }
    }

    /// Builds a representative active tenant row for loader tests.
    fn test_row(tenant_id: &str, user_id: &str) -> TenantRow {
        TenantRow {
            tenant_id: tenant_id.to_string(),
            user_id: user_id.to_string(),
            created_at: 0,
            status: TenantStatus::Active,
            data_path: format!("/tmp/{tenant_id}"),
            schema_version: 1,
            quota_bytes: None,
            quota_memories: None,
            last_access: 0,
        }
    }

    /// Confirms a new loader starts empty.
    #[tokio::test]
    async fn test_resident_count() {
        let loader =
            TenantLoader::new(PathBuf::from("/tmp/test"), test_config(), 1024, false, None);

        assert_eq!(loader.resident_count().await, 0);
        assert!(!loader.is_loaded("tenant_1").await);
    }

    /// Confirms concurrent first-touch requests collapse to one tenant open path.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_first_touch_opens_one_pool() {
        let dir = tempdir().expect("tempdir");
        let mut loader = TenantLoader::new(dir.path().to_path_buf(), test_config(), 8, false, None);
        loader.test_load_delay = Some(Duration::from_millis(50));
        let loader = Arc::new(loader);
        let row = test_row("tenant_4242", "user_4242");

        let mut tasks = Vec::new();
        for _ in 0..16 {
            let loader = Arc::clone(&loader);
            let row = row.clone();
            tasks.push(tokio::spawn(async move {
                loader.get_or_load(&row.tenant_id, &row).await
            }));
        }

        let mut handles = Vec::new();
        for task in tasks {
            handles.push(task.await.expect("task join").expect("tenant handle"));
        }

        let first = handles.first().expect("first handle").clone();
        for handle in handles.iter().skip(1) {
            assert!(
                Arc::ptr_eq(&first, handle),
                "all callers should receive the same resident tenant handle"
            );
        }
        assert_eq!(
            loader.test_load_count.load(Ordering::SeqCst),
            1,
            "only one slow-path tenant open should execute"
        );
    }
}
