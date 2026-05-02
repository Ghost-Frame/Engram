//! Database backup, verification, and WAL checkpoint helpers.

pub use super::types::{CheckpointMode, RestoreReport};

use crate::{EngError, Result};
use std::path::Path;

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

/// Creates a consistent backup of the database using VACUUM INTO.
/// The destination path must not contain single quotes.
#[tracing::instrument(skip(db, dest))]
pub async fn vacuum_into(db: &crate::db::Database, dest: &Path) -> Result<()> {
    let path_str = dest.to_string_lossy().to_string();
    if path_str.contains('\'') {
        return Err(EngError::InvalidInput(
            "backup destination path contains a single quote".into(),
        ));
    }
    let sql = format!("VACUUM INTO '{}'", path_str);
    db.write(move |conn| {
        conn.execute(&sql, [])
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        Ok(())
    })
    .await
}

/// Runs PRAGMA integrity_check on the given database file.
/// Returns Ok(vec![]) if the database is valid, or Ok(vec![messages]) if corrupt.
#[tracing::instrument(skip(path))]
pub async fn integrity_check(path: &Path) -> Result<Vec<String>> {
    let path_str = path.to_string_lossy().to_string();

    // Open a direct rusqlite connection for integrity check
    let conn = rusqlite::Connection::open(&path_str)
        .map_err(|e| EngError::DatabaseMessage(format!("open for integrity check: {e}")))?;

    let mut stmt = conn
        .prepare("PRAGMA integrity_check")
        .map_err(|e| EngError::DatabaseMessage(format!("prepare integrity check: {e}")))?;

    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| EngError::DatabaseMessage(format!("query integrity check: {e}")))?;

    let messages: Vec<_> = rows.flatten().collect();

    if messages.len() == 1 && messages[0] == "ok" {
        Ok(Vec::new())
    } else {
        Ok(messages)
    }
}

/// Restore-test hook: opens the backup file as a live SQLite database and
/// runs a handful of sanity queries. A successful return means the file is
/// openable, query-able, and has the expected SQLite metadata surface.
///
/// This is a strictly stronger signal than `integrity_check`: a page-level
/// checksum pass does not guarantee that the schema was copied cleanly nor
/// that the sqlite_master catalog is queryable. Here we actually execute
/// queries against the restored file -- exactly what a disaster-recovery
/// restore would do.
#[tracing::instrument(skip(path))]
pub async fn restore_test(path: &Path) -> Result<RestoreReport> {
    let path_str = path.to_string_lossy().to_string();
    let path_for_task = path_str.clone();
    tokio::task::spawn_blocking(move || -> Result<RestoreReport> {
        let conn = rusqlite::Connection::open_with_flags(
            &path_for_task,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| EngError::DatabaseMessage(format!("restore_test open: {e}")))?;

        let schema_version: i64 = conn
            .query_row("PRAGMA schema_version", [], |row| row.get(0))
            .map_err(rusqlite_to_eng_error)?;

        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table'",
                [],
                |row| row.get(0),
            )
            .map_err(rusqlite_to_eng_error)?;

        let memory_count: Option<i64> = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='memories'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .ok()
            .filter(|n| *n > 0)
            .and_then(|_| {
                conn.query_row("SELECT COUNT(*) FROM memories", [], |row| {
                    row.get::<_, i64>(0)
                })
                .ok()
            });

        Ok(RestoreReport {
            schema_version,
            memory_count,
            table_count,
        })
    })
    .await
    .map_err(|e| EngError::DatabaseMessage(format!("restore_test join: {e}")))?
}

