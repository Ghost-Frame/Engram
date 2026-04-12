//! Connection pooling for per-tenant databases using deadpool-sqlite.
//!
//! Provides reader/writer separation for concurrent access to tenant SQLite databases.
//! This module is only available when the `db_pool` feature is enabled.

use crate::{EngError, Result};
use deadpool_sqlite::{Config as PoolManagerConfig, Hook, HookError, Pool, PoolConfig, Runtime};
use std::path::Path;
use std::time::Duration;
use tracing::info;

/// Configuration for tenant database connection pools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TenantPoolConfig {
    /// Maximum number of reader connections per tenant.
    pub max_readers: usize,
    /// Number of writer connections (usually 1 for SQLite).
    pub writer_count: usize,
    /// Busy timeout in milliseconds.
    pub busy_timeout_ms: u64,
    /// WAL autocheckpoint interval.
    pub wal_autocheckpoint: u64,
}

impl Default for TenantPoolConfig {
    fn default() -> Self {
        // Smaller pools per tenant since we may have many tenants
        Self {
            max_readers: 4,
            writer_count: 1,
            busy_timeout_ms: 5_000,
            wal_autocheckpoint: 1_000,
        }
    }
}

/// Connection pools for a single tenant database.
#[derive(Clone)]
pub struct TenantPools {
    reader: Pool,
    writer: Pool,
    config: TenantPoolConfig,
    db_path: String,
}

impl TenantPools {
    /// Create new connection pools for a tenant database.
    pub async fn new(db_path: impl AsRef<Path>, config: TenantPoolConfig) -> Result<Self> {
        let db_path_str = db_path.as_ref().to_string_lossy().into_owned();

        // Ensure parent directory exists
        if let Some(parent) = db_path.as_ref().parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                EngError::Internal(format!("failed to create tenant directory: {}", e))
            })?;
        }

        let reader = build_pool(&db_path_str, config.max_readers, config)?;
        let writer = build_pool(&db_path_str, config.writer_count.max(1), config)?;

        let pools = Self {
            reader,
            writer,
            config,
            db_path: db_path_str.clone(),
        };

        pools.validate().await?;
        pools.ensure_schema().await?;

        info!("tenant pool created: {}", db_path_str);

        Ok(pools)
    }

    /// Get the reader pool.
    pub fn reader(&self) -> &Pool {
        &self.reader
    }

    /// Get the writer pool.
    pub fn writer(&self) -> &Pool {
        &self.writer
    }

    /// Get the pool configuration.
    pub fn config(&self) -> TenantPoolConfig {
        self.config
    }

    /// Get the database path.
    pub fn db_path(&self) -> &str {
        &self.db_path
    }

    /// Execute a read operation on the reader pool.
    pub async fn read<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&deadpool_sqlite::rusqlite::Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let conn = self.reader.get().await.map_err(|e| {
            EngError::Internal(format!("failed to acquire tenant reader connection: {e}"))
        })?;

        conn.interact(move |conn| f(conn))
            .await
            .map_err(|e| EngError::Internal(format!("tenant reader interaction failed: {e}")))?
    }

    /// Execute a write operation on the writer pool.
    pub async fn write<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut deadpool_sqlite::rusqlite::Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let conn = self.writer.get().await.map_err(|e| {
            EngError::Internal(format!("failed to acquire tenant writer connection: {e}"))
        })?;

        conn.interact(move |conn| f(conn))
            .await
            .map_err(|e| EngError::Internal(format!("tenant writer interaction failed: {e}")))?
    }

    /// Execute a transaction on the writer pool.
    pub async fn transaction<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&deadpool_sqlite::rusqlite::Transaction<'_>) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        self.write(move |conn| {
            let tx = conn
                .transaction()
                .map_err(|e| EngError::Internal(format!("failed to start transaction: {}", e)))?;
            let result = f(&tx)?;
            tx.commit()
                .map_err(|e| EngError::Internal(format!("failed to commit transaction: {}", e)))?;
            Ok(result)
        })
        .await
    }

    /// Checkpoint the WAL.
    pub async fn checkpoint(&self) -> Result<()> {
        self.write(|conn| {
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)", [])
                .map_err(|e| EngError::Internal(format!("checkpoint failed: {}", e)))?;
            Ok(())
        })
        .await
    }

    async fn validate(&self) -> Result<()> {
        let reader = self.reader.get().await.map_err(|e| {
            EngError::Internal(format!("failed to acquire tenant reader for validation: {e}"))
        })?;

        let expected = self.config.busy_timeout_ms as i64;
        let busy_timeout: i64 = reader
            .interact(move |conn: &mut deadpool_sqlite::rusqlite::Connection| {
                conn.query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            })
            .await
            .map_err(|e| EngError::Internal(format!("validation interaction failed: {e}")))?
            .map_err(|e| EngError::Internal(format!("busy_timeout query failed: {e}")))?;

        if busy_timeout != expected {
            return Err(EngError::Internal(format!(
                "tenant pool busy_timeout mismatch: expected {expected}, got {busy_timeout}"
            )));
        }

        Ok(())
    }

    async fn ensure_schema(&self) -> Result<()> {
        self.write(|conn| {
            super::schema::create_tables(conn)
                .map_err(|e| EngError::Internal(format!("tenant schema creation failed: {}", e)))
        })
        .await
    }
}

