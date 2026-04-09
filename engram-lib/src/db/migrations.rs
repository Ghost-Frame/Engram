use crate::Result;
use libsql::{Builder, Connection};

const MIGRATION_CREATE_SCHEMA: i64 = 1;

/// Run ordered, idempotent migrations and record applied versions.
pub async fn run_migrations(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        ",
    )
    .await?;

    let mut rows = conn
        .query("SELECT COALESCE(MAX(version), 0) FROM schema_version", ())
        .await?;
    let current_version: i64 = match rows.next().await? {
        Some(row) => row.get(0)?,
        None => 0,
    };

    if current_version < MIGRATION_CREATE_SCHEMA {
        super::schema::create_tables(conn).await?;
        conn.execute(
            "INSERT INTO schema_version (version, name) VALUES (?1, ?2)",
            libsql::params![MIGRATION_CREATE_SCHEMA, "create_tables"],
        )
        .await?;
    }

    Ok(())
}

/// Ensure schema/migrations are applied before any TypeScript import flow.
/// Source import is intentionally a no-op for now; schema setup is guaranteed.
pub async fn migrate_from_typescript(conn: &Connection, _source_path: &str) -> Result<()> {
    run_migrations(conn).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_migrations_idempotent() -> Result<()> {
        let db_path = std::env::temp_dir().join(format!(
            "engram-migrations-{}.db",
            uuid::Uuid::new_v4()
        ));
        let db = Builder::new_local(db_path.to_string_lossy().as_ref())
            .build()
            .await?;
        let conn = db.connect()?;

        run_migrations(&conn).await?;
        run_migrations(&conn).await?;

        let mut rows = conn
            .query(
                "SELECT COUNT(*) FROM schema_version WHERE version = ?1",
                libsql::params![MIGRATION_CREATE_SCHEMA],
            )
            .await?;
        let row = rows
            .next()
            .await?
            .ok_or_else(|| crate::EngError::Internal("missing schema_version row".to_string()))?;
        let count: i64 = row.get(0)?;
        assert_eq!(count, 1);

        let _ = std::fs::remove_file(&db_path);
        Ok(())
    }
}