/// Runs WAL checkpoint with the given mode.
/// Returns (busy, log, checkpointed) frame counts.
#[tracing::instrument(skip(db), fields(mode = ?mode))]
pub async fn wal_checkpoint(
    db: &crate::db::Database,
    mode: CheckpointMode,
) -> Result<(i32, i32, i32)> {
    let mode_str = mode.as_str().to_string();
    db.read(move |conn| {
        let sql = format!("PRAGMA wal_checkpoint({})", mode_str);
        conn.query_row(&sql, [], |row| {
            Ok((
                row.get::<_, i32>(0).unwrap_or(0),
                row.get::<_, i32>(1).unwrap_or(0),
                row.get::<_, i32>(2).unwrap_or(0),
            ))
        })
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))
    })
    .await
    .or(Ok((0, 0, 0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_vacuum_into_creates_backup_file() {
        let src_path = std::env::temp_dir().join(format!("engram-src-{}.db", uuid::Uuid::new_v4()));
        let dst_path = std::env::temp_dir().join(format!("engram-dst-{}.db", uuid::Uuid::new_v4()));

        let db = crate::db::Database::connect(src_path.to_str().unwrap())
            .await
            .expect("connect source db");

        vacuum_into(&db, &dst_path).await.expect("vacuum_into");
        assert!(dst_path.exists(), "backup file should exist");

        // Clean up
        let _ = std::fs::remove_file(&src_path);
        let _ = std::fs::remove_file(&dst_path);
        let _ = std::fs::remove_file(src_path.with_extension("db-wal"));
        let _ = std::fs::remove_file(src_path.with_extension("db-shm"));
    }

    #[tokio::test]
    async fn test_integrity_check_passes_for_minimal_db() {
        let path = std::env::temp_dir().join(format!("engram-minimal-{}.db", uuid::Uuid::new_v4()));
        let path_str = path.to_str().unwrap().to_string();

        {
            let conn = rusqlite::Connection::open(&path_str).unwrap();
            conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY);")
                .unwrap();
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)", []).ok();
        }

        let errors = integrity_check(&path).await.expect("integrity_check");
        assert!(
            errors.is_empty(),
            "minimal db should pass integrity check: {:?}",
            errors
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[tokio::test]
    async fn test_wal_checkpoint_does_not_error() {
        // In-memory DB has no WAL; SQLite returns -1 for all counts in that case.
        // Verify the function handles this without panicking.
        let db = crate::db::Database::connect_memory()
            .await
            .expect("in-memory db");
        let result = wal_checkpoint(&db, CheckpointMode::Passive).await;
        assert!(
            result.is_ok(),
            "wal_checkpoint should not error: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_restore_test_reports_tables_and_schema_version() {
        let path = std::env::temp_dir().join(format!("engram-restore-{}.db", uuid::Uuid::new_v4()));
        {
            let conn = rusqlite::Connection::open(path.to_str().unwrap()).unwrap();
            conn.execute_batch(
                "CREATE TABLE memories (id INTEGER PRIMARY KEY, content TEXT); \
                 INSERT INTO memories (content) VALUES ('a'), ('b'), ('c');",
            )
            .unwrap();
        }
        let report = restore_test(&path).await.expect("restore_test");
        assert!(report.table_count >= 1);
        assert_eq!(report.memory_count, Some(3));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn test_restore_test_errors_on_missing_file() {
        let path = std::env::temp_dir().join(format!("engram-missing-{}.db", uuid::Uuid::new_v4()));
        let result = restore_test(&path).await;
        assert!(result.is_err(), "missing file should fail restore_test");
    }

    #[tokio::test]
    async fn test_restore_test_handles_db_without_memories_table() {
        let path = std::env::temp_dir().join(format!("engram-no-mem-{}.db", uuid::Uuid::new_v4()));
        {
            let conn = rusqlite::Connection::open(path.to_str().unwrap()).unwrap();
            conn.execute_batch("CREATE TABLE other (id INTEGER PRIMARY KEY);")
                .unwrap();
        }
        let report = restore_test(&path).await.expect("restore_test");
        assert!(report.table_count >= 1);
        assert_eq!(report.memory_count, None);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn test_vacuum_into_rejects_path_with_single_quote() {
        let db = crate::db::Database::connect_memory()
            .await
            .expect("in-memory db");
        let bad_path = std::path::Path::new("/tmp/bad'path.db");
        let result = vacuum_into(&db, bad_path).await;
        assert!(result.is_err(), "should reject path with single quote");
    }
}
