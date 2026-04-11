//! Database backup, verification, and WAL checkpoint helpers.

use crate::{EngError, Result};
use std::path::Path;

/// Creates a consistent backup of the database using VACUUM INTO.
/// The destination path must not contain single quotes.
pub async fn vacuum_into(db: &crate::db::Database, dest: &Path) -> Result<()> {
    let path_str = dest.to_string_lossy();
    if path_str.contains('\'') {
        return Err(EngError::InvalidInput(
            "backup destination path contains a single quote".into(),
        ));
    }
    db.conn
        .execute(&format!("VACUUM INTO '{}'", path_str), ())
        .await?;
    Ok(())
}

/// Runs PRAGMA integrity_check on the given database file.
/// Returns Ok(vec![]) if the database is valid, or Ok(vec![messages]) if corrupt.
pub async fn integrity_check(path: &Path) -> Result<Vec<String>> {
    let path_str = path.to_string_lossy().to_string();
    let db = libsql::Builder::new_local(&path_str)
        .build()
        .await
        .map_err(|e| EngError::DatabaseMessage(format!("open for integrity check: {e}")))?;
    let conn = db
        .connect()
        .map_err(|e| EngError::DatabaseMessage(format!("connect for integrity check: {e}")))?;

    let mut rows = conn.query("PRAGMA integrity_check", ()).await?;
    let mut messages = Vec::new();
    while let Some(row) = rows.next().await? {
        let msg: String = row.get(0).unwrap_or_default();
        messages.push(msg);
    }

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
    let sql = format!("PRAGMA wal_checkpoint({})", mode.as_str());
    let mut rows = db.conn.query(&sql, ()).await?;
    if let Some(row) = rows.next().await? {
        let busy: i32 = row.get(0).unwrap_or(0);
        let log: i32 = row.get(1).unwrap_or(0);
        let checkpointed: i32 = row.get(2).unwrap_or(0);
        return Ok((busy, log, checkpointed));
    }
    Ok((0, 0, 0))
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

    /// Create a minimal file-based libsql database with a simple schema for
    /// backup tests. Using raw libsql (no engram migrations) avoids the shadow
    /// tables that libsql creates for vector support, which cause false positives
    /// in PRAGMA integrity_check.
    async fn minimal_db(path: &str) -> crate::db::Database {
        use libsql::Builder;
        let inner = Builder::new_local(path).build().await.unwrap();
        let conn = inner.connect().unwrap();
        conn.execute_batch("CREATE TABLE backup_test (id INTEGER PRIMARY KEY, val TEXT);")
            .await
            .unwrap();
        conn.execute("INSERT INTO backup_test VALUES (1, 'hello')", ())
            .await
            .unwrap();
        // Flush WAL so the file is self-contained.
        conn.execute("PRAGMA wal_checkpoint(TRUNCATE)", ())
            .await
            .ok();
        // Build a Database wrapper via the public constructor.
        crate::db::Database::connect(path).await.unwrap()
    }

    #[tokio::test]
    async fn test_vacuum_into_creates_backup_file() {
        let src_path = std::env::temp_dir()
            .join(format!("engram-src-{}.db", uuid::Uuid::new_v4()));
        let dst_path = std::env::temp_dir()
            .join(format!("engram-dst-{}.db", uuid::Uuid::new_v4()));

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
        let path = std::env::temp_dir()
            .join(format!("engram-minimal-{}.db", uuid::Uuid::new_v4()));
        let path_str = path.to_str().unwrap().to_string();

        {
            use libsql::Builder;
            let inner = Builder::new_local(&path_str).build().await.unwrap();
            let conn = inner.connect().unwrap();
            conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY);")
                .await
                .unwrap();
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)", ())
                .await
                .ok();
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
        assert!(result.is_ok(), "wal_checkpoint should not error: {:?}", result);
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
