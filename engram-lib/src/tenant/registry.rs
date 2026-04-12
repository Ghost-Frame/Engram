//! Tenant registry - the main entry point for tenant management.
//!
//! The registry coordinates:
//! - Tenant creation and deletion
//! - Lazy loading via the TenantLoader
//! - Registry database persistence

use super::id::tenant_id_from_user;
use super::loader::TenantLoader;
use super::registry_db::RegistryDb;
use super::schema::SCHEMA_VERSION;
use super::types::{TenantConfig, TenantHandle, TenantRow, TenantStatus};
use crate::{EngError, Result};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::info;

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
        let data_root = PathBuf::from("/tmp/engram-test");
        let registry_db = Arc::new(RegistryDb::open_memory()?);
        let loader = Arc::new(TenantLoader::new(
            data_root.clone(),
            config.clone(),
            vector_dimensions,
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

    /// Delete a tenant and all its data.
    ///
    /// This is irreversible! Use with caution.
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

    /// Touch a tenant to update last access time.
    pub fn touch(&self, tenant_id: &str) -> Result<()> {
        self.registry_db.touch(tenant_id)
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
