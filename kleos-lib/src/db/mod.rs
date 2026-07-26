pub mod backup;
pub mod converge;
pub mod migrations;
pub mod pitr;
pub mod pool;
pub mod schema;
pub mod schema_manifest;
pub mod schema_sql;
pub mod tenant_migrations;
pub mod types;

use crate::config::Config;
#[cfg(feature = "ml")]
use crate::vector::LanceIndex;
use crate::vector::VectorIndex;
use crate::{EngError, Result};
#[cfg(feature = "ml")]
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};

pub use pool::DatabasePools;
pub use types::DbPoolConfig;

/// Async SQLite database handle: pooled connections plus the optional
/// ANN vector indices and per-connection retrieval configuration.
pub struct Database {
    db_path: String,
    pools: DatabasePools,
    pub vector_index: Option<Arc<dyn VectorIndex>>,
    pub chunk_vector_index: Option<Arc<dyn VectorIndex>>,
    pub pagerank_notify: Arc<tokio::sync::Notify>,
    pub use_chunk_vector_search: bool,
    /// L5: mirror of `Config.facts_channel_enabled` -- gates the structured_facts RRF
    /// channel in `hybrid_search`. Default false; set via KLEOS_FACTS_CHANNEL_ENABLED.
    pub facts_channel_enabled: bool,
    pub embedding_chunk_max_chars: usize,
    pub embedding_chunk_overlap: usize,
    pub embedding_chunk_max_chunks: usize,
    is_tenant: bool,
}

