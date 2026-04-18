pub use super::types::PostImportValidation;

use crate::EngError;
use crate::Result;
use serde::Serialize;
use tracing::info;

// ---------------------------------------------------------------------------
// Migration descriptor
// ---------------------------------------------------------------------------

/// A single schema migration with an optional inverse.
///
/// `down` is `None` for all legacy migrations where generating safe inverse
/// SQL is not practical (DROP COLUMN on SQLite requires a full table rebuild
/// for anything added before SQLite 3.35, and reverting data-loss operations
/// such as DROP COLUMN is impossible without a backup). Only purely additive
/// migrations added after this refactor carry a `down` implementation.
pub struct Migration {
    pub version: u32,
    pub description: &'static str,
    pub up: fn(&rusqlite::Connection) -> Result<()>,
    pub down: Option<fn(&rusqlite::Connection) -> Result<()>>,
    /// When true the up/down fn is wrapped in a SAVEPOINT so it rolls back
    /// automatically on failure.
    pub transactional: bool,
}

// ---------------------------------------------------------------------------
// Migration plan (returned by dry_run and migrate_down)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct MigrationPlan {
    pub version: u32,
    pub description: String,
    pub direction: String,
}

// ---------------------------------------------------------------------------
// Migration status
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct MigrationStatus {
    pub current_version: u32,
    /// Migrations whose `up` has not yet been applied.
    pub pending_up: Vec<MigrationInfo>,
    /// Applied migrations that have a `down` fn and can therefore be reverted.
    pub revertible_down: Vec<MigrationInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MigrationInfo {
    pub version: u32,
    pub description: String,
    pub has_down: bool,
}

// ---------------------------------------------------------------------------
// The canonical migration list
// ---------------------------------------------------------------------------

pub static MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        description: "create_tables",
        up: |conn| super::schema::create_tables(conn),
        // Dropping the initial schema would destroy all data; no inverse.
        down: None,
        transactional: false,
    },
    Migration {
        version: 2,
        description: "add_missing_indexes",
        up: run_migration_add_missing_indexes,
        // Indexes are covered by CREATE INDEX IF NOT EXISTS; dropping them
        // individually is safe, but the sheer number makes the inverse
        // fragile and the original DB lacked them, so no inverse needed.
        down: None,
        transactional: false,
    },
    Migration {
        version: 3,
        description: "add_pagerank_tables",
        up: run_migration_pagerank_tables,
        down: None,
        transactional: false,
    },
    Migration {
        version: 4,
        description: "thymus_tenant_scope",
        up: run_migration_thymus_tenant_scope,
        down: None,
        transactional: false,
    },
    Migration {
        version: 5,
        description: "app_state_table",
        up: run_migration_app_state_table,
        down: None,
        transactional: false,
    },
    Migration {
        version: 6,
        description: "backfill_thymus_user_id",
        up: run_migration_backfill_thymus_user_id,
        // Data update; original values cannot be recovered.
        down: None,
        transactional: false,
    },
    Migration {
        version: 7,
        description: "vector_sync_pending",
        up: run_migration_vector_sync_pending,
        down: None,
        transactional: false,
    },
    Migration {
        version: 8,
        description: "add_community_id",
        up: run_migration_add_community_id,
        down: None,
        transactional: false,
    },
    Migration {
        version: 9,
        description: "drop_is_inference",
        up: run_migration_drop_is_inference,
        // DROP COLUMN is destructive; there is no way to recover the
        // original data without a backup.
        down: None,
        transactional: false,
    },
    Migration {
        version: 10,
        description: "syntheos_services",
        up: run_migration_syntheos_services,
        down: None,
        transactional: false,
    },
    Migration {
        version: 11,
        description: "brain_patterns",
        up: run_migration_brain_patterns,
        down: None,
        transactional: false,
    },
    Migration {
        version: 12,
        description: "approvals",
        up: run_migration_approvals,
        down: None,
        transactional: false,
    },
    Migration {
        version: 13,
        description: "error_events",
        up: run_migration_error_events,
        down: None,
        transactional: false,
    },
    Migration {
        version: 14,
        description: "brain_meta",
        up: run_migration_brain_meta,
        down: None,
        transactional: false,
    },
    Migration {
        version: 15,
        description: "brain_pca_models",
        up: run_migration_pca_models,
        down: None,
        transactional: false,
    },
    Migration {
        version: 16,
        description: "brain_dream_runs",
        up: run_migration_brain_dream_runs,
        down: None,
        transactional: false,
    },
    Migration {
        version: 17,
        description: "cred_tables",
        up: run_migration_cred_tables,
        down: None,
        transactional: false,
    },
    Migration {
        version: 18,
        description: "api_key_hash_unique",
        up: run_migration_api_key_hash_unique,
        // The UNIQUE index was added conditionally; dropping it is safe,
        // but we leave it None because we cannot know which DBs skipped it.
        down: None,
        transactional: false,
    },
    Migration {
        version: 19,
        description: "api_key_hash_version",
        up: run_migration_api_key_hash_version,
        // Purely additive ALTER TABLE ADD COLUMN. SQLite 3.35+ DROP COLUMN
        // is the safe inverse because the column has no constraints that
        // would require a full table rebuild.
        down: Some(down_migration_api_key_hash_version),
        transactional: true,
    },
    Migration {
        version: 20,
        description: "link_covering_indexes",
        up: run_migration_link_covering_indexes,
        // Two covering indexes; DROP INDEX is the clean inverse.
        down: Some(down_migration_link_covering_indexes),
        transactional: true,
    },
    Migration {
        version: 21,
        description: "upload_sessions",
        up: run_migration_upload_sessions,
        // Two new tables with no FK references from other tables; DROP TABLE
        // is the clean inverse.
        down: Some(down_migration_upload_sessions),
        transactional: true,
    },
    Migration {
        version: 22,
        description: "service_dead_letters",
        up: run_migration_service_dead_letters,
        // New table with no FK references; DROP TABLE is the clean inverse.
        down: Some(down_migration_service_dead_letters),
        transactional: true,
    },
];

