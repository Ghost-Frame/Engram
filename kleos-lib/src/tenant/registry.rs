//! Tenant registry - the main entry point for tenant management.
//!
//! The registry coordinates:
//! - Tenant creation and deletion
//! - Lazy loading via the TenantLoader
//! - Registry database persistence

use super::id::tenant_id_from_user;
use super::loader::TenantLoader;
use super::registry_db::RegistryDb;
use super::types::{TenantConfig, TenantHandle, TenantRow, TenantStatus};
use crate::{EngError, Result};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::info;

/// Schema version stamped on a newly created tenant registry row. Shards
/// start at v1 (tenant migration `apply_schema_v1`); the tenant migration
/// runner advances the shard schema from there on first load.
const SCHEMA_VERSION: i64 = 1;

/// The tenant registry manages all tenants.
///
/// This replaces the monolithic `Database` in `AppState`. Instead of one
/// database for all users, each user gets their own isolated tenant.
pub struct TenantRegistry {
    /// The registry database (system/registry.db).
    registry_db: Arc<RegistryDb>,

    /// The tenant loader for lazy loading and eviction.
    loader: Arc<TenantLoader>,

    /// Root data directory.
    data_root: PathBuf,

    /// Configuration.
    config: TenantConfig,
}

impl TenantRegistry {
    /// Create a new tenant registry.
    ///
    /// Opens or creates the registry database at `data_dir/system/registry.db`.
    pub fn new(
        data_dir: impl Into<PathBuf>,
        config: TenantConfig,
        vector_dimensions: usize,
        use_chunk_vector_search: bool,
        encryption_key: Option<[u8; 32]>,
    ) -> Result<Self> {
        let data_root = data_dir.into();

        // Create directory structure
        std::fs::create_dir_all(&data_root)
            .map_err(|e| EngError::Internal(format!("failed to create data directory: {}", e)))?;

        let registry_db = Arc::new(RegistryDb::open(&data_root)?);
        let loader = Arc::new(TenantLoader::new(
            data_root.clone(),
            config.clone(),
            vector_dimensions,
            use_chunk_vector_search,
            encryption_key,
        ));

        info!("tenant registry initialized at {}", data_root.display());

        Ok(Self {
            registry_db,
            loader,
            data_root,
            config,
        })
    }

    /// Create a registry with an in-memory database for testing.
    #[cfg(test)]
    pub fn new_memory(config: TenantConfig, vector_dimensions: usize) -> Result<Self> {
        let data_root = PathBuf::from("/tmp/kleos-test");
        let registry_db = Arc::new(RegistryDb::open_memory()?);
        let loader = Arc::new(TenantLoader::new(
            data_root.clone(),
            config.clone(),
            vector_dimensions,
            false,
            None,
        ));

        Ok(Self {
            registry_db,
            loader,
            data_root,
            config,
        })
    }

    /// Get or create a tenant for the given user_id.
    ///
    /// This is the main entry point for request handling:
    /// 1. Look up the tenant in the registry
    /// 2. Create if it doesn't exist
    /// 3. Load if not already resident
    /// 4. Return the handle
    pub async fn get_or_create(&self, user_id: &str) -> Result<Arc<TenantHandle>> {
        // Check if tenant exists
        let row = match self.registry_db.get_by_user_id(user_id)? {
            Some(row) => row,
            None => {
                // Create new tenant
                self.create_tenant(user_id).await?
            }
        };

        // Load or get from cache
        self.loader.get_or_load(&row.tenant_id, &row).await
    }

    /// Get a tenant by user_id without creating.
    ///
    /// Returns None if the tenant doesn't exist.
    pub async fn get(&self, user_id: &str) -> Result<Option<Arc<TenantHandle>>> {
        match self.registry_db.get_by_user_id(user_id)? {
            Some(row) => {
                let handle = self.loader.get_or_load(&row.tenant_id, &row).await?;
                Ok(Some(handle))
            }
            None => Ok(None),
        }
    }