/// Constructors and connection/pool plumbing.
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

    /// Connect using an explicit `Config` (encryption key optional).
    pub async fn connect_with_config(
        config: &Config,
        encryption_key: Option<[u8; 32]>,
    ) -> Result<Self> {
        Self::connect_with_pool_config(config, DbPoolConfig::default(), encryption_key).await
    }

    /// Connect with explicit pool sizing; runs migrations and opens the
    /// vector indices per config.
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

        writer
            .interact(|conn| {
                crate::db::converge::converge_schema(
                    conn,
                    crate::db::schema_manifest::SCHEMA_MANIFEST,
                )
                .map(|_| ())
            })
            .await
            .map_err(|e| {
                EngError::DatabaseMessage(format!("writer pool converge failed: {e}"))
            })??;

        let encrypted_label = if encryption_key.is_some() {
            " (encrypted)"
        } else {
            ""
        };
        info!("database connected: {}{}", db_path, encrypted_label);

        let (vector_index, chunk_vector_index) = open_vector_indices(config).await;

        Ok(Self {
            db_path: db_path.clone(),
            pools,
            vector_index,
            chunk_vector_index,
            pagerank_notify: Arc::new(tokio::sync::Notify::new()),
            use_chunk_vector_search: config.use_chunk_vector_search,
            facts_channel_enabled: config.facts_channel_enabled,
            embedding_chunk_max_chars: config.embedding_chunk_max_chars,
            embedding_chunk_overlap: config.embedding_chunk_overlap,
            embedding_chunk_max_chunks: config.embedding_chunk_max_chunks,
            is_tenant: false,
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

        writer
            .interact(|conn| {
                crate::db::converge::converge_schema(
                    conn,
                    crate::db::schema_manifest::SCHEMA_MANIFEST,
                )
                .map(|_| ())
            })
            .await
            .map_err(|e| {
                EngError::DatabaseMessage(format!("writer pool converge failed: {e}"))
            })??;

        Ok(Self {
            db_path: ":memory:".to_string(),
            pools,
            vector_index: None,
            chunk_vector_index: None,
            pagerank_notify: Arc::new(tokio::sync::Notify::new()),
            use_chunk_vector_search: false,
            facts_channel_enabled: false,
            embedding_chunk_max_chars: 1440,
            embedding_chunk_overlap: 160,
            embedding_chunk_max_chunks: 6,
            is_tenant: false,
        })
    }

    /// Open a tenant's database with lightweight pools.
    ///
    /// Runs the tenant migration chain (see `tenant_migrations`) on open so
    /// both freshly-created and existing tenant shards land at the latest
    /// tenant schema version. Pool sizes are kept small because thousands of
    /// tenants may be resident concurrently.
    ///
    /// `owner_user_id` is the integer id owning this shard (parsed from the
    /// tenant registry id); it is passed to the migration chain so the
    /// memory-core `user_id` re-add (tenant v55) can backfill existing rows to
    /// the owner. `None` for shards with non-numeric tenant ids (the reserved
    /// handoffs shard).
    pub async fn open_tenant(
        db_path: &str,
        vector_index: Option<Arc<dyn VectorIndex>>,
        encryption_key: Option<[u8; 32]>,
        owner_user_id: Option<i64>,
    ) -> Result<Self> {
        let pool_config = DbPoolConfig {
            max_readers: 2,
            writer_count: 1,
            ..DbPoolConfig::default()
        };
        let pools = DatabasePools::new(db_path, pool_config, encryption_key).await?;

        let writer = pools.writer().get().await.map_err(|e| {
            EngError::DatabaseMessage(format!(
                "failed to acquire tenant writer pool connection: {e}"
            ))
        })?;
        writer
            .interact(move |conn| tenant_migrations::run_tenant_migrations(conn, owner_user_id))
            .await
            .map_err(|e| {
                EngError::DatabaseMessage(format!("tenant pool migration failed: {e}"))
            })??;

        writer
            .interact(|conn| {
                crate::db::converge::converge_schema(
                    conn,
                    crate::db::schema_manifest::TENANT_SCHEMA_MANIFEST,
                )
                .map(|_| ())
            })
            .await
            .map_err(|e| EngError::DatabaseMessage(format!("tenant converge failed: {e}")))??;

        let encrypted_label = if encryption_key.is_some() {
            " (encrypted)"
        } else {
            ""
        };
        info!("tenant database connected: {}{}", db_path, encrypted_label);

        Ok(Self {
            db_path: db_path.to_string(),
            pools,
            vector_index,
            chunk_vector_index: None,
            pagerank_notify: Arc::new(tokio::sync::Notify::new()),
            use_chunk_vector_search: false,
            facts_channel_enabled: false,
            embedding_chunk_max_chars: 1440,
            embedding_chunk_overlap: 160,
            embedding_chunk_max_chunks: 6,
            is_tenant: true,
        })
    }

    /// Open an in-memory tenant database for testing.
    ///
    /// Runs the tenant migration chain so the schema matches a real tenant
    /// shard. Distinct from `connect_memory` which runs the system/main
    /// migration chain.
    pub async fn open_tenant_memory() -> Result<Self> {
        let id = uuid::Uuid::new_v4();
        let uri = format!("file:tenant_test_{id}?mode=memory&cache=shared");
        let pool_config = DbPoolConfig {
            max_readers: 2,
            writer_count: 1,
            ..DbPoolConfig::default()
        };
        let pools = DatabasePools::new(&uri, pool_config, None).await?;

        let writer = pools.writer().get().await.map_err(|e| {
            EngError::DatabaseMessage(format!("failed to acquire writer pool connection: {e}"))
        })?;
        writer
            .interact(|conn| tenant_migrations::run_tenant_migrations(conn, None))
            .await
            .map_err(|e| EngError::DatabaseMessage(format!("tenant migration failed: {e}")))??;

        writer
            .interact(|conn| {
                crate::db::converge::converge_schema(
                    conn,
                    crate::db::schema_manifest::TENANT_SCHEMA_MANIFEST,
                )
                .map(|_| ())
            })
            .await
            .map_err(|e| EngError::DatabaseMessage(format!("tenant converge failed: {e}")))??;

        Ok(Self {
            db_path: uri,
            pools,
            vector_index: None,
            chunk_vector_index: None,
            pagerank_notify: Arc::new(tokio::sync::Notify::new()),
            use_chunk_vector_search: false,
            facts_channel_enabled: false,
            embedding_chunk_max_chars: 1440,
            embedding_chunk_overlap: 160,
            embedding_chunk_max_chunks: 6,
            is_tenant: true,
        })
    }

    /// Checkpoint the WAL and truncate it. Call before evicting a tenant
    /// to ensure all in-flight writes are persisted to the main database file.
    pub async fn checkpoint(&self) -> Result<()> {
        self.write(|conn| {
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                .map_err(|e| EngError::DatabaseMessage(format!("checkpoint failed: {e}")))?;
            Ok(())
        })
        .await
    }

    /// Returns true if this is a tenant shard database.
    pub fn is_tenant(&self) -> bool {
        self.is_tenant
    }

    /// Path of the underlying database file (":memory:" for test DBs).
    pub fn db_path(&self) -> &str {
        &self.db_path
    }

    /// The underlying reader/writer connection pools.
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
            let tx = conn.transaction()?;
            let result = f(&tx)?;
            tx.commit()?;
            Ok(result)
        })
        .await
    }
}