// ---------------------------------------------------------------------------
// Legacy version constants (kept for compatibility with existing call sites)
// ---------------------------------------------------------------------------

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
const MIGRATION_LINK_COVERING_INDEXES: i64 = 20;
const MIGRATION_UPLOAD_SESSIONS: i64 = 21;
const MIGRATION_SERVICE_DEAD_LETTERS: i64 = 22;

// ---------------------------------------------------------------------------
// Up path (unchanged behavior)
// ---------------------------------------------------------------------------

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

    if current_version < MIGRATION_LINK_COVERING_INDEXES {
        info!("Running migration 20: link_covering_indexes");
        run_migration_link_covering_indexes(conn)?;
        record_migration(
            conn,
            MIGRATION_LINK_COVERING_INDEXES,
            "link_covering_indexes",
        )?;
    }

    if current_version < MIGRATION_UPLOAD_SESSIONS {
        info!("Running migration 21: upload_sessions");
        run_migration_upload_sessions(conn)?;
        record_migration(conn, MIGRATION_UPLOAD_SESSIONS, "upload_sessions")?;
    }

    if current_version < MIGRATION_SERVICE_DEAD_LETTERS {
        info!("Running migration 22: service_dead_letters");
        run_migration_service_dead_letters(conn)?;
        record_migration(conn, MIGRATION_SERVICE_DEAD_LETTERS, "service_dead_letters")?;
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

fn remove_migration_record(conn: &rusqlite::Connection, version: u32) -> Result<()> {
    conn.execute(
        "DELETE FROM schema_version WHERE version = ?1",
        rusqlite::params![version],
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Down path
// ---------------------------------------------------------------------------

/// Walk the migration list down from `current_version` to `target_version`
/// (exclusive), building a plan of what would be reverted.
///
/// Returns `Err` immediately if any migration in the range has `down: None`
/// because it is not safe to skip an intermediate migration.
fn build_down_plan(current_version: u32, target_version: u32) -> Result<Vec<MigrationPlan>> {
    if target_version >= current_version {
        return Ok(vec![]);
    }

    // Collect migrations to revert in reverse order (highest version first).
    let mut plan = Vec::new();
    for m in MIGRATIONS.iter().rev() {
        if m.version > current_version || m.version <= target_version {
            continue;
        }
        if m.down.is_none() {
            return Err(EngError::Internal(format!(
                "migration {} ({}) has no down; cannot roll back past version {}",
                m.version, m.description, target_version
            )));
        }
        plan.push(MigrationPlan {
            version: m.version,
            description: m.description.to_string(),
            direction: "down".to_string(),
        });
    }
    Ok(plan)
}

/// Roll the database schema back to `target_version`.
///
/// If `dry_run` is true, returns the plan without executing anything.
/// Fails fast if any migration in the range has no `down` implementation.
pub async fn migrate_down(
    db: &super::Database,
    target_version: u32,
    dry_run: bool,
) -> Result<Vec<MigrationPlan>> {
    // Read current version.
    let current_version: u32 = db
        .read(|conn| {
            conn.query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|v| v as u32)
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))
        })
        .await?;

    let plan = build_down_plan(current_version, target_version)?;

    if dry_run || plan.is_empty() {
        return Ok(plan);
    }

    // Execute each down migration in order (highest version first).
    for step in &plan {
        let version = step.version;
        let m = MIGRATIONS
            .iter()
            .find(|m| m.version == version)
            .ok_or_else(|| {
                EngError::Internal(format!("migration {version} not found in MIGRATIONS slice"))
            })?;
        // We need a mutable connection for SAVEPOINT semantics.
        let down_fn = m.down.unwrap();
        let transactional = m.transactional;

        db.write(move |conn| {
            if transactional {
                let sp_name = format!("sp_down_{version}");
                conn.execute_batch(&format!("SAVEPOINT {sp_name}"))
                    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
                match down_fn(conn) {
                    Ok(()) => {
                        conn.execute_batch(&format!("RELEASE {sp_name}"))
                            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
                    }
                    Err(e) => {
                        let _ = conn.execute_batch(&format!("ROLLBACK TO {sp_name}"));
                        return Err(e);
                    }
                }
            } else {
                down_fn(conn)?;
            }
            remove_migration_record(conn, version)?;
            info!("Rolled back migration {version}");
            Ok(())
        })
        .await?;
    }

    Ok(plan)
}

