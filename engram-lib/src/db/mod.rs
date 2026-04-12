pub mod backup;
pub mod migrations;
#[cfg(feature = "db_pool")]
pub mod pool;
pub mod schema;
mod schema_sql;

use crate::config::Config;
use crate::vector::{LanceIndex, VectorIndex};
#[cfg(feature = "db_pool")]
use crate::EngError;
use crate::Result;
use libsql::{Builder, Connection, Database as LibsqlDatabase};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseBackend {
    Libsql,
    #[cfg(feature = "db_pool")]
    Pool,
}

pub struct Database {
    backend: DatabaseBackend,
    db_path: String,
    db: LibsqlDatabase,
    pub conn: Connection,
    pub vector_index: Option<Arc<dyn VectorIndex>>,
    #[cfg(feature = "db_pool")]
    pools: Option<pool::DatabasePools>,
}

impl Database {
    /// Connect to a libsql database file, configure pragmas, enable WAL, create schema.
    pub async fn connect(db_path: &str) -> Result<Self> {
        let mut config = Config::from_env();
        config.db_path = db_path.to_string();
        Self::connect_with_config(&config).await
    }

    pub async fn connect_with_config(config: &Config) -> Result<Self> {
        Self::connect_with_config_inner(config, true).await
    }

    async fn connect_with_config_inner(config: &Config, run_migrations: bool) -> Result<Self> {
        let db_path = &config.db_path;
        let db = Builder::new_local(db_path).build().await?;
        let conn = db.connect()?;

        apply_libsql_pragmas(&conn, false).await?;

        if run_migrations {
            migrations::run_migrations(&conn).await?;
        }

        info!("database connected: {}", db_path);

        let vector_index = open_vector_index(config).await;

        Ok(Self {
            backend: DatabaseBackend::Libsql,
            db_path: db_path.clone(),
            db,
            conn,
            vector_index,
            #[cfg(feature = "db_pool")]
            pools: None,
        })
    }

    #[cfg(feature = "db_pool")]
    pub async fn connect_with_pool_config(
        config: &Config,
        pool_config: pool::DbPoolConfig,
    ) -> Result<Self> {
        let mut db = Self::connect_with_config_inner(config, false).await?;
        let pools = pool::DatabasePools::new(&db.db_path, pool_config).await?;
        let writer = pools.writer().get().await.map_err(|e| {
            EngError::DatabaseMessage(format!("failed to acquire writer pool connection: {e}"))
        })?;

        writer
            .interact(|conn| migrations::run_migrations_rusqlite(conn))
            .await
            .map_err(|e| {
                EngError::DatabaseMessage(format!("writer pool migration failed: {e}"))
            })??;

        db.backend = DatabaseBackend::Pool;
        db.pools = Some(pools);
        Ok(db)
    }

    /// Connect to an in-memory database for testing.
    /// Skips WAL and mmap PRAGMAs that are invalid for in-memory databases.
    pub async fn connect_memory() -> Result<Self> {
        let db = Builder::new_local(":memory:").build().await?;
        let conn = db.connect()?;

        apply_libsql_pragmas(&conn, true).await?;

        migrations::run_migrations(&conn).await?;

        Ok(Self {
            backend: DatabaseBackend::Libsql,
            db_path: ":memory:".to_string(),
            db,
            conn,
            vector_index: None,
            #[cfg(feature = "db_pool")]
            pools: None,
        })
    }

    pub fn backend(&self) -> DatabaseBackend {
        self.backend
    }

    pub fn db_path(&self) -> &str {
        &self.db_path
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Get a new connection from the database (for concurrent operations).
    pub fn new_connection(&self) -> Result<Connection> {
        Ok(self.db.connect()?)
    }

    #[cfg(feature = "db_pool")]
    pub fn pools(&self) -> Option<&pool::DatabasePools> {
        self.pools.as_ref()
    }

    #[cfg(feature = "db_pool")]
    pub async fn read<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&rusqlite::Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let pools = self.pools.as_ref().ok_or_else(|| {
            EngError::NotImplemented("db pool backend is not enabled for this database".into())
        })?;
        let conn = pools.reader().get().await.map_err(|e| {
            EngError::DatabaseMessage(format!("failed to acquire reader pool connection: {e}"))
        })?;

        conn.interact(move |conn| f(conn)).await.map_err(|e| {
            EngError::DatabaseMessage(format!("reader pool interaction failed: {e}"))
        })?
    }

    #[cfg(feature = "db_pool")]
    pub async fn write<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut rusqlite::Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let pools = self.pools.as_ref().ok_or_else(|| {
            EngError::NotImplemented("db pool backend is not enabled for this database".into())
        })?;
        let conn = pools.writer().get().await.map_err(|e| {
            EngError::DatabaseMessage(format!("failed to acquire writer pool connection: {e}"))
        })?;

        conn.interact(move |conn| f(conn)).await.map_err(|e| {
            EngError::DatabaseMessage(format!("writer pool interaction failed: {e}"))
        })?
    }

    #[cfg(feature = "db_pool")]
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

async fn apply_libsql_pragmas(conn: &Connection, is_memory: bool) -> Result<()> {
    if !is_memory {
        run_libsql_pragma(conn, "PRAGMA journal_mode = WAL").await?;
        run_libsql_pragma(conn, "PRAGMA synchronous = NORMAL").await?;
        run_libsql_pragma(conn, "PRAGMA mmap_size = 268435456").await?; // 256MB
    }

    run_libsql_pragma(conn, "PRAGMA cache_size = -64000").await?; // 64MB
    run_libsql_pragma(conn, "PRAGMA foreign_keys = ON").await?;
    run_libsql_pragma(conn, "PRAGMA busy_timeout = 5000").await?;
    run_libsql_pragma(conn, "PRAGMA temp_store = MEMORY").await?;

    Ok(())
}

async fn run_libsql_pragma(conn: &Connection, sql: &str) -> Result<()> {
    let mut rows = conn.query(sql, ()).await?;
    while rows.next().await?.is_some() {}
    Ok(())
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
                "LanceDB vector index unavailable, falling back to libsql vectors: {}",
                e
            );
            None
        }
    }
}