/// Open the LanceDB memory + chunk vector indices per config. Either index
/// failing to open degrades to `None` (FTS + sqlite-vec retrieval) rather
/// than failing the whole database connection.
#[cfg(feature = "ml")]
async fn open_vector_indices(
    config: &Config,
) -> (Option<Arc<dyn VectorIndex>>, Option<Arc<dyn VectorIndex>>) {
    if !config.use_lance_index {
        return (None, None);
    }

    let lance_path = config.lance_index_path.clone().unwrap_or_else(|| {
        PathBuf::from(&config.data_dir)
            .join("lance")
            .to_string_lossy()
            .into_owned()
    });

    let memory_index = match LanceIndex::open(&lance_path, config.vector_dimensions).await {
        Ok(index) => {
            info!("LanceDB vector index connected: {}", lance_path);
            Some(Arc::new(index) as Arc<dyn VectorIndex>)
        }
        Err(e) => {
            warn!("LanceDB vector index unavailable: {}", e);
            None
        }
    };

    let chunk_index = match LanceIndex::open_with_table(
        &lance_path,
        config.vector_dimensions,
        crate::vector::CHUNK_TABLE_NAME,
    )
    .await
    {
        Ok(index) => {
            info!("LanceDB chunk vector index connected: {}", lance_path);
            Some(Arc::new(index) as Arc<dyn VectorIndex>)
        }
        Err(e) => {
            warn!("LanceDB chunk vector index unavailable: {}", e);
            None
        }
    };

    (memory_index, chunk_index)
}

/// ml-off stub: no ANN backend is compiled in, so both indices are `None`
/// regardless of `use_lance_index`; retrieval degrades to FTS + sqlite-vec
/// exactly as it does when a Lance open fails at runtime.
#[cfg(not(feature = "ml"))]
async fn open_vector_indices(
    config: &Config,
) -> (Option<Arc<dyn VectorIndex>>, Option<Arc<dyn VectorIndex>>) {
    if config.use_lance_index {
        warn!(
            "use_lance_index is set but this build has the 'ml' feature disabled; \
             continuing without ANN vector indices"
        );
    }
    (None, None)
}

/// Boot-path guarantees for the declarative schema converger: whatever the
/// manifest adds on top of the migrated schema must settle after one pass, so
/// a running server does not repeat schema work on every start.
#[cfg(test)]
mod converge_boot_tests {
    use super::*;

    /// Keystone invariant: converge must be idempotent. Boot already migrates
    /// and then converges, so a converge run immediately afterwards must find
    /// nothing left to do. That catches a manifest entry the converge pass
    /// cannot actually satisfy -- a column spec that does not match what got
    /// created, an index the manifest keeps trying to add -- which would
    /// otherwise show up as work repeated on every single boot.
    ///
    /// Idempotence, not emptiness-on-the-first-pass, is the property that
    /// survives a non-empty manifest: asserting the first converge takes zero
    /// actions would forbid the manifest from ever owning a table, which is
    /// the entire point of the mechanism.
    #[tokio::test]
    async fn global_converge_is_idempotent() -> Result<()> {
        // connect_memory runs migrations and then converges, so this is the
        // second pass.
        let db = Database::connect_memory().await?;
        let actions = db
            .read(|conn| {
                crate::db::converge::converge_schema(
                    conn,
                    crate::db::schema_manifest::SCHEMA_MANIFEST,
                )
            })
            .await?;
        assert!(
            actions.is_empty(),
            "global converge not idempotent: {actions:?}"
        );
        Ok(())
    }

    /// Same invariant for the tenant manifest against a freshly-migrated shard.
    /// The first converge is allowed to act -- that is how a manifest-owned
    /// table gets created on a shard whose migration chain never had one.
    #[test]
    fn tenant_converge_is_idempotent() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::tenant_migrations::run_tenant_migrations(&conn, None).unwrap();
        crate::db::converge::converge_schema(
            &conn,
            crate::db::schema_manifest::TENANT_SCHEMA_MANIFEST,
        )
        .unwrap();
        let second = crate::db::converge::converge_schema(
            &conn,
            crate::db::schema_manifest::TENANT_SCHEMA_MANIFEST,
        )
        .unwrap();
        assert!(
            second.is_empty(),
            "tenant converge not idempotent: {second:?}"
        );
    }
}