/// Return the current migration status: which version is applied, which
/// migrations are pending (not yet applied), and which applied migrations
/// can be reverted.
pub async fn migration_status(
    db: &super::Database,
) -> Result<MigrationStatus> {
    let current_version: u32 = db
        .read(|conn| {
            conn.query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|v| v as u32)
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))
        })
        .await?;

    let pending_up: Vec<MigrationInfo> = MIGRATIONS
        .iter()
        .filter(|m| m.version > current_version)
        .map(|m| MigrationInfo {
            version: m.version,
            description: m.description.to_string(),
            has_down: m.down.is_some(),
        })
        .collect();

    let revertible_down: Vec<MigrationInfo> = MIGRATIONS
        .iter()
        .filter(|m| m.version <= current_version && m.down.is_some())
        .map(|m| MigrationInfo {
            version: m.version,
            description: m.description.to_string(),
            has_down: true,
        })
        .collect();

    Ok(MigrationStatus {
        current_version,
        pending_up,
        revertible_down,
    })
}

// ---------------------------------------------------------------------------
// Up migration implementations
// ---------------------------------------------------------------------------

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
    conn.execute_batch("ALTER TABLE api_keys ADD COLUMN hash_version INTEGER NOT NULL DEFAULT 1;")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

/// Down for migration 19: drop the hash_version column. Requires SQLite 3.35+.
fn down_migration_api_key_hash_version(conn: &rusqlite::Connection) -> Result<()> {
    // Only drop if the column still exists (idempotent).
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('api_keys') WHERE name = 'hash_version'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if exists > 0 {
        conn.execute_batch("ALTER TABLE api_keys DROP COLUMN hash_version;")
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Dropped api_keys.hash_version column (migration 19 down)");
    }
    Ok(())
}

