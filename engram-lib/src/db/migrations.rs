use crate::Result;
#[cfg(feature = "db_pool")]
use crate::EngError;
use libsql::Connection;
use tracing::info;

/// Migration versions - add new migrations here
const MIGRATION_CREATE_SCHEMA: i64 = 1;
const MIGRATION_ADD_MISSING_INDEXES: i64 = 2;
const MIGRATION_PAGERANK_TABLES: i64 = 3;
const MIGRATION_THYMUS_TENANT_SCOPE: i64 = 4;
const MIGRATION_APP_STATE_TABLE: i64 = 5;
const MIGRATION_BACKFILL_THYMUS_USER_ID: i64 = 6;
const MIGRATION_VECTOR_SYNC_PENDING: i64 = 7;
const MIGRATION_ADD_COMMUNITY_ID: i64 = 8;

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

    // Migration 4: Add user_id to thymus session_quality and
    // behavioral_drift_events so cross-tenant BOLA is impossible.
    if current_version < MIGRATION_THYMUS_TENANT_SCOPE {
        info!("Running migration 4: thymus_tenant_scope");
        run_migration_thymus_tenant_scope(conn).await?;
        record_migration(conn, MIGRATION_THYMUS_TENANT_SCOPE, "thymus_tenant_scope").await?;
    }

    // Migration 5: app_state table for the bootstrap sentinel and other
    // single-row flags the admin module already references.
    if current_version < MIGRATION_APP_STATE_TABLE {
        info!("Running migration 5: app_state_table");
        run_migration_app_state_table(conn).await?;
        record_migration(conn, MIGRATION_APP_STATE_TABLE, "app_state_table").await?;
    }

    // Migration 6: backfill any session_quality / behavioral_drift_events
    // rows that landed with the legacy DEFAULT 0 user_id. User 0 never
    // exists; re-home those rows onto the admin user (id = 1) so tenant
    // scoping queries actually see them.
    if current_version < MIGRATION_BACKFILL_THYMUS_USER_ID {
        info!("Running migration 6: backfill_thymus_user_id");
        run_migration_backfill_thymus_user_id(conn).await?;
        record_migration(
            conn,
            MIGRATION_BACKFILL_THYMUS_USER_ID,
            "backfill_thymus_user_id",
        )
        .await?;
    }

    // Migration 7: vector_sync_pending table used by memory::store and
    // memory::update to record LanceDB inserts/deletes that failed at write
    // time. A sweep task can replay rows and mark them resolved.
    if current_version < MIGRATION_VECTOR_SYNC_PENDING {
        info!("Running migration 7: vector_sync_pending");
        run_migration_vector_sync_pending(conn).await?;
        record_migration(conn, MIGRATION_VECTOR_SYNC_PENDING, "vector_sync_pending").await?;
    }

    // Migration 8: community_id column on memories. graph::communities reads
    // and writes this column but earlier builds never created it; community
    // detection and stats would fail at runtime.
    if current_version < MIGRATION_ADD_COMMUNITY_ID {
        info!("Running migration 8: add_community_id");
        run_migration_add_community_id(conn).await?;
        record_migration(conn, MIGRATION_ADD_COMMUNITY_ID, "add_community_id").await?;
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

    if current_version < MIGRATION_PAGERANK_TABLES {
        info!("Running migration 3: add_pagerank_tables");
        run_migration_pagerank_tables_rusqlite(conn)?;
        record_migration_rusqlite(conn, MIGRATION_PAGERANK_TABLES, "add_pagerank_tables")?;
    }

    if current_version < MIGRATION_THYMUS_TENANT_SCOPE {
        info!("Running migration 4: thymus_tenant_scope");
        run_migration_thymus_tenant_scope_rusqlite(conn)?;
        record_migration_rusqlite(conn, MIGRATION_THYMUS_TENANT_SCOPE, "thymus_tenant_scope")?;
    }

    if current_version < MIGRATION_APP_STATE_TABLE {
        info!("Running migration 5: app_state_table");
        run_migration_app_state_table_rusqlite(conn)?;
        record_migration_rusqlite(conn, MIGRATION_APP_STATE_TABLE, "app_state_table")?;
    }

    if current_version < MIGRATION_BACKFILL_THYMUS_USER_ID {
        info!("Running migration 6: backfill_thymus_user_id");
        run_migration_backfill_thymus_user_id_rusqlite(conn)?;
        record_migration_rusqlite(
            conn,
            MIGRATION_BACKFILL_THYMUS_USER_ID,
            "backfill_thymus_user_id",
        )?;
    }

    if current_version < MIGRATION_VECTOR_SYNC_PENDING {
        info!("Running migration 7: vector_sync_pending");
        run_migration_vector_sync_pending_rusqlite(conn)?;
        record_migration_rusqlite(conn, MIGRATION_VECTOR_SYNC_PENDING, "vector_sync_pending")?;
    }

    if current_version < MIGRATION_ADD_COMMUNITY_ID {
        info!("Running migration 8: add_community_id");
        run_migration_add_community_id_rusqlite(conn)?;
        record_migration_rusqlite(conn, MIGRATION_ADD_COMMUNITY_ID, "add_community_id")?;
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
        CREATE INDEX IF NOT EXISTS idx_memories_consolidated ON memories(is_consolidated) WHERE is_consolidated = 1;
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


/// Migration 4: Ensure session_quality and behavioral_drift_events carry user_id
/// so every read/write enforces tenant ownership. New columns default to the
/// admin user (id = 1) because user 0 never exists in the users table.
/// Databases that already ran an earlier build with DEFAULT 0 are repaired
/// by migration 6.
async fn run_migration_thymus_tenant_scope(conn: &Connection) -> Result<()> {
    add_column_if_not_exists(
        conn,
        "session_quality",
        "user_id",
        "INTEGER NOT NULL DEFAULT 1",
    )
    .await?;
    add_column_if_not_exists(
        conn,
        "behavioral_drift_events",
        "user_id",
        "INTEGER NOT NULL DEFAULT 1",
    )
    .await?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_session_quality_user ON session_quality(user_id);
         CREATE INDEX IF NOT EXISTS idx_behavioral_drift_user ON behavioral_drift_events(user_id);",
    )
    .await?;
    Ok(())
}

/// Migration 8: add community_id column to memories so Louvain community
/// detection has a place to persist cluster assignments.
async fn run_migration_add_community_id(conn: &Connection) -> Result<()> {
    add_column_if_not_exists(conn, "memories", "community_id", "INTEGER").await?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_memories_community \
            ON memories(community_id) WHERE community_id IS NOT NULL;",
    )
    .await?;
    Ok(())
}