    /// Create a new tenant for the given user_id.
    async fn create_tenant(&self, user_id: &str) -> Result<TenantRow> {
        let tenant_id = tenant_id_from_user(user_id);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let data_path = self
            .data_root
            .join("tenants")
            .join(&tenant_id)
            .to_string_lossy()
            .into_owned();

        // Create tenant directory structure
        let tenant_dir = self.data_root.join("tenants").join(&tenant_id);
        std::fs::create_dir_all(tenant_dir.join("hnsw"))
            .map_err(|e| EngError::Internal(format!("failed to create tenant directory: {}", e)))?;
        std::fs::create_dir_all(tenant_dir.join("blobs"))
            .map_err(|e| EngError::Internal(format!("failed to create blobs directory: {}", e)))?;

        let row = TenantRow {
            tenant_id: tenant_id.clone(),
            user_id: user_id.to_string(),
            created_at: now,
            status: TenantStatus::Active,
            data_path,
            schema_version: SCHEMA_VERSION,
            quota_bytes: None,
            quota_memories: None,
            last_access: now,
        };

        let row = self.registry_db.insert_or_get(&row)?;
        info!("created tenant: {} for user: {}", tenant_id, user_id);

        Ok(row)
    }

    /// Delete a tenant and all its data (legacy non-durable path).
    ///
    /// **Deprecated:** Use `begin_deprovision` from `tenant::teardown` instead,
    /// which provides durable two-phase teardown with archiving and audit log.
    #[deprecated(note = "Use tenant::teardown::begin_deprovision for durable teardown")]
    pub async fn delete(&self, user_id: &str) -> Result<()> {
        let row = self
            .registry_db
            .get_by_user_id(user_id)?
            .ok_or_else(|| EngError::NotFound(format!("tenant not found for user: {}", user_id)))?;

        // Mark as deleting
        self.registry_db
            .update_status(&row.tenant_id, TenantStatus::Deleting)?;

        // Evict from cache
        self.loader.evict(&row.tenant_id).await?;

        // Delete files
        let tenant_dir = self.data_root.join("tenants").join(&row.tenant_id);
        if tenant_dir.exists() {
            std::fs::remove_dir_all(&tenant_dir).map_err(|e| {
                EngError::Internal(format!("failed to delete tenant directory: {}", e))
            })?;
        }

        // Remove from registry
        self.registry_db.delete(&row.tenant_id)?;

        info!("deleted tenant: {} for user: {}", row.tenant_id, user_id);
        Ok(())
    }

    /// Suspend a tenant.
    pub fn suspend(&self, user_id: &str) -> Result<()> {
        let row = self
            .registry_db
            .get_by_user_id(user_id)?
            .ok_or_else(|| EngError::NotFound(format!("tenant not found for user: {}", user_id)))?;

        self.registry_db
            .update_status(&row.tenant_id, TenantStatus::Suspended)?;
        info!("suspended tenant: {}", row.tenant_id);
        Ok(())
    }

    /// Resume a suspended tenant.
    pub fn resume(&self, user_id: &str) -> Result<()> {
        let row = self
            .registry_db
            .get_by_user_id(user_id)?
            .ok_or_else(|| EngError::NotFound(format!("tenant not found for user: {}", user_id)))?;

        if row.status != TenantStatus::Suspended {
            return Err(EngError::InvalidInput(
                "tenant is not suspended".to_string(),
            ));
        }

        self.registry_db
            .update_status(&row.tenant_id, TenantStatus::Active)?;
        info!("resumed tenant: {}", row.tenant_id);
        Ok(())
    }

    /// List all tenants.
    pub fn list(&self) -> Result<Vec<TenantRow>> {
        self.registry_db.list()
    }

    /// Get tenant count.
    pub fn count(&self) -> Result<usize> {
        self.registry_db.count()
    }

    /// Get the number of currently loaded (resident) tenants.
    pub async fn resident_count(&self) -> usize {
        self.loader.resident_count().await
    }

    /// Run eviction for idle tenants.
    pub async fn evict_idle(&self) -> Result<usize> {
        self.loader.evict_idle().await
    }

    /// Get the data root path.
    pub fn data_root(&self) -> &PathBuf {
        &self.data_root
    }

    /// Get the configuration.
    pub fn config(&self) -> &TenantConfig {
        &self.config
    }

    /// Access the underlying registry database for direct queries.
    ///
    /// Used by the teardown subsystem for deprovision state queries.
    pub fn registry_db(&self) -> &RegistryDb {
        &self.registry_db
    }

