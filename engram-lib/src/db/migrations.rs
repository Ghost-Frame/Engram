use crate::Result;
#[cfg(feature = "db_pool")]
use crate::EngError;
use libsql::Connection;
use tracing::info;

/// Migration versions - add new migrations here
const MIGRATION_CREATE_SCHEMA: i64 = 1;
const MIGRATION_ADD_MISSING_INDEXES: i64 = 2;
const MIGRATION_PAGERANK_TABLES: i64 = 3;

/// Run ordered, idempotent migrations and record applied versions.
pub async fn run_migrations(conn: &Connection) -> Result<()> {
    // Create schema_version table if it doesn't exist
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

    // Migration 1: Create initial schema
    if current_version < MIGRATION_CREATE_SCHEMA {
        info!("Running migration 1: create_tables");
        super::schema::create_tables(conn).await?;
        record_migration(conn, MIGRATION_CREATE_SCHEMA, "create_tables").await?;
    }

    // Migration 2: Add any missing indexes (idempotent - CREATE INDEX IF NOT EXISTS)
    // This ensures existing DBs get new indexes added in schema.rs updates
    if current_version < MIGRATION_ADD_MISSING_INDEXES {
        info!("Running migration 2: add_missing_indexes");
        run_migration_add_missing_indexes(conn).await?;
        record_migration(conn, MIGRATION_ADD_MISSING_INDEXES, "add_missing_indexes").await?;
    }

    // Migration 3: Add pagerank cache and dirty-tracking tables
    if current_version < MIGRATION_PAGERANK_TABLES {
        info!("Running migration 3: add_pagerank_tables");
        run_migration_pagerank_tables(conn).await?;
        record_migration(conn, MIGRATION_PAGERANK_TABLES, "add_pagerank_tables").await?;
    }

    // Future migrations go here:
    // if current_version < MIGRATION_XXX { ... }

    Ok(())
}

#[cfg(feature = "db_pool")]
pub fn run_migrations_rusqlite(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        ",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    let current_version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    if current_version < MIGRATION_CREATE_SCHEMA {
        info!("Running migration 1: create_tables");
        super::schema::create_tables_rusqlite(conn)?;
        record_migration_rusqlite(conn, MIGRATION_CREATE_SCHEMA, "create_tables")?;
    }

    if current_version < MIGRATION_ADD_MISSING_INDEXES {
        info!("Running migration 2: add_missing_indexes");
        run_migration_add_missing_indexes_rusqlite(conn)?;
        record_migration_rusqlite(conn, MIGRATION_ADD_MISSING_INDEXES, "add_missing_indexes")?;
    }

    Ok(())
}

/// Record that a migration has been applied
async fn record_migration(conn: &Connection, version: i64, name: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO schema_version (version, name) VALUES (?1, ?2)",
        libsql::params![version, name],
    )
    .await?;
    Ok(())
}

/// Migration 2: Ensure all indexes from schema.rs exist
/// This is safe to run multiple times due to IF NOT EXISTS
async fn run_migration_add_missing_indexes(conn: &Connection) -> Result<()> {
    // Re-run index creation from schema to catch any new indexes
    // All indexes use CREATE INDEX IF NOT EXISTS so this is idempotent
    conn.execute_batch(
        "
        -- Memory indexes
        CREATE INDEX IF NOT EXISTS idx_memories_root ON memories(root_memory_id);
        CREATE INDEX IF NOT EXISTS idx_memories_superseded ON memories(is_superseded) WHERE is_superseded = 1;
        CREATE INDEX IF NOT EXISTS idx_memories_parent ON memories(parent_memory_id);
        CREATE INDEX IF NOT EXISTS idx_memories_latest ON memories(is_latest) WHERE is_latest = 1;
        CREATE INDEX IF NOT EXISTS idx_memories_forgotten ON memories(is_forgotten);
        CREATE INDEX IF NOT EXISTS idx_memories_archived ON memories(is_archived) WHERE is_archived = 1;
        CREATE INDEX IF NOT EXISTS idx_memories_forget_after ON memories(forget_after) WHERE forget_after IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_memories_tags ON memories(tags) WHERE tags IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_memories_episode ON memories(episode_id) WHERE episode_id IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_memories_access ON memories(access_count DESC);
        CREATE INDEX IF NOT EXISTS idx_memories_decay ON memories(decay_score DESC);
        CREATE INDEX IF NOT EXISTS idx_memories_status ON memories(status);
        CREATE INDEX IF NOT EXISTS idx_memories_user ON memories(user_id);
        CREATE INDEX IF NOT EXISTS idx_memories_space ON memories(space_id);
        CREATE INDEX IF NOT EXISTS idx_memories_fsrs_stability ON memories(fsrs_stability) WHERE fsrs_stability IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_memories_category ON memories(category);
        CREATE INDEX IF NOT EXISTS idx_memories_source ON memories(source);
        CREATE INDEX IF NOT EXISTS idx_memories_created ON memories(created_at);
        CREATE INDEX IF NOT EXISTS idx_memories_user_latest ON memories(user_id, is_latest, is_forgotten);

        -- Composite indexes for common query patterns
        CREATE INDEX IF NOT EXISTS idx_memories_search_composite ON memories(user_id, is_forgotten, is_latest, category);
        ",
    )
    .await?;
    Ok(())
}


