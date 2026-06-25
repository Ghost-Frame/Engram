pub use super::types::DbPoolConfig;

use crate::{EngError, Result};
use deadpool_sqlite::{Config as PoolManagerConfig, Hook, HookError, Pool, PoolConfig, Runtime};
use std::time::Duration;

#[derive(Clone)]
pub struct DatabasePools {
    reader: Pool,
    writer: Pool,
    config: DbPoolConfig,
    db_path: String,
}

impl DatabasePools {
    pub async fn new(
        db_path: &str,
        config: DbPoolConfig,
        encryption_key: Option<[u8; 32]>,
    ) -> Result<Self> {
        let reader = build_pool(db_path, config.max_readers, config, encryption_key)?;
        let writer = build_pool(db_path, config.writer_count.max(1), config, encryption_key)?;

        let pools = Self {
            reader,
            writer,
            config,
            db_path: db_path.to_string(),
        };

        if let Err(e) = pools.validate().await {
            // Only attempt legacy rekey on existing non-empty database files.
            // Fresh databases or in-memory DBs should fail immediately.
            let is_existing_file = !is_in_memory_db(db_path)
                && std::fs::metadata(db_path)
                    .map(|m| m.len() > 0)
                    .unwrap_or(false);
            if encryption_key.is_none() || !is_existing_file {
                return Err(e);
            }
            tracing::debug!(db = db_path, error = %e, "raw hex key failed, trying legacy passphrase rekey");
        } else {
            return Ok(pools);
        }

        let key = encryption_key.unwrap();

        // Try legacy passphrase rekey first (pre-fix databases).
        if migrate_legacy_passphrase_to_raw_hex(db_path, &key).is_ok() {
            tracing::info!(db = db_path, "legacy passphrase rekey succeeded");
        } else if migrate_plaintext_to_encrypted(db_path, &key).is_ok() {
            tracing::info!(db = db_path, "plaintext-to-encrypted migration succeeded");
        } else {
            return Err(EngError::Internal(format!(
                "database {db_path} could not be opened: raw hex key, legacy passphrase, \
                 and plaintext migration all failed"
            )));
        }

        let reader = build_pool(db_path, config.max_readers, config, encryption_key)?;
        let writer = build_pool(db_path, config.writer_count.max(1), config, encryption_key)?;
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
        let reader = self.reader.get().await.map_err(|e| {
            EngError::Internal(format!("failed to acquire reader pool connection: {e}"))
        })?;
        let writer = self.writer.get().await.map_err(|e| {
            EngError::Internal(format!("failed to acquire writer pool connection: {e}"))
        })?;

        let expected_busy_timeout = self.config.busy_timeout_ms as i64;
        let is_memory = is_in_memory_db(&self.db_path);

        for (label, conn) in [("reader", &reader), ("writer", &writer)] {
            let busy_timeout = conn
                .interact(|conn| {
                    conn.query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
                })
                .await
                .map_err(|e| {
                    EngError::Internal(format!("failed to validate {label} pool connection: {e}"))
                })?
                .map_err(|e| {
                    EngError::Internal(format!("failed to read {label} busy_timeout pragma: {e}"))
                })?;

            if busy_timeout != expected_busy_timeout {
                return Err(EngError::Internal(format!(
                    "{label} pool busy_timeout mismatch: expected {expected_busy_timeout}, got {busy_timeout}"
                )));
            }

            let foreign_keys = conn
                .interact(|conn| {
                    conn.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))
                })
                .await
                .map_err(|e| {
                    EngError::Internal(format!(
                        "failed to validate {label} foreign_keys pragma: {e}"
                    ))
                })?
                .map_err(|e| {
                    EngError::Internal(format!("failed to read {label} foreign_keys pragma: {e}"))
                })?;

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
                    .map_err(|e| {
                        EngError::Internal(format!(
                            "failed to validate {label} journal_mode pragma: {e}"
                        ))
                    })?
                    .map_err(|e| {
                        EngError::Internal(format!(
                            "failed to read {label} journal_mode pragma: {e}"
                        ))
                    })?;

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

fn build_pool(
    db_path: &str,
    max_size: usize,
    config: DbPoolConfig,
    encryption_key: Option<[u8; 32]>,
) -> Result<Pool> {
    let mut manager = PoolManagerConfig::new(db_path);
    manager.pool = Some(PoolConfig::new(max_size));
    let db_path_owned = db_path.to_string();

    manager
        .builder(Runtime::Tokio1)
        .map_err(|e| {
            EngError::Internal(format!(
                "failed to configure sqlite pool for {db_path}: {e}"
            ))
        })?
        .post_create(Hook::async_fn(move |conn, _| {
            let db_path = db_path_owned.clone();
            Box::pin(async move {
                conn.interact(move |conn| apply_pragmas(conn, &db_path, config, encryption_key))
                    .await
                    .map_err(|e| {
                        HookError::message(format!("failed to initialize sqlite connection: {e}"))
                    })?
                    .map_err(HookError::Backend)?;

                Ok(())
            })
        }))
        .build()
        .map_err(|e| EngError::Internal(format!("failed to build sqlite pool for {db_path}: {e}")))
}

fn apply_pragmas(
    conn: &mut deadpool_sqlite::rusqlite::Connection,
    db_path: &str,
    config: DbPoolConfig,
    encryption_key: Option<[u8; 32]>,
) -> deadpool_sqlite::rusqlite::Result<()> {
    // SQLCipher PRAGMA key MUST be the very first statement on a connection.
    // Any other statement on an encrypted DB without the key will fail with
    // "file is not a database".
    if let Some(ref key) = encryption_key {
        // SQLCipher raw key mode: PRAGMA key = x'<hex>' (unquoted hex literal).
        // rusqlite's pragma_update() wraps the value in single quotes, turning
        // the x'...' hex literal into a passphrase string. Use execute_batch()
        // to emit the raw SQL without quoting.
        let mut key_sql = format!(
            "PRAGMA key = {};",
            crate::encryption::format_pragma_key(key).as_str()
        );
        let pragma_result = conn.execute_batch(&key_sql);
        use zeroize::Zeroize;
        key_sql.zeroize();
        pragma_result?;

        // Verify the key is correct by reading schema_version. If the key
        // is wrong, SQLCipher returns "file is encrypted or is not a database"
        // on the first real read.
        conn.pragma_query_value(None, "schema_version", |_| Ok(()))
            .map_err(|e| {
                if e.to_string().contains("not a database") {
                    deadpool_sqlite::rusqlite::Error::SqliteFailure(
                        deadpool_sqlite::rusqlite::ffi::Error::new(
                            deadpool_sqlite::rusqlite::ffi::SQLITE_NOTADB,
                        ),
                        Some(
                            "wrong encryption key or unencrypted database opened with encryption enabled"
                                .to_string(),
                        ),
                    )
                } else {
                    e
                }
            })?;
    }

    let is_memory = is_in_memory_db(db_path);

    if !is_memory {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "wal_autocheckpoint", config.wal_autocheckpoint)?;
        conn.pragma_update(None, "mmap_size", 268_435_456_i64)?;
        // Cap WAL file size at 256 MiB. SQLite truncates after a checkpoint
        // that brings the WAL below this limit. Prevents unbounded WAL growth
        // during bursty write workloads (e.g. bulk ingest, PageRank refresh).
        conn.pragma_update(None, "journal_size_limit", 268_435_456_i64)?;
    }