fn build_pool(db_path: &str, max_size: usize, config: TenantPoolConfig) -> Result<Pool> {
    let mut manager = PoolManagerConfig::new(db_path);
    manager.pool = Some(PoolConfig::new(max_size));
    let db_path_owned = db_path.to_string();

    manager
        .builder(Runtime::Tokio1)
        .map_err(|e| {
            EngError::Internal(format!(
                "failed to configure tenant pool for {db_path}: {e}"
            ))
        })?
        .post_create(Hook::async_fn(move |conn, _| {
            let db_path = db_path_owned.clone();
            Box::pin(async move {
                conn.interact(move |conn: &mut deadpool_sqlite::rusqlite::Connection| {
                    apply_pragmas(conn, &db_path, config)
                })
                .await
                .map_err(|e| {
                    HookError::message(format!("failed to initialize tenant connection: {e}"))
                })?
                .map_err(HookError::Backend)?;
                Ok(())
            })
        }))
        .build()
        .map_err(|e| {
            EngError::Internal(format!("failed to build tenant pool for {db_path}: {e}"))
        })
}

fn apply_pragmas(
    conn: &mut deadpool_sqlite::rusqlite::Connection,
    db_path: &str,
    config: TenantPoolConfig,
) -> deadpool_sqlite::rusqlite::Result<()> {
    let is_memory = db_path == ":memory:" || db_path.contains("mode=memory");

    if !is_memory {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "wal_autocheckpoint", config.wal_autocheckpoint)?;
        conn.pragma_update(None, "mmap_size", 67_108_864_i64)?; // 64MB per tenant
    }

    conn.busy_timeout(Duration::from_millis(config.busy_timeout_ms))?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "cache_size", -16_384_i64)?; // 16MB cache per tenant
    conn.pragma_update(None, "temp_store", "MEMORY")?;

    Ok(())
}

impl std::fmt::Debug for TenantPools {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TenantPools")
            .field("db_path", &self.db_path)
            .field("config", &self.config)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db_path(prefix: &str) -> String {
        std::env::temp_dir()
            .join(format!("{prefix}-{}.db", uuid::Uuid::new_v4()))
            .to_string_lossy()
            .into_owned()
    }

    #[tokio::test]
    async fn tenant_pool_creates_and_validates() -> Result<()> {
        let db_path = temp_db_path("engram-tenant-pool");
        let pools = TenantPools::new(&db_path, TenantPoolConfig::default()).await?;

        let count: i64 = pools
            .read(|conn| {
                conn.query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))
                    .map_err(|e| EngError::Internal(e.to_string()))
            })
            .await?;

        assert_eq!(count, 0);

        let _ = std::fs::remove_file(&db_path);
        Ok(())
    }

    #[tokio::test]
    async fn tenant_pool_write_and_read() -> Result<()> {
        let db_path = temp_db_path("engram-tenant-pool-rw");
        let pools = TenantPools::new(&db_path, TenantPoolConfig::default()).await?;

        pools
            .write(|conn| {
                conn.execute(
                    "INSERT INTO memories (content, category) VALUES (?1, ?2)",
                    ["test content", "test"],
                )
                .map_err(|e| EngError::Internal(e.to_string()))?;
                Ok(())
            })
            .await?;

        let content: String = pools
            .read(|conn| {
                conn.query_row("SELECT content FROM memories WHERE id = 1", [], |row| {
                    row.get(0)
                })
                .map_err(|e| EngError::Internal(e.to_string()))
            })
            .await?;

        assert_eq!(content, "test content");

        let _ = std::fs::remove_file(&db_path);
        Ok(())
    }
}
