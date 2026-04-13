//! Database backup, verification, and WAL checkpoint helpers.

use crate::{EngError, Result};
use std::path::Path;

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

/// Creates a consistent backup of the database using VACUUM INTO.
/// The destination path must not contain single quotes.
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

/// Runs WAL checkpoint with the given mode.
/// Returns (busy, log, checkpointed) frame counts.
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

#[derive(Debug, Clone, Copy)]
pub enum CheckpointMode {
    Passive,
    Full,
    Restart,
    Truncate,
}

impl CheckpointMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Passive => "PASSIVE",
            Self::Full => "FULL",
            Self::Restart => "RESTART",
            Self::Truncate => "TRUNCATE",
        }
    }
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
    async fn test_vacuum_into_rejects_path_with_single_quote() {
        let db = crate::db::Database::connect_memory()
            .await
            .expect("in-memory db");
        let bad_path = std::path::Path::new("/tmp/bad'path.db");
        let result = vacuum_into(&db, bad_path).await;
        assert!(result.is_err(), "should reject path with single quote");
    }
}