    conn.busy_timeout(Duration::from_millis(config.busy_timeout_ms))?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "cache_size", -65_536_i64)?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;

    // Let SQLite refresh query planner statistics for tables that need it.
    // This is a no-op when stats are already fresh, so safe on every new
    // pooled connection.
    conn.execute_batch("PRAGMA optimize;")?;

    Ok(())
}

fn is_in_memory_db(db_path: &str) -> bool {
    db_path == ":memory:" || (db_path.starts_with("file:") && db_path.contains("mode=memory"))
}

/// Migrate a database from legacy passphrase encryption to raw hex key mode.
///
/// Prior to the PRAGMA key fix, rusqlite's `pragma_update` wrapped the
/// `x'<hex>'` literal in single quotes, causing SQLCipher to treat it as a
/// passphrase (PBKDF2-derived) instead of a raw 256-bit key. This opens the
/// file with that legacy passphrase and uses `PRAGMA rekey` to re-encrypt
/// with the correct raw hex key.
fn migrate_legacy_passphrase_to_raw_hex(
    db_path: &str,
    key: &[u8; crate::encryption::KEY_SIZE],
) -> Result<()> {
    use zeroize::Zeroize;
    // Both the hex key and the derived passphrase embed the raw key; wrap them
    // in Zeroizing so they are scrubbed on drop rather than left on the heap.
    let hex_str = zeroize::Zeroizing::new(hex::encode(key));
    let raw_key_pragma = crate::encryption::format_pragma_key(key);

    // The legacy passphrase: old format_pragma_key returned x'<hex>' (no
    // double quotes). rusqlite's pragma_update wrapped that in SQL string
    // quotes with internal ' doubled: PRAGMA key = 'x''<hex>'''. SQLCipher
    // received the passphrase x'<hex>' and derived the key via PBKDF2.
    let legacy_passphrase = zeroize::Zeroizing::new(format!("x'{}'", hex_str.as_str()));
    let mut legacy_key_sql = format!("PRAGMA key = '{}';", legacy_passphrase.replace('\'', "''"));

    let conn = deadpool_sqlite::rusqlite::Connection::open(db_path).map_err(|e| {
        EngError::DatabaseMessage(format!("failed to open {db_path} for rekey: {e}"))
    })?;

    conn.execute_batch(&legacy_key_sql).map_err(|e| {
        EngError::DatabaseMessage(format!("legacy PRAGMA key failed on {db_path}: {e}"))
    })?;
    legacy_key_sql.zeroize();

    // Verify the legacy key works
    conn.pragma_query_value(None, "schema_version", |_| Ok(()))
        .map_err(|e| {
            EngError::DatabaseMessage(format!(
                "legacy passphrase verification failed on {db_path}: {e} -- \
                 the database may be encrypted with a different key"
            ))
        })?;

    // Rekey to raw hex mode
    let mut rekey_sql = format!("PRAGMA rekey = {};", raw_key_pragma.as_str());
    conn.execute_batch(&rekey_sql)
        .map_err(|e| EngError::DatabaseMessage(format!("PRAGMA rekey failed on {db_path}: {e}")))?;
    rekey_sql.zeroize();

    tracing::warn!(
        db = db_path,
        "migrated encryption from legacy passphrase to raw hex key"
    );

    Ok(())
}

