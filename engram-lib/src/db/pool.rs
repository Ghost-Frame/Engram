use crate::{EngError, Result};
use deadpool_sqlite::{Config as PoolManagerConfig, Hook, HookError, Pool, PoolConfig, Runtime};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DbPoolConfig {
    pub max_readers: usize,
    pub writer_count: usize,
    pub busy_timeout_ms: u64,
    pub wal_autocheckpoint: u64,
}

impl Default for DbPoolConfig {
    fn default() -> Self {
        let cpu_count = std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1);

        Self {
            max_readers: cpu_count * 2,
            writer_count: 1,
            busy_timeout_ms: 5_000,
            wal_autocheckpoint: 1_000,
        }
    }
}

#[derive(Clone)]
pub struct DatabasePools {
    reader: Pool,
    writer: Pool,
    config: DbPoolConfig,
    db_path: String,
}

impl DatabasePools {
    pub async fn new(db_path: &str, config: DbPoolConfig) -> Result<Self> {
        ensure_libsql_initialized().await?;

        let reader = build_pool(db_path, config.max_readers, config)?;
        let writer = build_pool(db_path, config.writer_count.max(1), config)?;

        let pools = Self {
            reader,
            writer,
            config,
            db_path: db_path.to_string(),
        };

        pools.validate().await?;

        Ok(pools)
    }

    pub fn reader(&self) -> &Pool {
        &self.reader
    }

    pub fn writer(&self) -> &Pool {
        &self.writer
    }

    pub fn config(&self) -> DbPoolConfig {
        self.config
    }

    pub fn db_path(&self) -> &str {
        &self.db_path
    }

    async fn validate(&self) -> Result<()> {
        let reader = self
            .reader
            .get()
            .await
            .map_err(|e| EngError::Internal(format!("failed to acquire reader pool connection: {e}")))?;
        let writer = self
            .writer
            .get()
            .await
            .map_err(|e| EngError::Internal(format!("failed to acquire writer pool connection: {e}")))?;

        let expected_busy_timeout = self.config.busy_timeout_ms as i64;
        let is_memory = is_in_memory_db(&self.db_path);

        for (label, conn) in [("reader", &reader), ("writer", &writer)] {
            let busy_timeout = conn
                .interact(|conn| conn.query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0)))
                .await
                .map_err(|e| EngError::Internal(format!("failed to validate {label} pool connection: {e}")))?
                .map_err(|e| EngError::Internal(format!("failed to read {label} busy_timeout pragma: {e}")))?;

            if busy_timeout != expected_busy_timeout {
                return Err(EngError::Internal(format!(
                    "{label} pool busy_timeout mismatch: expected {expected_busy_timeout}, got {busy_timeout}"
                )));
            }

            let foreign_keys = conn
                .interact(|conn| conn.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0)))
                .await
                .map_err(|e| EngError::Internal(format!("failed to validate {label} foreign_keys pragma: {e}")))?
                .map_err(|e| EngError::Internal(format!("failed to read {label} foreign_keys pragma: {e}")))?;

            if foreign_keys != 1 {
                return Err(EngError::Internal(format!(
                    "{label} pool foreign_keys pragma not enabled"
                )));
            }

            if !is_memory {
                let journal_mode = conn
                    .interact(|conn| {
                        conn.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
                    })
                    .await
                    .map_err(|e| EngError::Internal(format!("failed to validate {label} journal_mode pragma: {e}")))?
                    .map_err(|e| EngError::Internal(format!("failed to read {label} journal_mode pragma: {e}")))?;

                if !journal_mode.eq_ignore_ascii_case("wal") {
                    return Err(EngError::Internal(format!(
                        "{label} pool journal_mode mismatch: expected wal, got {journal_mode}"
                    )));
                }
            }
        }

        Ok(())
    }
}