/// Migration 7: track failed LanceDB writes so they can be replayed.
/// The table is intentionally append-only; a sweeper deletes rows after a
/// successful replay.
async fn run_migration_vector_sync_pending(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS vector_sync_pending (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            memory_id INTEGER NOT NULL,
            user_id INTEGER NOT NULL,
            op TEXT NOT NULL,
            error TEXT,
            attempts INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_attempt_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_vector_sync_memory
            ON vector_sync_pending(memory_id);
        CREATE INDEX IF NOT EXISTS idx_vector_sync_user
            ON vector_sync_pending(user_id);
        ",
    )
    .await?;
    Ok(())
}

/// Migration 6: backfill session_quality / behavioral_drift_events rows that
/// were inserted with user_id = 0 (the legacy DEFAULT from migration 4).
/// Re-home them onto the admin user (id = 1) so tenant scoping queries work.
async fn run_migration_backfill_thymus_user_id(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE session_quality SET user_id = 1 WHERE user_id = 0",
        (),
    )
    .await?;
    conn.execute(
        "UPDATE behavioral_drift_events SET user_id = 1 WHERE user_id = 0",
        (),
    )
    .await?;
    Ok(())
}

/// Migration 5: add the app_state key/value table used by admin settings and
/// the atomic bootstrap claim sentinel.
async fn run_migration_app_state_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS app_state (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
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
        CREATE INDEX IF NOT EXISTS idx_memories_consolidated ON memories(is_consolidated) WHERE is_consolidated = 1;
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

#[cfg(feature = "db_pool")]
fn run_migration_pagerank_tables_rusqlite(conn: &rusqlite::Connection) -> Result<()> {
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
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

#[cfg(feature = "db_pool")]
fn run_migration_thymus_tenant_scope_rusqlite(conn: &rusqlite::Connection) -> Result<()> {
    add_column_if_not_exists_rusqlite(
        conn,
        "session_quality",
        "user_id",
        "INTEGER NOT NULL DEFAULT 1",
    )?;
    add_column_if_not_exists_rusqlite(
        conn,
        "behavioral_drift_events",
        "user_id",
        "INTEGER NOT NULL DEFAULT 1",
    )?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_session_quality_user ON session_quality(user_id);
         CREATE INDEX IF NOT EXISTS idx_behavioral_drift_user ON behavioral_drift_events(user_id);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

#[cfg(feature = "db_pool")]
fn run_migration_app_state_table_rusqlite(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS app_state (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

#[cfg(feature = "db_pool")]
fn run_migration_backfill_thymus_user_id_rusqlite(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute(
        "UPDATE session_quality SET user_id = 1 WHERE user_id = 0",
        [],
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    conn.execute(
        "UPDATE behavioral_drift_events SET user_id = 1 WHERE user_id = 0",
        [],
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

#[cfg(feature = "db_pool")]
fn run_migration_vector_sync_pending_rusqlite(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS vector_sync_pending (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            memory_id INTEGER NOT NULL,
            user_id INTEGER NOT NULL,
            op TEXT NOT NULL,
            error TEXT,
            attempts INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_attempt_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_vector_sync_memory
            ON vector_sync_pending(memory_id);
        CREATE INDEX IF NOT EXISTS idx_vector_sync_user
            ON vector_sync_pending(user_id);
        ",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

#[cfg(feature = "db_pool")]
fn run_migration_add_community_id_rusqlite(conn: &rusqlite::Connection) -> Result<()> {
    add_column_if_not_exists_rusqlite(conn, "memories", "community_id", "INTEGER")?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_memories_community \
            ON memories(community_id) WHERE community_id IS NOT NULL;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

#[cfg(feature = "db_pool")]
fn add_column_if_not_exists_rusqlite(
    conn: &rusqlite::Connection,
    table: &str,
    column: &str,
    column_def: &str,
) -> Result<()> {
    let check_sql = format!(
        "SELECT COUNT(*) FROM pragma_table_info('{}') WHERE name = ?1",
        table
    );
    let exists: i64 = conn
        .query_row(&check_sql, rusqlite::params![column], |row| row.get(0))
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if exists == 0 {
        let alter_sql = format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, column_def);
        conn.execute(&alter_sql, [])
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Added column {}.{}", table, column);
    }
    Ok(())
}

/// Add a new column to a table if it doesn't exist
/// SQLite doesn't have IF NOT EXISTS for ALTER TABLE ADD COLUMN, so we check first
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

/// Summary of post-import integrity checks. Each field is a row count for a
/// condition that should be zero on a healthy import. A non-zero value means
/// the migrate tool (or operator) has cleanup work before enabling live
/// traffic.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct PostImportValidation {
    /// Memories whose user_id does not resolve to a row in users.
    pub memories_orphan_user: i64,
    /// Memory rows marked latest that share a root with another latest row.
    pub memories_duplicate_latest: i64,
    /// Memories with a NULL active embedding column.
    pub memories_missing_embedding: i64,
    /// memory_links rows whose source or target memory no longer exists.
    pub links_orphan: i64,
    /// audit_log rows with NULL user_id (pre-tenant legacy rows).
    pub audit_log_null_user: i64,
    /// session_quality rows with user_id = 0 (pre-migration-6 drift).
    pub session_quality_zero_user: i64,
    /// behavioral_drift_events rows with user_id = 0.
    pub behavioral_drift_zero_user: i64,
}

impl PostImportValidation {
    /// True when every field is zero.
    pub fn is_clean(&self) -> bool {
        self.memories_orphan_user == 0
            && self.memories_duplicate_latest == 0
            && self.memories_missing_embedding == 0
            && self.links_orphan == 0
            && self.audit_log_null_user == 0
            && self.session_quality_zero_user == 0
            && self.behavioral_drift_zero_user == 0
    }
}

/// Run a set of read-only integrity queries the migrate tool can surface in a
/// pre-flight report. Every query is tolerant of missing tables so operators
/// running this against a partially-migrated DB still get a useful summary.
pub async fn validate_post_import(conn: &Connection) -> Result<PostImportValidation> {
    async fn count(conn: &Connection, sql: &str) -> Result<i64> {
        match conn.query(sql, ()).await {
            Ok(mut rows) => match rows.next().await? {
                Some(row) => Ok(row.get(0).unwrap_or(0)),
                None => Ok(0),
            },
            Err(_) => Ok(0),
        }
    }

    let memories_orphan_user = count(
        conn,
        "SELECT COUNT(*) FROM memories \
         WHERE user_id NOT IN (SELECT id FROM users)",
    )
    .await?;

    let memories_duplicate_latest = count(
        conn,
        "SELECT COUNT(*) FROM (
            SELECT root_memory_id FROM memories
            WHERE is_latest = 1 AND root_memory_id IS NOT NULL
            GROUP BY root_memory_id HAVING COUNT(*) > 1
         )",
    )
    .await?;

    let memories_missing_embedding = count(
        conn,
        "SELECT COUNT(*) FROM memories WHERE embedding_vec_1024 IS NULL \
         AND is_latest = 1 AND is_forgotten = 0",
    )
    .await?;

    let links_orphan = count(
        conn,
        "SELECT COUNT(*) FROM memory_links \
         WHERE source_id NOT IN (SELECT id FROM memories) \
            OR target_id NOT IN (SELECT id FROM memories)",
    )
    .await?;

    let audit_log_null_user =
        count(conn, "SELECT COUNT(*) FROM audit_log WHERE user_id IS NULL").await?;

    let session_quality_zero_user = count(
        conn,
        "SELECT COUNT(*) FROM session_quality WHERE user_id = 0",
    )
    .await?;

    let behavioral_drift_zero_user = count(
        conn,
        "SELECT COUNT(*) FROM behavioral_drift_events WHERE user_id = 0",
    )
    .await?;

    Ok(PostImportValidation {
        memories_orphan_user,
        memories_duplicate_latest,
        memories_missing_embedding,
        links_orphan,
        audit_log_null_user,
        session_quality_zero_user,
        behavioral_drift_zero_user,
    })
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