/// Encrypt a plaintext SQLite database in place.
///
/// Pre-deploy tenant databases may be unencrypted. This opens the file
/// without a key, ATTACHes a new encrypted copy, uses sqlcipher_export
/// to copy all data, then atomically swaps the files.
fn migrate_plaintext_to_encrypted(
    db_path: &str,
    key: &[u8; crate::encryption::KEY_SIZE],
) -> Result<()> {
    use zeroize::Zeroize;

    let conn = deadpool_sqlite::rusqlite::Connection::open(db_path).map_err(|e| {
        EngError::DatabaseMessage(format!("failed to open {db_path} for plaintext check: {e}"))
    })?;

    // Verify it's actually a readable plaintext DB.
    conn.pragma_query_value(None, "schema_version", |_| Ok(()))
        .map_err(|e| {
            EngError::DatabaseMessage(format!(
                "{db_path} is not a readable plaintext database: {e}"
            ))
        })?;

    let encrypted_path = format!("{db_path}.encrypting");
    let _ = std::fs::remove_file(&encrypted_path);

    let raw_key_pragma = crate::encryption::format_pragma_key(key);
    let mut attach_sql = format!(
        "ATTACH DATABASE '{}' AS encrypted KEY {};",
        encrypted_path.replace('\'', "''"),
        raw_key_pragma.as_str()
    );
    conn.execute_batch(&attach_sql).map_err(|e| {
        EngError::DatabaseMessage(format!("ATTACH encrypted DB failed on {db_path}: {e}"))
    })?;
    attach_sql.zeroize();

    conn.execute_batch("SELECT sqlcipher_export('encrypted');")
        .map_err(|e| {
            let _ = std::fs::remove_file(&encrypted_path);
            EngError::DatabaseMessage(format!("sqlcipher_export failed on {db_path}: {e}"))
        })?;

    conn.execute_batch("DETACH DATABASE encrypted;")
        .map_err(|e| EngError::DatabaseMessage(format!("DETACH failed on {db_path}: {e}")))?;
    drop(conn);

    // Backup the plaintext file, then atomic swap.
    let backup_path = format!("{db_path}.plaintext-backup");
    std::fs::rename(db_path, &backup_path).map_err(|e| {
        EngError::DatabaseMessage(format!("failed to rename {db_path} -> {backup_path}: {e}"))
    })?;
    std::fs::rename(&encrypted_path, db_path).map_err(|e| {
        let _ = std::fs::rename(&backup_path, db_path);
        EngError::DatabaseMessage(format!(
            "failed to rename {encrypted_path} -> {db_path}: {e}"
        ))
    })?;

    // Clean up WAL/SHM from the old plaintext DB.
    let _ = std::fs::remove_file(format!("{db_path}-wal"));
    let _ = std::fs::remove_file(format!("{db_path}-shm"));

    // Discard the plaintext backup by default: leaving it forever silently
    // defeats the at-rest-encryption threat model (a disk-access attacker reads
    // the unencrypted copy). Operators who want a rollback safety net retain it
    // with KLEOS_KEEP_PLAINTEXT_BACKUP=1. We only delete after confirming the
    // freshly-swapped encrypted DB actually opens and reads with the key, so a
    // failed migration never destroys the only readable copy.
    if should_keep_plaintext_backup(crate::kleos_env("KEEP_PLAINTEXT_BACKUP").ok().as_deref()) {
        tracing::warn!(
            db = db_path,
            backup = backup_path,
            "migrated plaintext database to encrypted (plaintext backup RETAINED via KLEOS_KEEP_PLAINTEXT_BACKUP)"
        );
    } else if !verify_encrypted_readable(db_path, key) {
        tracing::error!(
            db = db_path,
            backup = backup_path,
            "encrypted database failed post-migration verification; plaintext backup RETAINED for recovery -- investigate before removing it"
        );
    } else {
        match std::fs::remove_file(&backup_path) {
            Ok(()) => tracing::info!(
                db = db_path,
                "migrated plaintext database to encrypted (plaintext backup removed)"
            ),
            Err(e) => tracing::warn!(
                db = db_path,
                backup = backup_path,
                error = %e,
                "migrated to encrypted but could not remove plaintext backup; remove it manually"
            ),
        }
    }

    Ok(())
}