fn build_pool(db_path: &str, max_size: usize, config: DbPoolConfig) -> Result<Pool> {
    let mut manager = PoolManagerConfig::new(db_path);
    manager.pool = Some(PoolConfig::new(max_size));
    let db_path_owned = db_path.to_string();

    manager
        .builder(Runtime::Tokio1)
        .map_err(|e| EngError::Internal(format!("failed to configure sqlite pool for {db_path}: {e}")))?
        .post_create(Hook::async_fn(move |conn, _| {
            let db_path = db_path_owned.clone();
            Box::pin(async move {
                conn.interact(move |conn| apply_pragmas(conn, &db_path, config))
                    .await
                    .map_err(|e| HookError::message(format!("failed to initialize sqlite connection: {e}")))?
                    .map_err(HookError::Backend)?;

                Ok(())
            })
        }))
        .build()
        .map_err(|e| EngError::Internal(format!("failed to build sqlite pool for {db_path}: {e}")))
}

async fn ensure_libsql_initialized() -> Result<()> {
    let db = libsql::Builder::new_local(":memory:").build().await?;
    let conn = db.connect()?;
    drop(conn);
    drop(db);
    Ok(())
}

fn apply_pragmas(
    conn: &mut deadpool_sqlite::rusqlite::Connection,
    db_path: &str,
    config: DbPoolConfig,
) -> deadpool_sqlite::rusqlite::Result<()> {
    let is_memory = is_in_memory_db(db_path);

    if !is_memory {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "wal_autocheckpoint", config.wal_autocheckpoint)?;
        conn.pragma_update(None, "mmap_size", 268_435_456_i64)?;
    }

    conn.busy_timeout(Duration::from_millis(config.busy_timeout_ms))?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "cache_size", -65_536_i64)?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;

    Ok(())
}

fn is_in_memory_db(db_path: &str) -> bool {
    db_path == ":memory:" || (db_path.starts_with("file:") && db_path.contains("mode=memory"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::db::Database;
    use crate::EngError;

    fn temp_db_path(prefix: &str) -> String {
        std::env::temp_dir()
            .join(format!("{prefix}-{}.db", uuid::Uuid::new_v4()))
            .to_string_lossy()
            .into_owned()
    }

    #[tokio::test]
    async fn pool_applies_expected_pragmas() -> Result<()> {
        let db_path = temp_db_path("engram-pool-pragmas");
        let pools = DatabasePools::new(&db_path, DbPoolConfig::default()).await?;
        let conn = pools
            .reader()
            .get()
            .await
            .map_err(|e| EngError::Internal(format!("failed to get reader connection: {e}")))?;

        let busy_timeout = conn
            .interact(|conn| conn.query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0)))
            .await
            .map_err(|e| EngError::Internal(format!("pragma interaction failed: {e}")))?
            .map_err(|e| EngError::Internal(format!("pragma query failed: {e}")))?;
        let journal_mode = conn
            .interact(|conn| conn.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0)))
            .await
            .map_err(|e| EngError::Internal(format!("journal_mode interaction failed: {e}")))?
            .map_err(|e| EngError::Internal(format!("journal_mode query failed: {e}")))?;

        assert_eq!(busy_timeout, 5_000);
        assert!(journal_mode.eq_ignore_ascii_case("wal"));

        let _ = std::fs::remove_file(&db_path);
        Ok(())
    }

    #[tokio::test]
    async fn database_transaction_rolls_back_on_error() -> Result<()> {
        let db_path = temp_db_path("engram-pool-rollback");
        let config = Config {
            db_path: db_path.clone(),
            use_lance_index: false,
            ..Config::default()
        };

        let db = Database::connect_with_pool_config(&config, DbPoolConfig::default()).await?;

        db.write(|conn| {
            conn.execute(
                "CREATE TABLE IF NOT EXISTS pool_test_rollback (id INTEGER PRIMARY KEY)",
                [],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            Ok(())
        })
        .await?;

        let result = db
            .transaction(|tx| {
                tx.execute("INSERT INTO pool_test_rollback (id) VALUES (1)", [])
                    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
                tx.execute("INSERT INTO pool_test_missing DEFAULT VALUES", [])
                    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
                Ok(())
            })
            .await;

        assert!(matches!(result, Err(EngError::DatabaseMessage(_))));

        let count = db
            .read(|conn| {
                conn.query_row("SELECT COUNT(*) FROM pool_test_rollback", [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))
            })
            .await?;

        assert_eq!(count, 0);

        let _ = std::fs::remove_file(&db_path);
        Ok(())
    }
}
