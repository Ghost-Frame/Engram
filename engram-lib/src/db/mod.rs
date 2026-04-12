pub mod backup;
pub mod migrations;
pub mod pool;
pub mod schema;
pub mod schema_sql;

use crate::config::Config;
use crate::vector::{LanceIndex, VectorIndex};
use crate::{EngError, Result};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};

pub use pool::{DatabasePools, DbPoolConfig};

pub struct Database {
    db_path: String,
    pools: DatabasePools,
    pub vector_index: Option<Arc<dyn VectorIndex>>,
}

impl Database {
    /// Connect to a rusqlite database file, configure pragmas, enable WAL, create schema.
    pub async fn connect(db_path: &str) -> Result<Self> {
        let mut config = Config::from_env();
        config.db_path = db_path.to_string();
        Self::connect_with_config(&config).await
    }

    pub async fn connect_with_config(config: &Config) -> Result<Self> {
        Self::connect_with_pool_config(config, DbPoolConfig::default()).await
    }

    pub async fn connect_with_pool_config(
        config: &Config,
        pool_config: DbPoolConfig,
    ) -> Result<Self> {
        let db_path = &config.db_path;
        let pools = DatabasePools::new(db_path, pool_config).await?;

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

        info!("database connected: {}", db_path);

        let vector_index = open_vector_index(config).await;

        Ok(Self {
            db_path: db_path.clone(),
            pools,
            vector_index,
        })
    }

    /// Connect to an in-memory database for testing.
    pub async fn connect_memory() -> Result<Self> {
        let pools = DatabasePools::new(":memory:", DbPoolConfig::default()).await?;

        let writer = pools.writer().get().await.map_err(|e| {
            EngError::DatabaseMessage(format!("failed to acquire writer pool connection: {e}"))
        })?;

        writer
            .interact(|conn| migrations::run_migrations(conn))
            .await
            .map_err(|e| {
                EngError::DatabaseMessage(format!("migration failed: {e}"))
            })??;

        Ok(Self {
            db_path: ":memory:".to_string(),
            pools,
            vector_index: None,
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
            warn!(
                "LanceDB vector index unavailable: {}",
                e
            );
            None
        }
    }
}