/// Pure parse of `KLEOS_KEEP_PLAINTEXT_BACKUP`: retain the plaintext backup only
/// when explicitly opted in. Absent or any non-truthy value means delete.
fn should_keep_plaintext_backup(env_val: Option<&str>) -> bool {
    match env_val {
        Some(v) => {
            let s = v.trim().to_ascii_lowercase();
            s == "1" || s == "true" || s == "yes" || s == "on"
        }
        None => false,
    }
}

/// Confirm the encrypted database at `db_path` opens and reads with `key`.
/// Used to gate deletion of the plaintext backup so a botched migration never
/// leaves the operator with no readable copy.
fn verify_encrypted_readable(db_path: &str, key: &[u8; crate::encryption::KEY_SIZE]) -> bool {
    use deadpool_sqlite::rusqlite::{Connection, OpenFlags};
    use zeroize::Zeroize;
    // READ_ONLY (no CREATE): a missing or unreadable file fails rather than
    // being created as an empty DB that would spuriously verify.
    let conn = match Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let pragma = crate::encryption::format_pragma_key(key);
    let mut key_sql = format!("PRAGMA key = {};", pragma.as_str());
    let keyed = conn.execute_batch(&key_sql).is_ok();
    key_sql.zeroize();
    if !keyed {
        return false;
    }
    // Succeeds only when the cipher key decrypts the database header.
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
        .is_ok()
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

    #[test]
    fn keep_plaintext_backup_defaults_to_delete() {
        // Absent or non-truthy -> delete (do not retain plaintext).
        assert!(!should_keep_plaintext_backup(None));
        assert!(!should_keep_plaintext_backup(Some("0")));
        assert!(!should_keep_plaintext_backup(Some("false")));
        assert!(!should_keep_plaintext_backup(Some("")));
        // Explicit opt-in retains.
        for on in ["1", "true", "yes", "on", " TRUE "] {
            assert!(should_keep_plaintext_backup(Some(on)), "{on} must retain");
        }
    }

    #[test]
    fn verify_encrypted_readable_false_for_missing_db() {
        // A non-existent / unreadable path must never report verified, so the
        // backup is never deleted on a failed migration.
        let key = [7u8; crate::encryption::KEY_SIZE];
        assert!(!verify_encrypted_readable(
            &temp_db_path("nope-missing"),
            &key
        ));
    }

    #[tokio::test]
    async fn pool_applies_expected_pragmas() -> Result<()> {
        let db_path = temp_db_path("engram-pool-pragmas");
        let pools = DatabasePools::new(&db_path, DbPoolConfig::default(), None).await?;
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
            .interact(|conn| {
                conn.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
            })
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

        let db = Database::connect_with_pool_config(&config, DbPoolConfig::default(), None).await?;

        db.write(|conn| {
            conn.execute(
                "CREATE TABLE IF NOT EXISTS pool_test_rollback (id INTEGER PRIMARY KEY)",
                [],
            )?;
            Ok(())
        })
        .await?;

        let result = db
            .transaction(|tx| {
                tx.execute("INSERT INTO pool_test_rollback (id) VALUES (1)", [])?;
                tx.execute("INSERT INTO pool_test_missing DEFAULT VALUES", [])?;
                Ok(())
            })
            .await;

        assert!(matches!(result, Err(EngError::Database(_))));

        let count = db
            .read(|conn| {
                Ok(
                    conn.query_row("SELECT COUNT(*) FROM pool_test_rollback", [], |row| {
                        row.get::<_, i64>(0)
                    })?,
                )
            })
            .await?;

        assert_eq!(count, 0);

        let _ = std::fs::remove_file(&db_path);
        Ok(())
    }
}
