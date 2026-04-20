pub mod backup;
pub mod migrations;
pub mod pitr;
pub mod pool;
pub mod schema;
pub mod schema_sql;
pub mod types;

use crate::config::Config;
use crate::vector::{LanceIndex, VectorIndex};
use crate::{EngError, Result};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};

pub use pool::DatabasePools;
pub use types::DbPoolConfig;

pub struct Database {
    db_path: String,
    pools: DatabasePools,
    pub vector_index: Option<Arc<dyn VectorIndex>>,
}

impl Database {
    /// Connect to a rusqlite database file without encryption.
    ///
    /// For encrypted databases, use `connect_encrypted` instead.
    pub async fn connect(db_path: &str) -> Result<Self> {
        let mut config = Config::from_env();
        config.db_path = db_path.to_string();
        Self::connect_with_config(&config, None).await
    }

    /// Connect to an encrypted rusqlite database file.
    ///
    /// The 32-byte key is applied via `PRAGMA key` as the first statement on
    /// every connection. Pass `None` for an unencrypted database.
    pub async fn connect_encrypted(db_path: &str, key: Option<[u8; 32]>) -> Result<Self> {
        let mut config = Config::from_env();
        config.db_path = db_path.to_string();
        Self::connect_with_config(&config, key).await
    }

    pub async fn connect_with_config(
        config: &Config,
        encryption_key: Option<[u8; 32]>,
    ) -> Result<Self> {
        Self::connect_with_pool_config(config, DbPoolConfig::default(), encryption_key).await
    }

    pub async fn connect_with_pool_config(
        config: &Config,
        pool_config: DbPoolConfig,
        encryption_key: Option<[u8; 32]>,
    ) -> Result<Self> {
        let db_path = &config.db_path;
        let pools = DatabasePools::new(db_path, pool_config, encryption_key).await?;

        // Run migrations using the writer pool
        let writer = pools.writer().get().await.map_err(|e| {
            EngError::DatabaseMessage(format!("failed to acquire writer pool connection: {e}"))
        })?;

        writer
            .interact(|conn| migrations::run_migrations(conn))
            .await
            .map_err(|e| {
                EngError::DatabaseMessage(format!("writer pool migration failed: {e}"))
            })??;

        let encrypted_label = if encryption_key.is_some() {
            " (encrypted)"
        } else {
            ""
        };
        info!("database connected: {}{}", db_path, encrypted_label);

        let vector_index = open_vector_index(config).await;

        Ok(Self {
            db_path: db_path.clone(),
            pools,
            vector_index,
        })
    }

    /// Connect to an in-memory database for testing.
    ///
    /// Uses a shared-cache URI with a unique name so all pool connections
    /// (readers + writer) share the same in-memory database instance.
    pub async fn connect_memory() -> Result<Self> {
        let id = uuid::Uuid::new_v4();
        let uri = format!("file:engram_test_{id}?mode=memory&cache=shared");
        let pools = DatabasePools::new(&uri, DbPoolConfig::default(), None).await?;

        let writer = pools.writer().get().await.map_err(|e| {
            EngError::DatabaseMessage(format!("failed to acquire writer pool connection: {e}"))
        })?;

        writer
            .interact(|conn| migrations::run_migrations(conn))
            .await
            .map_err(|e| EngError::DatabaseMessage(format!("migration failed: {e}")))??;

        Ok(Self {
            db_path: ":memory:".to_string(),
            pools,
            vector_index: None,
        })
    }

    /// Open a tenant's database with lightweight pools.
    /// Skips migrations (tenant DBs are already initialized) and
    /// uses a small pool config suitable for background processing.
    pub async fn open_tenant(
        db_path: &str,
        vector_index: Option<Arc<dyn VectorIndex>>,
    ) -> Result<Self> {
        let pool_config = DbPoolConfig {
            max_readers: 2,
            writer_count: 1,
            ..DbPoolConfig::default()
        };
        let pools = DatabasePools::new(db_path, pool_config, None).await?;

        Ok(Self {
            db_path: db_path.to_string(),
            pools,
            vector_index,
        })
    }

    pub fn db_path(&self) -> &str {
        &self.db_path
    }

    pub fn pools(&self) -> &DatabasePools {
        &self.pools
    }

    /// Execute a read operation on the database.
    pub async fn read<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&rusqlite::Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let conn = self.pools.reader().get().await.map_err(|e| {
            EngError::DatabaseMessage(format!("failed to acquire reader pool connection: {e}"))
        })?;

        conn.interact(move |conn| f(conn)).await.map_err(|e| {
            EngError::DatabaseMessage(format!("reader pool interaction failed: {e}"))
        })?
    }

    /// Execute a write operation on the database.
    pub async fn write<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut rusqlite::Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let conn = self.pools.writer().get().await.map_err(|e| {
            EngError::DatabaseMessage(format!("failed to acquire writer pool connection: {e}"))
        })?;

        conn.interact(move |conn| f(conn)).await.map_err(|e| {
            EngError::DatabaseMessage(format!("writer pool interaction failed: {e}"))
        })?
    }

    /// Execute a transaction on the database.
    pub async fn transaction<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&rusqlite::Transaction<'_>) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        self.write(move |conn| {
            let tx = conn
                .transaction()
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            let result = f(&tx)?;
            tx.commit()
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            Ok(result)
        })
        .await
    }
}

async fn open_vector_index(config: &Config) -> Option<Arc<dyn VectorIndex>> {
    if !config.use_lance_index {
        return None;
    }

    let lance_path = config.lance_index_path.clone().unwrap_or_else(|| {
        PathBuf::from(&config.data_dir)
            .join("lance")
            .to_string_lossy()
            .into_owned()
    });

    match LanceIndex::open(&lance_path, config.vector_dimensions).await {
        Ok(index) => {
            info!("LanceDB vector index connected: {}", lance_path);
            Some(Arc::new(index) as Arc<dyn VectorIndex>)
        }
        Err(e) => {
            warn!("LanceDB vector index unavailable: {}", e);
            None
        }
    }
}