/// Migration 20: covering indexes on memory_links for graph neighbor and
/// link-fetch queries. Both source_id and target_id get a covering index
/// that includes the join columns (similarity, type) so the query planner
/// can satisfy the common UNION query from the index alone without touching
/// the main table.
fn run_migration_link_covering_indexes(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_links_source_covering \
             ON memory_links(source_id, target_id, similarity, type);
         CREATE INDEX IF NOT EXISTS idx_links_target_covering \
             ON memory_links(target_id, source_id, similarity, type);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

/// Down for migration 20: drop the covering indexes added above.
fn down_migration_link_covering_indexes(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "DROP INDEX IF EXISTS idx_links_source_covering;
         DROP INDEX IF EXISTS idx_links_target_covering;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    info!("Dropped link covering indexes (migration 20 down)");
    Ok(())
}

/// Migration 21: resumable upload sessions + per-chunk persistence. Large
/// ingestion payloads can now be uploaded piece by piece and survive transient
/// network failures. Chunks are content-hashed so an interrupted client can
/// probe `status` and replay only what it needs.
fn run_migration_upload_sessions(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS upload_sessions (
             upload_id TEXT PRIMARY KEY,
             user_id INTEGER NOT NULL,
             filename TEXT,
             content_type TEXT,
             source TEXT NOT NULL DEFAULT 'upload',
             total_size INTEGER,
             total_chunks INTEGER,
             chunk_size INTEGER NOT NULL,
             status TEXT NOT NULL DEFAULT 'active',
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             completed_at TEXT,
             expires_at TEXT NOT NULL,
             final_sha256 TEXT
         );
         CREATE INDEX IF NOT EXISTS idx_upload_sessions_user ON upload_sessions(user_id);
         CREATE INDEX IF NOT EXISTS idx_upload_sessions_status ON upload_sessions(status);
         CREATE INDEX IF NOT EXISTS idx_upload_sessions_expires ON upload_sessions(expires_at);

         CREATE TABLE IF NOT EXISTS upload_chunks (
             upload_id TEXT NOT NULL,
             chunk_index INTEGER NOT NULL,
             chunk_hash TEXT NOT NULL,
             size INTEGER NOT NULL,
             data BLOB NOT NULL,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             PRIMARY KEY (upload_id, chunk_index),
             FOREIGN KEY (upload_id) REFERENCES upload_sessions(upload_id) ON DELETE CASCADE
         );",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

/// Down for migration 21: drop the upload tables added above.
/// upload_chunks is dropped first because it has a FK to upload_sessions.
fn down_migration_upload_sessions(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS upload_chunks;
         DROP TABLE IF EXISTS upload_sessions;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    info!("Dropped upload_sessions and upload_chunks tables (migration 21 down)");
    Ok(())
}

/// Migration 22: dead-letter table for internal service calls (reranker,
/// embedder, brain, etc.). Records calls that exhausted all retry attempts
/// so operators can inspect and replay them.
fn run_migration_service_dead_letters(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS service_dead_letters (
             id INTEGER PRIMARY KEY,
             service TEXT NOT NULL,
             operation TEXT NOT NULL,
             payload_json TEXT,
             error TEXT,
             retry_count INTEGER,
             created_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE INDEX IF NOT EXISTS idx_sdl_service ON service_dead_letters(service);
         CREATE INDEX IF NOT EXISTS idx_sdl_created ON service_dead_letters(created_at DESC);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    info!("Created service_dead_letters table (migration 22)");
    Ok(())
}

fn down_migration_service_dead_letters(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch("DROP TABLE IF EXISTS service_dead_letters;")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    info!("Dropped service_dead_letters table (migration 22 down)");
    Ok(())
}

// ---------------------------------------------------------------------------
// Post-import validation (unchanged)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn open_test_db() -> rusqlite::Connection {
        let db_path =
            std::env::temp_dir().join(format!("engram-migrations-{}.db", uuid::Uuid::new_v4()));
        rusqlite::Connection::open(&db_path).expect("open test db")
    }

    /// Helper: apply up through migration 21 then manually fake a lower
    /// current_version so down tests can operate on a small slice.
    fn apply_migrations_up_to(conn: &rusqlite::Connection, version: u32) {
        // Always create the schema_version table first.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();

        for m in MIGRATIONS.iter() {
            if m.version > version {
                break;
            }
            (m.up)(conn).unwrap_or_else(|e| {
                // Some up fns are tolerant of missing tables (e.g. backfill).
                // Swallow errors so we can run partial migrations in tests.
                let _ = e;
            });
            conn.execute(
                "INSERT OR IGNORE INTO schema_version (version, name) VALUES (?1, ?2)",
                rusqlite::params![m.version, m.description],
            )
            .unwrap();
        }
    }

    #[tokio::test]
    async fn test_migrations_idempotent() -> Result<()> {
        let db_path =
            std::env::temp_dir().join(format!("engram-migrations-{}.db", uuid::Uuid::new_v4()));
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

    // -----------------------------------------------------------------------
    // Down migration tests (operate directly on rusqlite::Connection to avoid
    // needing a full async Database pool in unit tests)
    // -----------------------------------------------------------------------

    /// build_down_plan with dry_run semantics: verify it returns the right
    /// steps without touching the DB.
    #[test]
    fn test_migrate_down_dry_run_returns_plan() {
        // Migrations 19, 20, 21 all have down fns.
        let plan = build_down_plan(21, 18).expect("build_down_plan should succeed");
        assert_eq!(plan.len(), 3, "should plan 3 steps (21, 20, 19)");
        assert_eq!(plan[0].version, 21);
        assert_eq!(plan[1].version, 20);
        assert_eq!(plan[2].version, 19);
        for step in &plan {
            assert_eq!(step.direction, "down");
        }
    }

    /// Verify that down fns for migrations 19, 20, 21 actually execute
    /// without error against a real (in-memory) SQLite DB.
    #[test]
    fn test_migrate_down_executes_reversible() {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory db");
        // Apply full migration set so all tables/indexes/columns exist.
        apply_migrations_up_to(&conn, 21);

        // Execute down fns for 21, 20, 19 in reverse order.
        for version in [21u32, 20, 19] {
            let m = MIGRATIONS
                .iter()
                .find(|m| m.version == version)
                .expect("migration exists");
            let down_fn = m.down.expect("down fn present");
            down_fn(&conn).unwrap_or_else(|e| panic!("down {} failed: {e}", version));
        }

        // Verify upload tables are gone.
        let upload_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='upload_sessions'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(upload_exists, 0, "upload_sessions should be dropped");

        // Verify covering indexes are gone.
        let idx_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_links_source_covering'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(idx_exists, 0, "idx_links_source_covering should be dropped");

        // Verify hash_version column is gone.
        let col_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('api_keys') WHERE name='hash_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(col_exists, 0, "hash_version column should be dropped");
    }

    /// Verify that attempting to roll back past a migration with no down fn
    /// returns an error.
    #[test]
    fn test_migrate_down_refuses_when_down_missing() {
        // Migration 18 has down: None, so rolling back from 21 to 17 should fail.
        let result = build_down_plan(21, 17);
        assert!(
            result.is_err(),
            "should fail because migration 18 has no down"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("migration 18") || err_msg.contains("no down"),
            "error message should mention migration 18 or 'no down': {err_msg}"
        );
    }
}
