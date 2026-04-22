//! Lazy tenant loading and LRU eviction.
//!
//! Tenants are loaded on first access and evicted when:
//! - They've been idle longer than `idle_timeout`
//! - The number of resident tenants exceeds `max_resident`

use super::types::{TenantConfig, TenantHandle, TenantRow, TenantStatus};
use crate::db::Database;
use crate::vector::LanceIndex;
use crate::{EngError, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Instant, SystemTime};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Manages lazy loading and eviction of tenant handles.
pub struct TenantLoader {
    /// Root data directory.
    data_root: PathBuf,

    /// Configuration for loading/eviction.
    config: TenantConfig,

    /// Currently loaded tenant handles.
    handles: RwLock<HashMap<String, Arc<TenantHandle>>>,

    /// Dimension of embedding vectors.
    vector_dimensions: usize,
}

impl TenantLoader {
    /// Create a new tenant loader.
    pub fn new(data_root: PathBuf, config: TenantConfig, vector_dimensions: usize) -> Self {
        Self {
            data_root,
            config,
            handles: RwLock::new(HashMap::new()),
            vector_dimensions,
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

        // Slow path: load the tenant
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

        // Evict if necessary before loading
        self.maybe_evict().await?;

        // Ensure tenant directory exists before opening pools.
        let tenant_dir = self.data_root.join("tenants").join(tenant_id);
        std::fs::create_dir_all(&tenant_dir).map_err(|e| {
            EngError::Internal(format!("failed to create tenant directory: {}", e))
        })?;

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

        // Open the tenant's SQLite pool. The existing deployment path is
        // `tenants/<id>/kleos.db`; migration (tenant chain v1+) runs inside
        // `Database::open_tenant`.
        let db_path = tenant_dir.join("kleos.db").to_string_lossy().into_owned();
        let db = Arc::new(
            Database::open_tenant(&db_path, Some(Arc::clone(&vector_index))).await?,
        );

        let handle = Arc::new(TenantHandle {
            tenant_id: tenant_id.to_string(),
            user_id: row.user_id.clone(),
            db,
            vector_index,
            created_at: SystemTime::UNIX_EPOCH
                + std::time::Duration::from_secs(row.created_at as u64),
            last_access: std::sync::Mutex::new(Instant::now()),
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

    /// Get all currently loaded tenant IDs.
    pub async fn loaded_tenant_ids(&self) -> Vec<String> {
        let handles = self.handles.read().await;
        handles.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn test_config() -> TenantConfig {
        TenantConfig {
            max_resident: 3,
            idle_timeout: Duration::from_millis(100),
            preload_on_start: false,
        }
    }

    #[tokio::test]
    async fn test_resident_count() {
        let loader = TenantLoader::new(PathBuf::from("/tmp/test"), test_config(), 1024);

        assert_eq!(loader.resident_count().await, 0);
        assert!(!loader.is_loaded("tenant_1").await);
    }
}