    /// Clone the Arc-wrapped registry database for use in background tasks.
    ///
    /// Needed by the deprovision job handler and cluster heartbeat task,
    /// which must own an Arc to outlive the registry borrow.
    pub fn registry_db_arc(&self) -> Arc<RegistryDb> {
        Arc::clone(&self.registry_db)
    }

    /// Evict a tenant handle from the in-memory cache.
    ///
    /// Used by the teardown subsystem to release file handles before removal.
    pub async fn evict(&self, tenant_id: &str) -> Result<()> {
        self.loader.evict(tenant_id).await
    }

    /// Touch a tenant to update last access time.
    pub fn touch(&self, tenant_id: &str) -> Result<()> {
        self.registry_db.touch(tenant_id)
    }

    /// Return all currently resident tenant handles.
    ///
    /// Used by the disk sampler to iterate loaded tenants without re-loading
    /// evicted ones.
    pub async fn snapshot_all_handles(&self) -> Vec<Arc<TenantHandle>> {
        self.loader.snapshot_all_handles().await
    }

    /// Update quota limits for a tenant in the registry and refresh the in-memory handle.
    ///
    /// Writes the new limits to the registry then, if the handle is resident,
    /// replaces the ArcSwap so subsequent writes see the new limits immediately.
    pub async fn update_quota(
        &self,
        user_id: &str,
        content_bytes: Option<i64>,
        memory_count: Option<i64>,
        disk_bytes: Option<i64>,
    ) -> Result<()> {
        self.registry_db
            .update_quota(user_id, content_bytes, memory_count, disk_bytes)?;
        if let Some(handle) = self.loader.get_if_loaded(user_id).await {
            handle.refresh_quota(crate::tenant::types::QuotaConfig {
                content_bytes,
                memory_count,
                disk_bytes,
            });
        }
        Ok(())
    }

    /// Recompute tenant_state counters from the live shard and return (bytes, count).
    ///
    /// Overwrites content_bytes and memory_count in tenant_state.
    pub async fn recompute_usage(&self, user_id: &str) -> Result<(i64, i64)> {
        let handle = self
            .get(user_id)
            .await?
            .ok_or_else(|| crate::EngError::NotFound(format!("tenant not found: {}", user_id)))?;
        let db = handle.database();
        let (bytes, count) = db
            .write(|conn| {
                let (b, c): (i64, i64) = conn.query_row(
                    "SELECT COALESCE(SUM(length(content)), 0), COUNT(*) \
                         FROM memories WHERE is_latest = 1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                conn.execute(
                    "UPDATE tenant_state SET value = ?1, updated_at = datetime('now') \
                     WHERE key = 'content_bytes'",
                    rusqlite::params![b],
                )?;
                conn.execute(
                    "UPDATE tenant_state SET value = ?1, updated_at = datetime('now') \
                     WHERE key = 'memory_count'",
                    rusqlite::params![c],
                )?;
                Ok((b, c))
            })
            .await?;
        handle.mark_dirty();
        Ok((bytes, count))
    }

    /// Read quota limits and shadow usage from the registry for a user.
    pub fn get_quota_row(&self, user_id: &str) -> Result<crate::tenant::types::TenantQuotaRow> {
        self.registry_db.get_quota_row(user_id)
    }
}

impl std::fmt::Debug for TenantRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TenantRegistry")
            .field("data_root", &self.data_root)
            .field("config", &self.config)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Shared TenantConfig builder reserved for future registry-level tests
    /// (LRU eviction, lazy load, shutdown). Currently only
    /// `test_tenant_id_generation` runs here and does not need a config.
    #[allow(dead_code)]
    fn test_config() -> TenantConfig {
        TenantConfig {
            max_resident: 10,
            idle_timeout: Duration::from_secs(60),
            preload_on_start: false,
        }
    }

    #[test]
    fn test_tenant_id_generation() {
        // Safe IDs pass through
        assert_eq!(tenant_id_from_user("alice"), "alice");
        assert_eq!(tenant_id_from_user("user-123"), "user-123");

        // Unsafe IDs get hashed
        assert!(tenant_id_from_user("../etc/passwd").starts_with("t_"));
        assert!(tenant_id_from_user("user@example.com").starts_with("t_"));
    }
}
