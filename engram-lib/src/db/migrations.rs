use crate::EngError;
use crate::Result;
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
const MIGRATION_DROP_IS_INFERENCE: i64 = 9;
const MIGRATION_SYNTHEOS_SERVICES: i64 = 10;
const MIGRATION_BRAIN_PATTERNS: i64 = 11;
const MIGRATION_APPROVALS: i64 = 12;
const MIGRATION_ERROR_EVENTS: i64 = 13;
const MIGRATION_BRAIN_META: i64 = 14;
const MIGRATION_PCA_MODELS: i64 = 15;
const MIGRATION_BRAIN_DREAM_RUNS: i64 = 16;
const MIGRATION_CRED_TABLES: i64 = 17;
const MIGRATION_API_KEY_HASH_UNIQUE: i64 = 18;
const MIGRATION_API_KEY_HASH_VERSION: i64 = 19;

/// Run ordered, idempotent migrations and record applied versions.
pub fn run_migrations(conn: &rusqlite::Connection) -> Result<()> {
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
        super::schema::create_tables(conn)?;
        record_migration(conn, MIGRATION_CREATE_SCHEMA, "create_tables")?;
    }

    if current_version < MIGRATION_ADD_MISSING_INDEXES {
        info!("Running migration 2: add_missing_indexes");
        run_migration_add_missing_indexes(conn)?;
        record_migration(conn, MIGRATION_ADD_MISSING_INDEXES, "add_missing_indexes")?;
    }

    if current_version < MIGRATION_PAGERANK_TABLES {
        info!("Running migration 3: add_pagerank_tables");
        run_migration_pagerank_tables(conn)?;
        record_migration(conn, MIGRATION_PAGERANK_TABLES, "add_pagerank_tables")?;
    }

    if current_version < MIGRATION_THYMUS_TENANT_SCOPE {
        info!("Running migration 4: thymus_tenant_scope");
        run_migration_thymus_tenant_scope(conn)?;
        record_migration(conn, MIGRATION_THYMUS_TENANT_SCOPE, "thymus_tenant_scope")?;
    }

    if current_version < MIGRATION_APP_STATE_TABLE {
        info!("Running migration 5: app_state_table");
        run_migration_app_state_table(conn)?;
        record_migration(conn, MIGRATION_APP_STATE_TABLE, "app_state_table")?;
    }

    if current_version < MIGRATION_BACKFILL_THYMUS_USER_ID {
        info!("Running migration 6: backfill_thymus_user_id");
        run_migration_backfill_thymus_user_id(conn)?;
        record_migration(
            conn,
            MIGRATION_BACKFILL_THYMUS_USER_ID,
            "backfill_thymus_user_id",
        )?;
    }

    if current_version < MIGRATION_VECTOR_SYNC_PENDING {
        info!("Running migration 7: vector_sync_pending");
        run_migration_vector_sync_pending(conn)?;
        record_migration(conn, MIGRATION_VECTOR_SYNC_PENDING, "vector_sync_pending")?;
    }

    if current_version < MIGRATION_ADD_COMMUNITY_ID {
        info!("Running migration 8: add_community_id");
        run_migration_add_community_id(conn)?;
        record_migration(conn, MIGRATION_ADD_COMMUNITY_ID, "add_community_id")?;
    }

    if current_version < MIGRATION_DROP_IS_INFERENCE {
        info!("Running migration 9: drop_is_inference");
        run_migration_drop_is_inference(conn)?;
        record_migration(conn, MIGRATION_DROP_IS_INFERENCE, "drop_is_inference")?;
    }

    if current_version < MIGRATION_SYNTHEOS_SERVICES {
        info!("Running migration 10: syntheos_services");
        run_migration_syntheos_services(conn)?;
        record_migration(conn, MIGRATION_SYNTHEOS_SERVICES, "syntheos_services")?;
    }

    if current_version < MIGRATION_BRAIN_PATTERNS {
        info!("Running migration 11: brain_patterns");
        run_migration_brain_patterns(conn)?;
        record_migration(conn, MIGRATION_BRAIN_PATTERNS, "brain_patterns")?;
    }

    if current_version < MIGRATION_APPROVALS {
        info!("Running migration 12: approvals");
        run_migration_approvals(conn)?;
        record_migration(conn, MIGRATION_APPROVALS, "approvals")?;
    }

    if current_version < MIGRATION_ERROR_EVENTS {
        info!("Running migration 13: error_events");
        run_migration_error_events(conn)?;
        record_migration(conn, MIGRATION_ERROR_EVENTS, "error_events")?;
    }

    if current_version < MIGRATION_BRAIN_META {
        info!("Running migration 14: brain_meta");
        run_migration_brain_meta(conn)?;
        record_migration(conn, MIGRATION_BRAIN_META, "brain_meta")?;
    }

    if current_version < MIGRATION_PCA_MODELS {
        info!("Running migration 15: brain_pca_models");
        run_migration_pca_models(conn)?;
        record_migration(conn, MIGRATION_PCA_MODELS, "brain_pca_models")?;
    }

    if current_version < MIGRATION_BRAIN_DREAM_RUNS {
        info!("Running migration 16: brain_dream_runs");
        run_migration_brain_dream_runs(conn)?;
        record_migration(conn, MIGRATION_BRAIN_DREAM_RUNS, "brain_dream_runs")?;
    }

    if current_version < MIGRATION_CRED_TABLES {
        info!("Running migration 17: cred_tables");
        run_migration_cred_tables(conn)?;
        record_migration(conn, MIGRATION_CRED_TABLES, "cred_tables")?;
    }

    if current_version < MIGRATION_API_KEY_HASH_UNIQUE {
        info!("Running migration 18: api_key_hash_unique");
        run_migration_api_key_hash_unique(conn)?;
        record_migration(conn, MIGRATION_API_KEY_HASH_UNIQUE, "api_key_hash_unique")?;
    }

    if current_version < MIGRATION_API_KEY_HASH_VERSION {
        info!("Running migration 19: api_key_hash_version");
        run_migration_api_key_hash_version(conn)?;
        record_migration(conn, MIGRATION_API_KEY_HASH_VERSION, "api_key_hash_version")?;
    }

    Ok(())
}