/// Migration 3: Create pagerank cache and dirty-tracking tables
async fn run_migration_pagerank_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS memory_pagerank (
            memory_id INTEGER PRIMARY KEY,
            user_id INTEGER NOT NULL,
            score REAL NOT NULL,
            computed_at INTEGER NOT NULL,
            FOREIGN KEY (memory_id) REFERENCES memories(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_pagerank_user ON memory_pagerank(user_id);
        CREATE INDEX IF NOT EXISTS idx_pagerank_score ON memory_pagerank(score DESC);

        CREATE TABLE IF NOT EXISTS pagerank_dirty (
            user_id INTEGER PRIMARY KEY,
            dirty_count INTEGER NOT NULL DEFAULT 0,
            last_refresh INTEGER NOT NULL DEFAULT 0
        );
        ",
    )
    .await?;
    Ok(())
}
#[cfg(feature = "db_pool")]
fn record_migration_rusqlite(conn: &rusqlite::Connection, version: i64, name: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO schema_version (version, name) VALUES (?1, ?2)",
        rusqlite::params![version, name],
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

#[cfg(feature = "db_pool")]
fn run_migration_add_missing_indexes_rusqlite(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "
        -- Memory indexes
        CREATE INDEX IF NOT EXISTS idx_memories_root ON memories(root_memory_id);
        CREATE INDEX IF NOT EXISTS idx_memories_superseded ON memories(is_superseded) WHERE is_superseded = 1;
        CREATE INDEX IF NOT EXISTS idx_memories_parent ON memories(parent_memory_id);
        CREATE INDEX IF NOT EXISTS idx_memories_latest ON memories(is_latest) WHERE is_latest = 1;
        CREATE INDEX IF NOT EXISTS idx_memories_forgotten ON memories(is_forgotten);
        CREATE INDEX IF NOT EXISTS idx_memories_archived ON memories(is_archived) WHERE is_archived = 1;
        CREATE INDEX IF NOT EXISTS idx_memories_forget_after ON memories(forget_after) WHERE forget_after IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_memories_tags ON memories(tags) WHERE tags IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_memories_episode ON memories(episode_id) WHERE episode_id IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_memories_access ON memories(access_count DESC);
        CREATE INDEX IF NOT EXISTS idx_memories_decay ON memories(decay_score DESC);
        CREATE INDEX IF NOT EXISTS idx_memories_status ON memories(status);
        CREATE INDEX IF NOT EXISTS idx_memories_user ON memories(user_id);
        CREATE INDEX IF NOT EXISTS idx_memories_space ON memories(space_id);
        CREATE INDEX IF NOT EXISTS idx_memories_fsrs_stability ON memories(fsrs_stability) WHERE fsrs_stability IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_memories_category ON memories(category);
        CREATE INDEX IF NOT EXISTS idx_memories_source ON memories(source);
        CREATE INDEX IF NOT EXISTS idx_memories_created ON memories(created_at);
        CREATE INDEX IF NOT EXISTS idx_memories_user_latest ON memories(user_id, is_latest, is_forgotten);

        -- Composite indexes for common query patterns
        CREATE INDEX IF NOT EXISTS idx_memories_search_composite ON memories(user_id, is_forgotten, is_latest, category);
        ",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

/// Add a new column to a table if it doesn't exist
/// SQLite doesn't have IF NOT EXISTS for ALTER TABLE ADD COLUMN, so we check first
#[allow(dead_code)]
async fn add_column_if_not_exists(
    conn: &Connection,
    table: &str,
    column: &str,
    column_def: &str,
) -> Result<()> {
    // Check if column exists
    let check_sql = format!(
        "SELECT COUNT(*) FROM pragma_table_info('{}') WHERE name = ?1",
        table
    );
    let mut rows = conn.query(&check_sql, libsql::params![column]).await?;
    let exists: i64 = if let Some(row) = rows.next().await? {
        row.get(0)?
    } else {
        0
    };

    if exists == 0 {
        let alter_sql = format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, column_def);
        conn.execute(&alter_sql, ()).await?;
        info!("Added column {}.{}", table, column);
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
    use libsql::Builder;

    #[tokio::test]
    async fn test_migrations_idempotent() -> Result<()> {
        let db_path =
            std::env::temp_dir().join(format!("engram-migrations-{}.db", uuid::Uuid::new_v4()));
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

    #[cfg(feature = "db_pool")]
    #[tokio::test]
    async fn test_rusqlite_migrations_idempotent() -> Result<()> {
        let db_path =
            std::env::temp_dir().join(format!("engram-rusqlite-migrations-{}.db", uuid::Uuid::new_v4()));
        let conn = rusqlite::Connection::open(&db_path)
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;

        run_migrations_rusqlite(&conn)?;
        run_migrations_rusqlite(&conn)?;

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_version WHERE version = ?1",
                rusqlite::params![MIGRATION_CREATE_SCHEMA],
                |row| row.get(0),
            )
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        assert_eq!(count, 1);

        let _ = std::fs::remove_file(&db_path);
        Ok(())
    }
}