fn record_migration(conn: &rusqlite::Connection, version: i64, name: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO schema_version (version, name) VALUES (?1, ?2)",
        rusqlite::params![version, name],
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

fn run_migration_add_missing_indexes(conn: &rusqlite::Connection) -> Result<()> {
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

fn run_migration_pagerank_tables(conn: &rusqlite::Connection) -> Result<()> {
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

fn run_migration_thymus_tenant_scope(conn: &rusqlite::Connection) -> Result<()> {
    add_column_if_not_exists(
        conn,
        "session_quality",
        "user_id",
        "INTEGER NOT NULL DEFAULT 1",
    )?;
    add_column_if_not_exists(
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

fn run_migration_app_state_table(conn: &rusqlite::Connection) -> Result<()> {
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

fn run_migration_backfill_thymus_user_id(conn: &rusqlite::Connection) -> Result<()> {
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

fn run_migration_vector_sync_pending(conn: &rusqlite::Connection) -> Result<()> {
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

fn run_migration_add_community_id(conn: &rusqlite::Connection) -> Result<()> {
    add_column_if_not_exists(conn, "memories", "community_id", "INTEGER")?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_memories_community \
            ON memories(community_id) WHERE community_id IS NOT NULL;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

/// Migration 9: drop the is_inference dead column from memories.
/// Idempotent: only runs DROP COLUMN if the column still exists.
/// Requires SQLite 3.35+ (bundled rusqlite is 3.44+).
fn run_migration_drop_is_inference(conn: &rusqlite::Connection) -> Result<()> {
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('memories') WHERE name = ?1",
            rusqlite::params!["is_inference"],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if exists > 0 {
        conn.execute("ALTER TABLE memories DROP COLUMN is_inference", [])
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Dropped memories.is_inference column");
    }
    Ok(())
}

/// Migration 10: port the full syntheos services schema. Mirrors
/// the async variant exactly by running the shared SYNTHEOS_SERVICES_SQL
/// const through execute_batch.
fn run_migration_syntheos_services(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(crate::db::schema_sql::SYNTHEOS_SERVICES_SQL)
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

/// Migration 11: brain_patterns + brain_edges tables.
fn run_migration_brain_patterns(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS brain_patterns (
            id INTEGER PRIMARY KEY,
            user_id INTEGER NOT NULL,
            pattern BLOB NOT NULL,
            strength REAL NOT NULL DEFAULT 1.0,
            importance INTEGER NOT NULL DEFAULT 5,
            access_count INTEGER NOT NULL DEFAULT 0,
            last_activated_at TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_brain_patterns_user ON brain_patterns(user_id);
        CREATE INDEX IF NOT EXISTS idx_brain_patterns_strength ON brain_patterns(strength);

        CREATE TABLE IF NOT EXISTS brain_edges (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            source_id INTEGER NOT NULL,
            target_id INTEGER NOT NULL,
            weight REAL NOT NULL DEFAULT 1.0,
            edge_type TEXT NOT NULL DEFAULT 'association',
            user_id INTEGER NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(source_id, target_id, edge_type)
        );
        CREATE INDEX IF NOT EXISTS idx_brain_edges_source ON brain_edges(source_id);
        CREATE INDEX IF NOT EXISTS idx_brain_edges_target ON brain_edges(target_id);
        CREATE INDEX IF NOT EXISTS idx_brain_edges_user ON brain_edges(user_id);
        ",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

/// Migration 12: approvals table for human-in-the-loop approval workflow.
fn run_migration_approvals(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS approvals (
            id TEXT PRIMARY KEY,
            action TEXT NOT NULL,
            context TEXT,
            requester TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            decision_by TEXT,
            decision_reason TEXT,
            created_at TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            decided_at TEXT,
            user_id INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_approvals_status ON approvals(status);
        CREATE INDEX IF NOT EXISTS idx_approvals_expires ON approvals(expires_at);
        CREATE INDEX IF NOT EXISTS idx_approvals_user ON approvals(user_id);
        CREATE INDEX IF NOT EXISTS idx_approvals_user_status ON approvals(user_id, status);
        ",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

fn add_column_if_not_exists(
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

/// Migration 13: error_events table.
fn run_migration_error_events(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS error_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            source TEXT NOT NULL,
            level TEXT NOT NULL,
            message TEXT NOT NULL,
            context TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            user_id TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_error_events_level ON error_events(level);
        CREATE INDEX IF NOT EXISTS idx_error_events_source ON error_events(source);
        CREATE INDEX IF NOT EXISTS idx_error_events_created_at ON error_events(created_at);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

/// Migration 14: brain_meta key-value table.
fn run_migration_brain_meta(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS brain_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

fn run_migration_pca_models(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS brain_pca_models (
            id INTEGER PRIMARY KEY,
            source_dim INTEGER NOT NULL,
            target_dim INTEGER NOT NULL,
            fit_at TEXT NOT NULL,
            model_blob BLOB NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_pca_models_dims
            ON brain_pca_models(source_dim, target_dim);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

/// Migration 16: brain_dream_runs table for dream cycle audit trail.
fn run_migration_brain_dream_runs(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS brain_dream_runs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL,
            started_at TEXT NOT NULL DEFAULT (datetime('now')),
            finished_at TEXT,
            replay_count INTEGER NOT NULL DEFAULT 0,
            merge_count INTEGER NOT NULL DEFAULT 0,
            prune_count INTEGER NOT NULL DEFAULT 0,
            discover_count INTEGER NOT NULL DEFAULT 0,
            decorrelate_count INTEGER NOT NULL DEFAULT 0,
            resolve_count INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_brain_dream_runs_user
            ON brain_dream_runs(user_id);
        CREATE INDEX IF NOT EXISTS idx_brain_dream_runs_started
            ON brain_dream_runs(started_at);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

/// Migration 17: cred tables for credential management.
fn run_migration_cred_tables(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "-- Encrypted secrets storage
        CREATE TABLE IF NOT EXISTS cred_secrets (
            id INTEGER PRIMARY KEY,
            user_id INTEGER NOT NULL,
            name TEXT NOT NULL,
            category TEXT NOT NULL,
            secret_type TEXT NOT NULL,
            encrypted_data BLOB NOT NULL,
            nonce BLOB NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            UNIQUE(user_id, category, name)
        );

        -- Agent keys for service authentication
        CREATE TABLE IF NOT EXISTS cred_agent_keys (
            id INTEGER PRIMARY KEY,
            user_id INTEGER NOT NULL,
            key_hash TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            permissions TEXT NOT NULL,
            created_at TEXT NOT NULL,
            revoked_at TEXT,
            UNIQUE(user_id, name)
        );

        -- Audit log
        CREATE TABLE IF NOT EXISTS cred_audit (
            id INTEGER PRIMARY KEY,
            user_id INTEGER NOT NULL,
            agent_name TEXT,
            action TEXT NOT NULL,
            category TEXT NOT NULL,
            secret_name TEXT NOT NULL,
            access_tier TEXT,
            success INTEGER NOT NULL,
            timestamp TEXT NOT NULL
        );

        -- Recovery keys
        CREATE TABLE IF NOT EXISTS cred_recovery (
            id INTEGER PRIMARY KEY,
            user_id INTEGER NOT NULL UNIQUE,
            encrypted_master BLOB NOT NULL,
            recovery_hint TEXT,
            created_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_cred_secrets_user ON cred_secrets(user_id);
        CREATE INDEX IF NOT EXISTS idx_cred_audit_user ON cred_audit(user_id, timestamp);
        CREATE INDEX IF NOT EXISTS idx_cred_agent_keys_user ON cred_agent_keys(user_id);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

/// Migration 18: add UNIQUE index on api_keys(key_hash).
/// Checks for pre-existing duplicates first; skips index creation if any are
/// found rather than failing the migration (RB-L7).
fn run_migration_api_key_hash_unique(conn: &rusqlite::Connection) -> Result<()> {
    // Check for existing duplicate key_hash values.
    let dup_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM (SELECT key_hash FROM api_keys GROUP BY key_hash HAVING COUNT(*) > 1)",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    if dup_count > 0 {
        tracing::warn!(
            duplicates = dup_count,
            "api_keys has duplicate key_hash rows; skipping UNIQUE index creation (RB-L7)"
        );
        return Ok(());
    }

    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_api_keys_hash ON api_keys(key_hash);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

/// Migration 19: add hash_version column to api_keys for peppered hashing.
///
/// - v1 (default): legacy SHA-256(raw_key)
/// - v2: SHA-256(pepper || raw_key) when ENGRAM_API_KEY_PEPPER is set
fn run_migration_api_key_hash_version(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "ALTER TABLE api_keys ADD COLUMN hash_version INTEGER NOT NULL DEFAULT 1;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
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
pub fn validate_post_import(conn: &rusqlite::Connection) -> Result<PostImportValidation> {
    fn count(conn: &rusqlite::Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |row| row.get(0)).unwrap_or(0)
    }

    let memories_orphan_user = count(
        conn,
        "SELECT COUNT(*) FROM memories \
         WHERE user_id NOT IN (SELECT id FROM users)",
    );

    let memories_duplicate_latest = count(
        conn,
        "SELECT COUNT(*) FROM (
            SELECT root_memory_id FROM memories
            WHERE is_latest = 1 AND root_memory_id IS NOT NULL
            GROUP BY root_memory_id HAVING COUNT(*) > 1
         )",
    );

    let memories_missing_embedding = count(
        conn,
        "SELECT COUNT(*) FROM memories WHERE embedding_vec_1024 IS NULL \
         AND is_latest = 1 AND is_forgotten = 0",
    );

    let links_orphan = count(
        conn,
        "SELECT COUNT(*) FROM memory_links \
         WHERE source_id NOT IN (SELECT id FROM memories) \
            OR target_id NOT IN (SELECT id FROM memories)",
    );

    let audit_log_null_user = count(conn, "SELECT COUNT(*) FROM audit_log WHERE user_id IS NULL");

    let session_quality_zero_user = count(
        conn,
        "SELECT COUNT(*) FROM session_quality WHERE user_id = 0",
    );

    let behavioral_drift_zero_user = count(
        conn,
        "SELECT COUNT(*) FROM behavioral_drift_events WHERE user_id = 0",
    );

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

    #[tokio::test]
    async fn test_migrations_idempotent() -> Result<()> {
        let db_path = std::env::temp_dir().join(format!(
            "engram-migrations-{}.db",
            uuid::Uuid::new_v4()
        ));
        let conn = rusqlite::Connection::open(&db_path)
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;

        run_migrations(&conn)?;
        run_migrations(&conn)?;

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
