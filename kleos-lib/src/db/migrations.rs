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
    Migration {
        version: 23,
        description: "memories_list_covering_index",
        up: run_migration_memories_list_covering_index,
        down: Some(down_migration_memories_list_covering_index),
        transactional: true,
    },
    Migration {
        version: 24,
        description: "commerce_tables",
        up: run_migration_commerce_tables,
        down: Some(down_migration_commerce_tables),
        transactional: true,
    },
    Migration {
        version: 25,
        description: "drop_user_id_memory_core",
        up: run_migration_drop_user_id_memory_core,
        // DROP COLUMN is destructive; no safe inverse without a backup.
        down: None,
        transactional: false,
    },
    Migration {
        version: 26,
        description: "drop_user_id_scratchpad",
        up: run_migration_drop_user_id_scratchpad,
        // 12-step table rebuild; no safe inverse.
        down: None,
        transactional: false,
    },
    Migration {
        version: 27,
        description: "drop_user_id_sessions",
        up: run_migration_drop_user_id_sessions,
        // DROP COLUMN is destructive; no safe inverse without a backup.
        down: None,
        transactional: false,
    },
    Migration {
        version: 28,
        description: "drop_user_id_chiasm",
        up: run_migration_drop_user_id_chiasm,
        // DROP COLUMN is destructive; no safe inverse without a backup.
        down: None,
        transactional: false,
    },
    Migration {
        version: 29,
        description: "drop_user_id_approvals",
        up: run_migration_drop_user_id_approvals,
        // DROP COLUMN is destructive; no safe inverse without a backup.
        down: None,
        transactional: false,
    },
    Migration {
        version: 30,
        description: "drop_user_id_broca",
        up: run_migration_drop_user_id_broca,
        // DROP COLUMN is destructive; no safe inverse without a backup.
        down: None,
        transactional: false,
    },
    Migration {
        version: 31,
        description: "drop_user_id_projects",
        up: run_migration_drop_user_id_projects,
        // 12-step table rebuild; no safe inverse.
        down: None,
        transactional: false,
    },
    Migration {
        version: 32,
        description: "drop_user_id_activity",
        up: run_migration_drop_user_id_activity,
        // DROP COLUMN is destructive; no safe inverse without a backup.
        down: None,
        transactional: false,
    },
    Migration {
        version: 33,
        description: "drop_user_id_webhooks",
        up: run_migration_drop_user_id_webhooks,
        // DROP COLUMN is destructive; no safe inverse without a backup.
        down: None,
        transactional: false,
    },
    Migration {
        version: 34,
        description: "drop_user_id_axon",
        up: run_migration_drop_user_id_axon,
        // DROP COLUMN is destructive; no safe inverse without a backup.
        down: None,
        transactional: false,
    },
    Migration {
        version: 35,
        description: "drop_user_id_growth",
        up: run_migration_drop_user_id_growth,
        // DROP COLUMN is destructive; no safe inverse without a backup.
        down: None,
        transactional: false,
    },
    Migration {
        version: 36,
        description: "drop_user_id_ingestion_hashes",
        up: run_migration_drop_user_id_ingestion_hashes,
        // DROP COLUMN / PK rebuild is destructive; no safe inverse without a backup.
        down: None,
        transactional: false,
    },
    Migration {
        version: 37,
        description: "drop_user_id_loom",
        up: run_migration_drop_user_id_loom,
        // UNIQUE rebuild + DROP COLUMN is destructive; no safe inverse without a backup.
        down: None,
        transactional: false,
    },
    Migration {
        version: 38,
        description: "drop_user_id_graph",
        up: run_migration_drop_user_id_graph,
        // UNIQUE/PK rebuild + DROP COLUMN is destructive; no safe inverse without a backup.
        down: None,
        transactional: false,
    },
    Migration {
        version: 39,
        description: "drop_user_id_thymus",
        up: run_migration_drop_user_id_thymus,
        // DROP COLUMN + index swap is destructive; no safe inverse without a backup.
        down: None,
        transactional: false,
    },
    Migration {
        version: 40,
        description: "drop_user_id_portability",
        up: run_migration_drop_user_id_portability,
        // UNIQUE rebuild + DROP COLUMN is destructive; no safe inverse without a backup.
        down: None,
        transactional: false,
    },
    Migration {
        version: 41,
        description: "drop_user_id_intelligence",
        up: run_migration_drop_user_id_intelligence,
        // UNIQUE rebuild + DROP COLUMN is destructive; no safe inverse without a backup.
        down: None,
        transactional: false,
    },
    Migration {
        version: 42,
        description: "drop_user_id_skills",
        up: run_migration_drop_user_id_skills,
        // Shape B + FTS shadow rebuild is destructive; no safe inverse without a backup.
        down: None,
        transactional: false,
    },
    Migration {
        version: 43,
        description: "drop_user_id_episodes",
        up: run_migration_drop_user_id_episodes,
        // DROP INDEX + DROP COLUMN is destructive; no safe inverse without a backup.
        down: None,
        transactional: false,
    },
    // C-R3-004: re-add user_id to monolith projects + broca_actions so
    // single-DB deployments are safe even when tenant sharding is disabled.
    // Tenant shards remain user_id-free; only the monolith carries these.
    Migration {
        version: 44,
        description: "readd_user_id_projects",
        up: run_migration_readd_user_id_projects,
        down: None,
        transactional: false,
    },
    Migration {
        version: 45,
        description: "readd_user_id_broca",
        up: run_migration_readd_user_id_broca,
        down: None,
        transactional: false,
    },
    // Sparkling Fairy Stage 1: identity tables for PIV-Everywhere auth.
    Migration {
        version: 46,
        description: "identity_keys_and_identities",
        up: run_migration_identity_tables,
        down: None,
        transactional: true,
    },
    Migration {
        version: 47,
        description: "audit_log_identity_columns",
        up: run_migration_audit_identity_columns,
        down: None,
        transactional: false,
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
const MIGRATION_MEMORIES_LIST_COVERING_INDEX: i64 = 23;
const MIGRATION_COMMERCE_TABLES: i64 = 24;
const MIGRATION_DROP_USER_ID_MEMORY_CORE: i64 = 25;
const MIGRATION_DROP_USER_ID_SCRATCHPAD: i64 = 26;
const MIGRATION_DROP_USER_ID_SESSIONS: i64 = 27;
const MIGRATION_DROP_USER_ID_CHIASM: i64 = 28;
const MIGRATION_DROP_USER_ID_APPROVALS: i64 = 29;
const MIGRATION_DROP_USER_ID_BROCA: i64 = 30;
const MIGRATION_DROP_USER_ID_PROJECTS: i64 = 31;
const MIGRATION_DROP_USER_ID_ACTIVITY: i64 = 32;
const MIGRATION_DROP_USER_ID_WEBHOOKS: i64 = 33;
const MIGRATION_DROP_USER_ID_AXON: i64 = 34;
const MIGRATION_DROP_USER_ID_GROWTH: i64 = 35;
const MIGRATION_DROP_USER_ID_INGESTION_HASHES: i64 = 36;
const MIGRATION_DROP_USER_ID_LOOM: i64 = 37;
const MIGRATION_DROP_USER_ID_GRAPH: i64 = 38;
const MIGRATION_DROP_USER_ID_THYMUS: i64 = 39;
const MIGRATION_DROP_USER_ID_PORTABILITY: i64 = 40;
const MIGRATION_DROP_USER_ID_INTELLIGENCE: i64 = 41;
const MIGRATION_DROP_USER_ID_SKILLS: i64 = 42;
const MIGRATION_DROP_USER_ID_EPISODES: i64 = 43;
const MIGRATION_READD_USER_ID_PROJECTS: i64 = 44;
const MIGRATION_READD_USER_ID_BROCA: i64 = 45;
const MIGRATION_IDENTITY_TABLES: i64 = 46;
const MIGRATION_AUDIT_IDENTITY_COLUMNS: i64 = 47;

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

    if current_version < MIGRATION_MEMORIES_LIST_COVERING_INDEX {
        info!("Running migration 23: memories_list_covering_index");
        run_migration_memories_list_covering_index(conn)?;
        record_migration(
            conn,
            MIGRATION_MEMORIES_LIST_COVERING_INDEX,
            "memories_list_covering_index",
        )?;
    }

    if current_version < MIGRATION_COMMERCE_TABLES {
        info!("Running migration 24: commerce_tables");
        run_migration_commerce_tables(conn)?;
        record_migration(conn, MIGRATION_COMMERCE_TABLES, "commerce_tables")?;
    }

    if current_version < MIGRATION_DROP_USER_ID_MEMORY_CORE {
        info!("Running migration 25: drop_user_id_memory_core");
        run_migration_drop_user_id_memory_core(conn)?;
        record_migration(
            conn,
            MIGRATION_DROP_USER_ID_MEMORY_CORE,
            "drop_user_id_memory_core",
        )?;
    }

    if current_version < MIGRATION_DROP_USER_ID_SCRATCHPAD {
        info!("Running migration 26: drop_user_id_scratchpad");
        run_migration_drop_user_id_scratchpad(conn)?;
        record_migration(
            conn,
            MIGRATION_DROP_USER_ID_SCRATCHPAD,
            "drop_user_id_scratchpad",
        )?;
    }

    if current_version < MIGRATION_DROP_USER_ID_SESSIONS {
        info!("Running migration 27: drop_user_id_sessions");
        run_migration_drop_user_id_sessions(conn)?;
        record_migration(
            conn,
            MIGRATION_DROP_USER_ID_SESSIONS,
            "drop_user_id_sessions",
        )?;
    }

    if current_version < MIGRATION_DROP_USER_ID_CHIASM {
        info!("Running migration 28: drop_user_id_chiasm");
        run_migration_drop_user_id_chiasm(conn)?;
        record_migration(conn, MIGRATION_DROP_USER_ID_CHIASM, "drop_user_id_chiasm")?;
    }

    if current_version < MIGRATION_DROP_USER_ID_APPROVALS {
        info!("Running migration 29: drop_user_id_approvals");
        run_migration_drop_user_id_approvals(conn)?;
        record_migration(
            conn,
            MIGRATION_DROP_USER_ID_APPROVALS,
            "drop_user_id_approvals",
        )?;
    }

    if current_version < MIGRATION_DROP_USER_ID_BROCA {
        info!("Running migration 30: drop_user_id_broca");
        run_migration_drop_user_id_broca(conn)?;
        record_migration(conn, MIGRATION_DROP_USER_ID_BROCA, "drop_user_id_broca")?;
    }

    if current_version < MIGRATION_DROP_USER_ID_PROJECTS {
        info!("Running migration 31: drop_user_id_projects");
        run_migration_drop_user_id_projects(conn)?;
        record_migration(
            conn,
            MIGRATION_DROP_USER_ID_PROJECTS,
            "drop_user_id_projects",
        )?;
    }

    if current_version < MIGRATION_DROP_USER_ID_ACTIVITY {
        info!("Running migration 32: drop_user_id_activity");
        run_migration_drop_user_id_activity(conn)?;
        record_migration(
            conn,
            MIGRATION_DROP_USER_ID_ACTIVITY,
            "drop_user_id_activity",
        )?;
    }

    if current_version < MIGRATION_DROP_USER_ID_WEBHOOKS {
        info!("Running migration 33: drop_user_id_webhooks");
        run_migration_drop_user_id_webhooks(conn)?;
        record_migration(
            conn,
            MIGRATION_DROP_USER_ID_WEBHOOKS,
            "drop_user_id_webhooks",
        )?;
    }

    if current_version < MIGRATION_DROP_USER_ID_AXON {
        info!("Running migration 34: drop_user_id_axon");
        run_migration_drop_user_id_axon(conn)?;
        record_migration(conn, MIGRATION_DROP_USER_ID_AXON, "drop_user_id_axon")?;
    }

    if current_version < MIGRATION_DROP_USER_ID_GROWTH {
        info!("Running migration 35: drop_user_id_growth");
        run_migration_drop_user_id_growth(conn)?;
        record_migration(conn, MIGRATION_DROP_USER_ID_GROWTH, "drop_user_id_growth")?;
    }

    if current_version < MIGRATION_DROP_USER_ID_INGESTION_HASHES {
        info!("Running migration 36: drop_user_id_ingestion_hashes");
        run_migration_drop_user_id_ingestion_hashes(conn)?;
        record_migration(
            conn,
            MIGRATION_DROP_USER_ID_INGESTION_HASHES,
            "drop_user_id_ingestion_hashes",
        )?;
    }

    if current_version < MIGRATION_DROP_USER_ID_LOOM {
        info!("Running migration 37: drop_user_id_loom");
        run_migration_drop_user_id_loom(conn)?;
        record_migration(conn, MIGRATION_DROP_USER_ID_LOOM, "drop_user_id_loom")?;
    }

    if current_version < MIGRATION_DROP_USER_ID_GRAPH {
        info!("Running migration 38: drop_user_id_graph");
        run_migration_drop_user_id_graph(conn)?;
        record_migration(conn, MIGRATION_DROP_USER_ID_GRAPH, "drop_user_id_graph")?;
    }

    if current_version < MIGRATION_DROP_USER_ID_THYMUS {
        info!("Running migration 39: drop_user_id_thymus");
        run_migration_drop_user_id_thymus(conn)?;
        record_migration(conn, MIGRATION_DROP_USER_ID_THYMUS, "drop_user_id_thymus")?;
    }

    if current_version < MIGRATION_DROP_USER_ID_PORTABILITY {
        info!("Running migration 40: drop_user_id_portability");
        run_migration_drop_user_id_portability(conn)?;
        record_migration(
            conn,
            MIGRATION_DROP_USER_ID_PORTABILITY,
            "drop_user_id_portability",
        )?;
    }

    if current_version < MIGRATION_DROP_USER_ID_INTELLIGENCE {
        info!("Running migration 41: drop_user_id_intelligence");
        run_migration_drop_user_id_intelligence(conn)?;
        record_migration(
            conn,
            MIGRATION_DROP_USER_ID_INTELLIGENCE,
            "drop_user_id_intelligence",
        )?;
    }

    if current_version < MIGRATION_DROP_USER_ID_SKILLS {
        info!("Running migration 42: drop_user_id_skills");
        run_migration_drop_user_id_skills(conn)?;
        record_migration(conn, MIGRATION_DROP_USER_ID_SKILLS, "drop_user_id_skills")?;
    }

    if current_version < MIGRATION_DROP_USER_ID_EPISODES {
        info!("Running migration 43: drop_user_id_episodes");
        run_migration_drop_user_id_episodes(conn)?;
        record_migration(
            conn,
            MIGRATION_DROP_USER_ID_EPISODES,
            "drop_user_id_episodes",
        )?;
    }

    if current_version < MIGRATION_READD_USER_ID_PROJECTS {
        info!("Running migration 44: readd_user_id_projects");
        run_migration_readd_user_id_projects(conn)?;
        record_migration(
            conn,
            MIGRATION_READD_USER_ID_PROJECTS,
            "readd_user_id_projects",
        )?;
    }

    if current_version < MIGRATION_READD_USER_ID_BROCA {
        info!("Running migration 45: readd_user_id_broca");
        run_migration_readd_user_id_broca(conn)?;
        record_migration(conn, MIGRATION_READD_USER_ID_BROCA, "readd_user_id_broca")?;
    }

    if current_version < MIGRATION_IDENTITY_TABLES {
        info!("Running migration 46: identity_keys_and_identities");
        run_migration_identity_tables(conn)?;
        record_migration(conn, MIGRATION_IDENTITY_TABLES, "identity_keys_and_identities")?;
    }

    if current_version < MIGRATION_AUDIT_IDENTITY_COLUMNS {
        info!("Running migration 47: audit_log_identity_columns");
        run_migration_audit_identity_columns(conn)?;
        record_migration(conn, MIGRATION_AUDIT_IDENTITY_COLUMNS, "audit_log_identity_columns")?;
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
pub async fn migration_status(db: &super::Database) -> Result<MigrationStatus> {
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

/// Migration 23: partial covering index for the /memory list hot path.
///
/// The list query always filters by `is_latest = 1 AND is_consolidated = 0`
/// and nearly always by `user_id`, then orders by `id DESC` for
/// most-recent-first pagination. Without a composite index the planner falls
/// back to `idx_memories_user` (user_id only) plus a temp-table sort, which
/// costs O(N log N) per page on high-fanout users.
///
/// The partial predicate keeps the index narrow (rows destined to be hidden
/// by `is_latest = 0` or `is_consolidated = 1` are excluded entirely), and
/// `(user_id, id DESC)` means the planner can satisfy ORDER BY via index
/// walk with a simple seek + LIMIT k.
fn run_migration_memories_list_covering_index(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_memories_list_user_id_desc \
         ON memories(user_id, id DESC) \
         WHERE is_latest = 1 AND is_consolidated = 0;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    info!("Created idx_memories_list_user_id_desc (migration 23)");
    Ok(())
}

fn down_migration_memories_list_covering_index(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch("DROP INDEX IF EXISTS idx_memories_list_user_id_desc;")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    info!("Dropped idx_memories_list_user_id_desc (migration 23 down)");
    Ok(())
}

// ---------------------------------------------------------------------------
// Migration 24: commerce tables
// ---------------------------------------------------------------------------

fn run_migration_commerce_tables(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS service_pricing (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             service_id TEXT NOT NULL UNIQUE,
             base_amount TEXT NOT NULL,
             currency TEXT NOT NULL DEFAULT 'USDC',
             chain TEXT NOT NULL DEFAULT 'base',
             chain_id INTEGER NOT NULL DEFAULT 8453,
             is_active BOOLEAN NOT NULL DEFAULT 1,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );

         CREATE TABLE IF NOT EXISTS volume_discounts (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             service_id TEXT NOT NULL,
             min_calls INTEGER NOT NULL,
             amount TEXT NOT NULL,
             FOREIGN KEY (service_id) REFERENCES service_pricing(service_id)
         );
         CREATE INDEX IF NOT EXISTS idx_vd_service ON volume_discounts(service_id);

         CREATE TABLE IF NOT EXISTS payment_quotes (
             id TEXT PRIMARY KEY,
             user_id INTEGER,
             wallet_address TEXT,
             service_id TEXT NOT NULL,
             amount TEXT NOT NULL,
             currency TEXT NOT NULL DEFAULT 'USDC',
             discount_applied TEXT,
             status TEXT NOT NULL DEFAULT 'pending',
             parameters TEXT,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             expires_at TEXT NOT NULL,
             settled_at TEXT,
             FOREIGN KEY (user_id) REFERENCES users(id)
         );
         CREATE INDEX IF NOT EXISTS idx_pq_user ON payment_quotes(user_id);
         CREATE INDEX IF NOT EXISTS idx_pq_status ON payment_quotes(status);
         CREATE INDEX IF NOT EXISTS idx_pq_expires ON payment_quotes(expires_at)
             WHERE status = 'pending';

         CREATE TABLE IF NOT EXISTS payment_settlements (
             id TEXT PRIMARY KEY,
             quote_id TEXT NOT NULL UNIQUE,
             user_id INTEGER,
             wallet_address TEXT,
             amount TEXT NOT NULL,
             currency TEXT NOT NULL DEFAULT 'USDC',
             payment_method TEXT NOT NULL,
             tx_hash TEXT,
             block_number INTEGER,
             status TEXT NOT NULL DEFAULT 'pending',
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             confirmed_at TEXT,
             FOREIGN KEY (quote_id) REFERENCES payment_quotes(id),
             FOREIGN KEY (user_id) REFERENCES users(id)
         );
         CREATE INDEX IF NOT EXISTS idx_ps_user ON payment_settlements(user_id);
         CREATE INDEX IF NOT EXISTS idx_ps_quote ON payment_settlements(quote_id);
         CREATE INDEX IF NOT EXISTS idx_ps_created ON payment_settlements(created_at DESC);

         CREATE TABLE IF NOT EXISTS account_balances (
             user_id INTEGER PRIMARY KEY,
             balance TEXT NOT NULL DEFAULT '0',
             currency TEXT NOT NULL DEFAULT 'USDC',
             updated_at TEXT NOT NULL DEFAULT (datetime('now')),
             FOREIGN KEY (user_id) REFERENCES users(id)
         );

         CREATE TABLE IF NOT EXISTS daily_spend (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             user_id INTEGER NOT NULL,
             date TEXT NOT NULL,
             total_amount TEXT NOT NULL DEFAULT '0',
             call_count INTEGER NOT NULL DEFAULT 0,
             updated_at TEXT NOT NULL DEFAULT (datetime('now')),
             UNIQUE(user_id, date),
             FOREIGN KEY (user_id) REFERENCES users(id)
         );
         CREATE INDEX IF NOT EXISTS idx_ds_user_date ON daily_spend(user_id, date);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    info!("Created commerce tables (migration 24)");
    Ok(())
}

fn down_migration_commerce_tables(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS daily_spend;
         DROP TABLE IF EXISTS account_balances;
         DROP TABLE IF EXISTS payment_settlements;
         DROP TABLE IF EXISTS payment_quotes;
         DROP TABLE IF EXISTS volume_discounts;
         DROP TABLE IF EXISTS service_pricing;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    info!("Dropped commerce tables (migration 24 down)");
    Ok(())
}

// ---------------------------------------------------------------------------
// Migration 25: drop user_id from memory core tables
// ---------------------------------------------------------------------------

/// Migration 25: drop user_id from memories, artifacts, vector_sync_pending,
/// and structured_facts on the monolith. Idempotent: each ALTER TABLE and DROP
/// INDEX is guarded by a pragma_table_info check or IF EXISTS clause.
fn run_migration_drop_user_id_memory_core(conn: &rusqlite::Connection) -> Result<()> {
    // Drop the prevent_cross_tenant_links trigger: it referenced memories.user_id
    // which is being dropped in this migration. Tenant isolation is now enforced
    // at the database level (one DB per tenant) rather than via row-level user_id.
    conn.execute_batch("DROP TRIGGER IF EXISTS prevent_cross_tenant_links;")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    // Drop indexes that key on user_id for these tables.
    // idx_memories_user: simple index on memories(user_id) from migration 2.
    // idx_memories_search: composite (user_id, is_forgotten, is_archived, is_latest).
    // idx_memories_search_composite: (user_id, is_forgotten, is_latest, category).
    // idx_memories_user_latest: (user_id, is_latest, is_forgotten).
    // idx_memories_list_user_id_desc: partial (user_id, id DESC) from migration 23.
    // idx_artifacts_user: simple index on artifacts(user_id).
    // idx_facts_user / idx_sf_subject_verb / idx_facts_user_subject_predicate:
    //   indexes on structured_facts keyed by user_id.
    conn.execute_batch(
        "DROP INDEX IF EXISTS idx_memories_user;
         DROP INDEX IF EXISTS idx_memories_search;
         DROP INDEX IF EXISTS idx_memories_search_composite;
         DROP INDEX IF EXISTS idx_memories_user_latest;
         DROP INDEX IF EXISTS idx_memories_list_user_id_desc;
         DROP INDEX IF EXISTS idx_vector_sync_user;
         DROP INDEX IF EXISTS idx_artifacts_user;
         DROP INDEX IF EXISTS idx_facts_user;
         DROP INDEX IF EXISTS idx_sf_subject_verb;
         DROP INDEX IF EXISTS idx_facts_user_subject_predicate;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    // Drop user_id from memories if still present.
    let mem_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('memories') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if mem_has_user_id > 0 {
        conn.execute("ALTER TABLE memories DROP COLUMN user_id", [])
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Dropped memories.user_id (migration 25)");
    }

    // Drop user_id from artifacts if still present.
    let art_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('artifacts') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if art_has_user_id > 0 {
        conn.execute("ALTER TABLE artifacts DROP COLUMN user_id", [])
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Dropped artifacts.user_id (migration 25)");
    }

    // Drop user_id from vector_sync_pending if still present.
    let vsp_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('vector_sync_pending') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if vsp_has_user_id > 0 {
        conn.execute("ALTER TABLE vector_sync_pending DROP COLUMN user_id", [])
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Dropped vector_sync_pending.user_id (migration 25)");
    }

    // Drop user_id from structured_facts if still present.
    let sf_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('structured_facts') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if sf_has_user_id > 0 {
        conn.execute("ALTER TABLE structured_facts DROP COLUMN user_id", [])
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Dropped structured_facts.user_id (migration 25)");
    }

    // Rebuild a non-user_id covering index for the primary memories filter pattern.
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_memories_latest_filter \
         ON memories(is_forgotten, is_archived, is_latest);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!("Migration 25 complete: user_id dropped from memory core tables");
    Ok(())
}

// ---------------------------------------------------------------------------
// Migration 26: drop user_id from scratchpad (12-step UNIQUE rebuild)
// ---------------------------------------------------------------------------

/// Migration 26: drop user_id from scratchpad via the 12-step rebuild path.
/// scratchpad carried UNIQUE(user_id, session, entry_key), which blocks
/// ALTER TABLE DROP COLUMN, so this rebuilds the table with the new
/// UNIQUE(session, agent, entry_key) constraint. Idempotent: if scratchpad
/// already lacks user_id the migration is a no-op.
fn run_migration_drop_user_id_scratchpad(conn: &rusqlite::Connection) -> Result<()> {
    let has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('scratchpad') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if has_user_id == 0 {
        info!("scratchpad.user_id already absent, migration 26 is a no-op");
        return Ok(());
    }

    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;

         ALTER TABLE scratchpad RENAME TO _scratchpad_old_v25;

         DROP INDEX IF EXISTS idx_scratchpad_agent;
         DROP INDEX IF EXISTS idx_scratchpad_expires;
         DROP INDEX IF EXISTS idx_scratchpad_user_expires;
         DROP INDEX IF EXISTS idx_scratchpad_session;

         CREATE TABLE scratchpad (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             agent TEXT NOT NULL DEFAULT 'unknown',
             session TEXT NOT NULL DEFAULT 'default',
             model TEXT NOT NULL DEFAULT '',
             entry_key TEXT NOT NULL,
             value TEXT NOT NULL DEFAULT '',
             expires_at TEXT,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now')),
             UNIQUE(session, agent, entry_key)
         );

         INSERT INTO scratchpad (id, agent, session, model, entry_key, value, expires_at, created_at, updated_at)
         SELECT id, agent, session, model, entry_key, value, expires_at, created_at, updated_at
         FROM _scratchpad_old_v25;

         DROP TABLE _scratchpad_old_v25;

         CREATE INDEX idx_scratchpad_agent ON scratchpad(agent);
         CREATE INDEX idx_scratchpad_session ON scratchpad(session);
         CREATE INDEX idx_scratchpad_expires ON scratchpad(expires_at) WHERE expires_at IS NOT NULL;

         PRAGMA foreign_keys = ON;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!("Migration 26 complete: user_id dropped from scratchpad");
    Ok(())
}

// ---------------------------------------------------------------------------
// Migration 27: drop user_id from sessions (simple DROP INDEX + DROP COLUMN)
// ---------------------------------------------------------------------------

/// Migration 27: drop user_id shim from sessions. No UNIQUE or FK references
/// the column, so ALTER TABLE DROP COLUMN is safe. session_output never had
/// user_id, so it is not touched. Idempotent: no-op if user_id already absent.
fn run_migration_drop_user_id_sessions(conn: &rusqlite::Connection) -> Result<()> {
    let has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if has_user_id == 0 {
        info!("sessions.user_id already absent, migration 27 is a no-op");
        return Ok(());
    }

    conn.execute_batch("DROP INDEX IF EXISTS idx_sessions_user;")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    conn.execute("ALTER TABLE sessions DROP COLUMN user_id", [])
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!("Migration 27 complete: user_id dropped from sessions");
    Ok(())
}

// ---------------------------------------------------------------------------
// Migration 28: drop user_id from chiasm_tasks + chiasm_task_updates
// ---------------------------------------------------------------------------

/// Migration 28: drop user_id shim from chiasm_tasks and chiasm_task_updates.
/// No UNIQUE or FK references the column on either table, so ALTER TABLE
/// DROP COLUMN is safe. chiasm_task_updates has no user_id index, so only
/// the column drop runs there. Idempotent: skips each table that already
/// lacks user_id.
fn run_migration_drop_user_id_chiasm(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch("DROP INDEX IF EXISTS idx_chiasm_tasks_user;")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    for table in &["chiasm_tasks", "chiasm_task_updates"] {
        let has_user_id: i64 = conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM pragma_table_info('{}') WHERE name = 'user_id'",
                    table
                ),
                [],
                |row| row.get(0),
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        if has_user_id == 0 {
            info!("{}.user_id already absent, skipping", table);
            continue;
        }
        conn.execute(&format!("ALTER TABLE {} DROP COLUMN user_id", table), [])
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Dropped {}.user_id (migration 28)", table);
    }

    info!("Migration 28 complete: user_id dropped from chiasm tables");
    Ok(())
}

// ---------------------------------------------------------------------------
// Migration 29: drop user_id from approvals (simple DROP INDEX + DROP COLUMN)
// ---------------------------------------------------------------------------

/// Migration 29: drop user_id shim from approvals. Both the simple
/// idx_approvals_user and the composite idx_approvals_user_status
/// indexes are dropped before the column goes. No UNIQUE or FK references
/// the column. Idempotent: skips if user_id already absent.
fn run_migration_drop_user_id_approvals(conn: &rusqlite::Connection) -> Result<()> {
    let has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('approvals') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if has_user_id == 0 {
        info!("approvals.user_id already absent, migration 29 is a no-op");
        return Ok(());
    }

    conn.execute_batch(
        "DROP INDEX IF EXISTS idx_approvals_user;
         DROP INDEX IF EXISTS idx_approvals_user_status;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    conn.execute("ALTER TABLE approvals DROP COLUMN user_id", [])
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!("Migration 29 complete: user_id dropped from approvals");
    Ok(())
}

// ---------------------------------------------------------------------------
// Migration 30: drop user_id from broca_actions (simple DROP INDEX + DROP COLUMN)
// ---------------------------------------------------------------------------

/// Migration 30: drop user_id shim from broca_actions. No UNIQUE or FK
/// references the column. Idempotent: skips if user_id already absent.
fn run_migration_drop_user_id_broca(conn: &rusqlite::Connection) -> Result<()> {
    let has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('broca_actions') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if has_user_id == 0 {
        info!("broca_actions.user_id already absent, migration 30 is a no-op");
        return Ok(());
    }

    conn.execute_batch("DROP INDEX IF EXISTS idx_broca_actions_user;")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    conn.execute("ALTER TABLE broca_actions DROP COLUMN user_id", [])
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!("Migration 30 complete: user_id dropped from broca_actions");
    Ok(())
}

// ---------------------------------------------------------------------------
// Migration 31: drop user_id from projects (12-step UNIQUE rebuild)
// ---------------------------------------------------------------------------

/// Migration 31: drop user_id from projects via the 12-step rebuild path.
/// projects carried UNIQUE(name, user_id), which blocks ALTER TABLE DROP
/// COLUMN, so this rebuilds the table with UNIQUE(name). memory_projects
/// references projects(id); legacy_alter_table=1 keeps that FK referring
/// to "projects" by name through the rename so it resolves to the new
/// table. Idempotent: if projects already lacks user_id the migration is
/// a no-op.
fn run_migration_drop_user_id_projects(conn: &rusqlite::Connection) -> Result<()> {
    let has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('projects') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if has_user_id == 0 {
        info!("projects.user_id already absent, migration 31 is a no-op");
        return Ok(());
    }

    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         PRAGMA legacy_alter_table = 1;

         ALTER TABLE projects RENAME TO _projects_old_v30;

         DROP INDEX IF EXISTS idx_projects_user;
         DROP INDEX IF EXISTS idx_projects_status;

         CREATE TABLE projects (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             name TEXT NOT NULL,
             description TEXT,
             status TEXT NOT NULL DEFAULT 'active',
             metadata TEXT,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now')),
             UNIQUE(name)
         );

         INSERT INTO projects (id, name, description, status, metadata, created_at, updated_at)
         SELECT id, name, description, status, metadata, created_at, updated_at
         FROM _projects_old_v30;

         DROP TABLE _projects_old_v30;

         CREATE INDEX idx_projects_status ON projects(status);

         PRAGMA legacy_alter_table = 0;
         PRAGMA foreign_keys = ON;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!("Migration 31 complete: user_id dropped from projects");
    Ok(())
}

// ---------------------------------------------------------------------------
// Migration 32: drop user_id from axon_events + soma_agents
// ---------------------------------------------------------------------------

/// Migration 32: drop user_id shim from axon_events and soma_agents. No UNIQUE
/// or FK references the column on either table. Idempotent: each table is
/// checked independently before its DROP INDEX + DROP COLUMN pair.
fn run_migration_drop_user_id_activity(conn: &rusqlite::Connection) -> Result<()> {
    let axon_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('axon_events') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if axon_has_user_id > 0 {
        conn.execute_batch("DROP INDEX IF EXISTS idx_axon_events_user;")
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        conn.execute("ALTER TABLE axon_events DROP COLUMN user_id", [])
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Migration 32: user_id dropped from axon_events");
    } else {
        info!("axon_events.user_id already absent, skipping");
    }

    let soma_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('soma_agents') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if soma_has_user_id > 0 {
        conn.execute_batch("DROP INDEX IF EXISTS idx_soma_agents_user;")
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        conn.execute("ALTER TABLE soma_agents DROP COLUMN user_id", [])
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Migration 32: user_id dropped from soma_agents");
    } else {
        info!("soma_agents.user_id already absent, skipping");
    }

    info!("Migration 32 complete: user_id dropped from axon_events + soma_agents");
    Ok(())
}

// ---------------------------------------------------------------------------
// Migration 33: drop user_id from webhooks (DROP INDEX + DROP COLUMN)
// ---------------------------------------------------------------------------

/// Migration 33: drop user_id shim from webhooks. No UNIQUE references the
/// column (the tenant shard dropped the FK in v9). Idempotent: skips if
/// user_id already absent.
fn run_migration_drop_user_id_webhooks(conn: &rusqlite::Connection) -> Result<()> {
    let has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('webhooks') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if has_user_id == 0 {
        info!("webhooks.user_id already absent, migration 33 is a no-op");
        return Ok(());
    }

    conn.execute_batch("DROP INDEX IF EXISTS idx_webhooks_user;")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    conn.execute("ALTER TABLE webhooks DROP COLUMN user_id", [])
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!("Migration 33 complete: user_id dropped from webhooks");
    Ok(())
}

// ---------------------------------------------------------------------------
// Migration 34: drop user_id from axon_subscriptions + axon_cursors
// ---------------------------------------------------------------------------

/// Migration 34: drop user_id shim from axon_subscriptions and axon_cursors.
/// UNIQUE(agent, channel) on axon_subscriptions and PRIMARY KEY(agent, channel)
/// on axon_cursors do NOT include user_id -- plain DROP COLUMN works for both.
/// No idx_*_user indexes exist on either table. Idempotent: each table
/// checked independently.
fn run_migration_drop_user_id_axon(conn: &rusqlite::Connection) -> Result<()> {
    let subs_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('axon_subscriptions') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if subs_has_user_id > 0 {
        conn.execute("ALTER TABLE axon_subscriptions DROP COLUMN user_id", [])
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Migration 34: user_id dropped from axon_subscriptions");
    } else {
        info!("axon_subscriptions.user_id already absent, skipping");
    }

    let cursors_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('axon_cursors') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if cursors_has_user_id > 0 {
        conn.execute("ALTER TABLE axon_cursors DROP COLUMN user_id", [])
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Migration 34: user_id dropped from axon_cursors");
    } else {
        info!("axon_cursors.user_id already absent, skipping");
    }

    info!("Migration 34 complete: user_id dropped from axon_subscriptions + axon_cursors");
    Ok(())
}

// ---------------------------------------------------------------------------
// Migration 35: drop user_id from reflections (DROP INDEX + DROP COLUMN)
// ---------------------------------------------------------------------------

/// Migration 35: drop user_id shim from reflections. No UNIQUE or FK
/// references the column. idx_reflections_user must drop first;
/// idx_reflections_type and idx_reflections_period stay. Idempotent.
fn run_migration_drop_user_id_growth(conn: &rusqlite::Connection) -> Result<()> {
    let has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('reflections') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if has_user_id == 0 {
        info!("reflections.user_id already absent, migration 35 is a no-op");
        return Ok(());
    }

    conn.execute_batch("DROP INDEX IF EXISTS idx_reflections_user;")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    conn.execute("ALTER TABLE reflections DROP COLUMN user_id", [])
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!("Migration 35 complete: user_id dropped from reflections");
    Ok(())
}

// ---------------------------------------------------------------------------
// Migration 36: drop user_id from ingestion_hashes (PK rebuild)
// ---------------------------------------------------------------------------

/// Migration 36: drop user_id from ingestion_hashes via the 12-step rebuild
/// path. ingestion_hashes carried PRIMARY KEY (sha256, user_id), which blocks
/// ALTER TABLE DROP COLUMN on the PK column, so we rebuild with PRIMARY KEY
/// (sha256). INSERT OR IGNORE in the row copy handles the edge case where
/// multiple rows shared the same sha256 under the old composite PK.
/// Idempotent: if ingestion_hashes already lacks user_id the migration is
/// a no-op.
fn run_migration_drop_user_id_ingestion_hashes(conn: &rusqlite::Connection) -> Result<()> {
    let has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('ingestion_hashes') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if has_user_id == 0 {
        info!("ingestion_hashes.user_id already absent, migration 36 is a no-op");
        return Ok(());
    }

    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         PRAGMA legacy_alter_table = 1;

         ALTER TABLE ingestion_hashes RENAME TO _ingestion_hashes_old_v35;

         CREATE TABLE ingestion_hashes (
             sha256 TEXT NOT NULL,
             first_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
             job_id TEXT,
             PRIMARY KEY (sha256)
         );

         INSERT OR IGNORE INTO ingestion_hashes (sha256, first_seen_at, job_id)
         SELECT sha256, first_seen_at, job_id
         FROM _ingestion_hashes_old_v35;

         DROP TABLE _ingestion_hashes_old_v35;

         PRAGMA legacy_alter_table = 0;
         PRAGMA foreign_keys = ON;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!("Migration 36 complete: user_id dropped from ingestion_hashes");
    Ok(())
}

fn run_migration_drop_user_id_loom(conn: &rusqlite::Connection) -> Result<()> {
    // Idempotent guard: check loom_workflows first (Shape B rebuild).
    let wf_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('loom_workflows') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    let runs_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('loom_runs') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    if wf_has_user_id == 0 && runs_has_user_id == 0 {
        info!("loom_workflows and loom_runs user_id already absent, migration 37 is a no-op");
        return Ok(());
    }

    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         PRAGMA legacy_alter_table = 1;

         ALTER TABLE loom_workflows RENAME TO _loom_workflows_old_v36;

         DROP INDEX IF EXISTS idx_loom_workflows_user;

         CREATE TABLE loom_workflows (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             name TEXT NOT NULL,
             description TEXT,
             steps TEXT NOT NULL DEFAULT '[]',
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now')),
             UNIQUE(name)
         );

         INSERT INTO loom_workflows (id, name, description, steps, created_at, updated_at)
         SELECT id, name, description, steps, created_at, updated_at
         FROM _loom_workflows_old_v36;

         DROP TABLE _loom_workflows_old_v36;

         DROP INDEX IF EXISTS idx_loom_runs_user;
         ALTER TABLE loom_runs DROP COLUMN user_id;

         PRAGMA legacy_alter_table = 0;
         PRAGMA foreign_keys = ON;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!("Migration 37 complete: user_id dropped from loom_workflows and loom_runs");
    Ok(())
}

fn run_migration_drop_user_id_graph(conn: &rusqlite::Connection) -> Result<()> {
    // Idempotent guard: check entities (Shape B), memory_pagerank (Shape B),
    // pagerank_dirty (CHECK rebuild), entity_cooccurrences (Shape A),
    // brain_edges (Shape A).
    //
    // NOTE: structured_facts.user_id was already dropped by migration 25
    // (run_migration_drop_user_id_memory_core) so we do NOT attempt to drop
    // it here. Migration 38 only handles the remaining 5 tables.
    let entities_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('entities') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    let ec_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('entity_cooccurrences') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    let mp_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('memory_pagerank') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    let pd_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('pagerank_dirty') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    let be_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('brain_edges') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    if entities_has_user_id == 0
        && ec_has_user_id == 0
        && mp_has_user_id == 0
        && pd_has_user_id == 0
        && be_has_user_id == 0
    {
        info!("graph cluster user_id already absent, migration 38 is a no-op");
        return Ok(());
    }

    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         PRAGMA legacy_alter_table = 1;

         -- entities: Shape B rebuild
         ALTER TABLE entities RENAME TO _entities_old_v37;
         DROP INDEX IF EXISTS idx_entities_user;
         DROP INDEX IF EXISTS idx_entities_name;
         DROP INDEX IF EXISTS idx_entities_type;

         CREATE TABLE entities (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             name TEXT NOT NULL,
             entity_type TEXT NOT NULL DEFAULT 'concept',
             type TEXT NOT NULL DEFAULT 'generic',
             description TEXT,
             aliases TEXT,
             aka TEXT,
             metadata TEXT,
             space_id INTEGER,
             confidence REAL NOT NULL DEFAULT 1.0,
             occurrence_count INTEGER NOT NULL DEFAULT 1,
             first_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
             last_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now')),
             UNIQUE(name, entity_type)
         );

         INSERT OR IGNORE INTO entities
             (id, name, entity_type, type, description, aliases, aka, metadata,
              space_id, confidence, occurrence_count,
              first_seen_at, last_seen_at, created_at, updated_at)
         SELECT
             id, name, entity_type, type, description, aliases, aka, metadata,
             space_id, confidence, occurrence_count,
             first_seen_at, last_seen_at, created_at, updated_at
         FROM _entities_old_v37;

         DROP TABLE _entities_old_v37;

         CREATE INDEX IF NOT EXISTS idx_entities_name ON entities(name);
         CREATE INDEX IF NOT EXISTS idx_entities_type ON entities(entity_type);

         -- entity_cooccurrences: Shape A
         DROP INDEX IF EXISTS idx_ec_user;
         ALTER TABLE entity_cooccurrences DROP COLUMN user_id;

         -- memory_pagerank: Shape B rebuild
         ALTER TABLE memory_pagerank RENAME TO _memory_pagerank_old_v37;
         DROP INDEX IF EXISTS idx_pagerank_user;

         CREATE TABLE memory_pagerank (
             memory_id INTEGER PRIMARY KEY,
             score REAL NOT NULL,
             computed_at INTEGER NOT NULL,
             FOREIGN KEY (memory_id) REFERENCES memories(id) ON DELETE CASCADE
         );

         INSERT OR IGNORE INTO memory_pagerank (memory_id, score, computed_at)
         SELECT memory_id, score, computed_at
         FROM _memory_pagerank_old_v37;

         DROP TABLE _memory_pagerank_old_v37;

         CREATE INDEX IF NOT EXISTS idx_pagerank_score ON memory_pagerank(score DESC);

         -- pagerank_dirty: CHECK constraint rebuild
         ALTER TABLE pagerank_dirty RENAME TO _pagerank_dirty_old_v37;

         CREATE TABLE pagerank_dirty (
             id INTEGER PRIMARY KEY CHECK (id = 1),
             dirty_count INTEGER NOT NULL DEFAULT 0,
             last_refresh INTEGER NOT NULL DEFAULT 0
         );

         INSERT OR IGNORE INTO pagerank_dirty (id, dirty_count, last_refresh)
         SELECT 1, COALESCE(dirty_count, 0), COALESCE(last_refresh, 0)
         FROM _pagerank_dirty_old_v37
         LIMIT 1;

         INSERT OR IGNORE INTO pagerank_dirty (id, dirty_count, last_refresh)
         VALUES (1, 0, 0);

         DROP TABLE _pagerank_dirty_old_v37;

         -- brain_edges: Shape A
         DROP INDEX IF EXISTS idx_brain_edges_user;
         ALTER TABLE brain_edges DROP COLUMN user_id;

         PRAGMA legacy_alter_table = 0;
         PRAGMA foreign_keys = ON;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!("Migration 38 complete: user_id dropped from graph cluster (5 tables; structured_facts.user_id was dropped in migration 25)");
    Ok(())
}

fn run_migration_drop_user_id_thymus(conn: &rusqlite::Connection) -> Result<()> {
    // Idempotent guard: check rubrics (Shape A with index swap) as sentinel.
    // If rubrics.user_id is already gone, all 5 thymus tables have been
    // processed by a prior run.
    let rubrics_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('rubrics') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    let evals_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('evaluations') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    let qm_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('quality_metrics') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    let sq_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('session_quality') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    let bde_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('behavioral_drift_events') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    if rubrics_has_user_id == 0
        && evals_has_user_id == 0
        && qm_has_user_id == 0
        && sq_has_user_id == 0
        && bde_has_user_id == 0
    {
        info!("thymus user_id already absent, migration 39 is a no-op");
        return Ok(());
    }

    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         PRAGMA legacy_alter_table = 1;

         -- rubrics: Shape A with index swap
         DROP INDEX IF EXISTS idx_rubrics_user_name;
         DROP INDEX IF EXISTS idx_rubrics_user;
         ALTER TABLE rubrics DROP COLUMN user_id;
         CREATE UNIQUE INDEX IF NOT EXISTS idx_rubrics_name ON rubrics(name);

         -- evaluations: Shape A
         DROP INDEX IF EXISTS idx_evaluations_user;
         ALTER TABLE evaluations DROP COLUMN user_id;

         -- quality_metrics: Shape A
         DROP INDEX IF EXISTS idx_quality_metrics_user;
         ALTER TABLE quality_metrics DROP COLUMN user_id;

         -- session_quality: Shape A
         DROP INDEX IF EXISTS idx_session_quality_user;
         ALTER TABLE session_quality DROP COLUMN user_id;

         -- behavioral_drift_events: Shape A
         DROP INDEX IF EXISTS idx_behavioral_drift_user;
         ALTER TABLE behavioral_drift_events DROP COLUMN user_id;

         PRAGMA legacy_alter_table = 0;
         PRAGMA foreign_keys = ON;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!("Migration 39 complete: user_id dropped from thymus cluster (5 tables)");
    Ok(())
}

fn run_migration_drop_user_id_portability(conn: &rusqlite::Connection) -> Result<()> {
    // Idempotent guard: check both tables. user_preferences is Shape B (table
    // rebuild required due to in-table UNIQUE constraint); conversations is
    // Shape A (simple DROP COLUMN).
    let up_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('user_preferences') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    let conv_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('conversations') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    if up_has_user_id == 0 && conv_has_user_id == 0 {
        info!("user_preferences and conversations user_id already absent, migration 40 is a no-op");
        return Ok(());
    }

    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         PRAGMA legacy_alter_table = 1;

         -- user_preferences: Shape B rebuild (in-table UNIQUE(user_id, key) prevents
         -- simple DROP COLUMN; full 12-step table rebuild required).
         ALTER TABLE user_preferences RENAME TO _user_preferences_old_v39;

         DROP INDEX IF EXISTS idx_up_domain_pref_user;
         DROP INDEX IF EXISTS idx_user_prefs_user;
         DROP INDEX IF EXISTS idx_up_domain;

         CREATE TABLE user_preferences (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             key TEXT NOT NULL,
             value TEXT NOT NULL,
             domain TEXT,
             preference TEXT,
             strength REAL NOT NULL DEFAULT 1.0,
             evidence_memory_id INTEGER REFERENCES memories(id) ON DELETE SET NULL,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now')),
             UNIQUE(key)
         );

         INSERT INTO user_preferences (id, key, value, domain, preference, strength, evidence_memory_id, created_at, updated_at)
         SELECT id, key, value, domain, preference, strength, evidence_memory_id, created_at, updated_at
         FROM _user_preferences_old_v39;

         DROP TABLE _user_preferences_old_v39;

         CREATE INDEX IF NOT EXISTS idx_up_domain ON user_preferences(domain COLLATE NOCASE);
         CREATE UNIQUE INDEX IF NOT EXISTS idx_up_domain_pref ON user_preferences(domain, preference);

         -- conversations: Shape A (simple DROP COLUMN)
         DROP INDEX IF EXISTS idx_conversations_user;
         ALTER TABLE conversations DROP COLUMN user_id;

         PRAGMA legacy_alter_table = 0;
         PRAGMA foreign_keys = ON;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!(
        "Migration 40 complete: user_id dropped from user_preferences (rebuild) and conversations"
    );
    Ok(())
}

fn run_migration_drop_user_id_intelligence(conn: &rusqlite::Connection) -> Result<()> {
    // Idempotent guard: check current_state (Shape B) and consolidations (Shape A).
    // If both already lack user_id, the migration already ran.
    let cs_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('current_state') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    let consolidations_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('consolidations') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    if cs_has_user_id == 0 && consolidations_has_user_id == 0 {
        info!("intelligence tables user_id already absent, migration 41 is a no-op");
        return Ok(());
    }

    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         PRAGMA legacy_alter_table = 1;

         -- current_state: Shape B rebuild (in-table UNIQUE(agent, key, user_id) prevents
         -- simple DROP COLUMN; full 12-step table rebuild required).
         ALTER TABLE current_state RENAME TO _current_state_old_v40;

         DROP INDEX IF EXISTS idx_current_state_user;
         DROP INDEX IF EXISTS idx_cs_key_user;
         DROP INDEX IF EXISTS idx_current_state_agent;
         DROP INDEX IF EXISTS idx_cs_key;

         CREATE TABLE current_state (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             agent TEXT NOT NULL,
             key TEXT NOT NULL,
             value TEXT NOT NULL,
             memory_id INTEGER REFERENCES memories(id) ON DELETE SET NULL,
             previous_value TEXT,
             previous_memory_id INTEGER,
             updated_count INTEGER NOT NULL DEFAULT 1,
             updated_at TEXT NOT NULL DEFAULT (datetime('now')),
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             UNIQUE(agent, key)
         );

         INSERT OR IGNORE INTO current_state
             (id, agent, key, value, memory_id, previous_value, previous_memory_id,
              updated_count, updated_at, created_at)
         SELECT id, agent, key, value, memory_id, previous_value, previous_memory_id,
                updated_count, updated_at, created_at
         FROM _current_state_old_v40
         ORDER BY id DESC;

         DROP TABLE _current_state_old_v40;

         CREATE INDEX IF NOT EXISTS idx_current_state_agent ON current_state(agent);
         CREATE INDEX IF NOT EXISTS idx_cs_key ON current_state(key COLLATE NOCASE);

         -- consolidations: Shape A
         DROP INDEX IF EXISTS idx_consolidations_user;
         ALTER TABLE consolidations DROP COLUMN user_id;

         -- causal_chains: Shape A
         DROP INDEX IF EXISTS idx_causal_chains_user;
         ALTER TABLE causal_chains DROP COLUMN user_id;

         -- reconsolidations: Shape A (no user-scoped index)
         ALTER TABLE reconsolidations DROP COLUMN user_id;

         -- temporal_patterns: Shape A
         DROP INDEX IF EXISTS idx_temporal_patterns_user;
         ALTER TABLE temporal_patterns DROP COLUMN user_id;

         -- digests: Shape A (preserve idx_digests_period and idx_digests_next)
         DROP INDEX IF EXISTS idx_digests_user;
         ALTER TABLE digests DROP COLUMN user_id;

         -- memory_feedback: Shape A (preserve idx_feedback_memory)
         DROP INDEX IF EXISTS idx_feedback_user;
         ALTER TABLE memory_feedback DROP COLUMN user_id;

         PRAGMA legacy_alter_table = 0;
         PRAGMA foreign_keys = ON;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!("Migration 41 complete: user_id dropped from 7 intelligence tables (current_state rebuild + 6 DROP COLUMN)");
    Ok(())
}

fn run_migration_drop_user_id_skills(conn: &rusqlite::Connection) -> Result<()> {
    // Idempotent guard: check skill_records. If user_id is already absent,
    // the migration already ran.
    let has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('skill_records') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    if has_user_id == 0 {
        info!("skill_records user_id already absent, migration 42 is a no-op");
        return Ok(());
    }

    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         PRAGMA legacy_alter_table = 1;

         -- Drop FTS triggers before renaming the content table.
         DROP TRIGGER IF EXISTS skills_fts_insert;
         DROP TRIGGER IF EXISTS skills_fts_delete;
         DROP TRIGGER IF EXISTS skills_fts_update;

         -- Rename the old table out of the way.
         ALTER TABLE skill_records RENAME TO _skill_records_old_v41;

         -- Drop all indexes so they can be recreated against the new table.
         DROP INDEX IF EXISTS idx_skill_records_user;
         DROP INDEX IF EXISTS idx_skill_records_agent;
         DROP INDEX IF EXISTS idx_skill_records_name;
         DROP INDEX IF EXISTS idx_skill_records_active;
         DROP INDEX IF EXISTS idx_skill_records_category;
         DROP INDEX IF EXISTS idx_skill_records_parent;

         -- Create the new table without user_id.
         -- New in-table constraint: UNIQUE(name, agent, version).
         CREATE TABLE skill_records (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             skill_id TEXT UNIQUE,
             name TEXT NOT NULL,
             agent TEXT NOT NULL,
             description TEXT,
             code TEXT NOT NULL,
             path TEXT,
             content TEXT NOT NULL DEFAULT '',
             category TEXT NOT NULL DEFAULT 'workflow',
             origin TEXT NOT NULL DEFAULT 'imported',
             generation INTEGER NOT NULL DEFAULT 0,
             lineage_change_summary TEXT,
             creator_id TEXT,
             language TEXT NOT NULL DEFAULT 'javascript',
             version INTEGER NOT NULL DEFAULT 1,
             parent_skill_id INTEGER REFERENCES skill_records(id),
             root_skill_id INTEGER REFERENCES skill_records(id),
             embedding BLOB,
             embedding_vec_1024 FLOAT32(1024),
             trust_score REAL NOT NULL DEFAULT 50,
             success_count INTEGER NOT NULL DEFAULT 0,
             failure_count INTEGER NOT NULL DEFAULT 0,
             execution_count INTEGER NOT NULL DEFAULT 0,
             avg_duration_ms REAL,
             is_active BOOLEAN NOT NULL DEFAULT 1,
             is_deprecated BOOLEAN NOT NULL DEFAULT 0,
             total_selections INTEGER NOT NULL DEFAULT 0,
             total_applied INTEGER NOT NULL DEFAULT 0,
             total_completions INTEGER NOT NULL DEFAULT 0,
             visibility TEXT NOT NULL DEFAULT 'private',
             lineage_source_task_id TEXT,
             lineage_content_diff TEXT NOT NULL DEFAULT '',
             lineage_content_snapshot TEXT NOT NULL DEFAULT '{}',
             total_fallbacks INTEGER NOT NULL DEFAULT 0,
             metadata TEXT,
             first_seen TEXT NOT NULL DEFAULT (datetime('now')),
             last_updated TEXT NOT NULL DEFAULT (datetime('now')),
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now')),
             UNIQUE(name, agent, version)
         );

         -- Copy rows forward preserving id values so FK references stay valid.
         -- On conflict (rows that differed only by user_id now share the same
         -- (name, agent, version) triple), keep the row with the lower id.
         INSERT OR IGNORE INTO skill_records (
             id, skill_id, name, agent, description, code, path, content, category, origin,
             generation, lineage_change_summary, creator_id, language, version,
             parent_skill_id, root_skill_id, embedding, embedding_vec_1024,
             trust_score, success_count, failure_count, execution_count, avg_duration_ms,
             is_active, is_deprecated, total_selections, total_applied, total_completions,
             visibility, lineage_source_task_id, lineage_content_diff, lineage_content_snapshot,
             total_fallbacks, metadata, first_seen, last_updated, created_at, updated_at
         )
         SELECT
             id, skill_id, name, agent, description, code, path, content, category, origin,
             generation, lineage_change_summary, creator_id, language, version,
             parent_skill_id, root_skill_id, embedding, embedding_vec_1024,
             trust_score, success_count, failure_count, execution_count, avg_duration_ms,
             is_active, is_deprecated, total_selections, total_applied, total_completions,
             visibility, lineage_source_task_id, lineage_content_diff, lineage_content_snapshot,
             total_fallbacks, metadata, first_seen, last_updated, created_at, updated_at
         FROM _skill_records_old_v41
         ORDER BY id ASC;

         DROP TABLE _skill_records_old_v41;

         -- Recreate the 5 preserved indexes.
         CREATE INDEX IF NOT EXISTS idx_skill_records_agent ON skill_records(agent);
         CREATE INDEX IF NOT EXISTS idx_skill_records_name ON skill_records(name);
         CREATE INDEX IF NOT EXISTS idx_skill_records_active ON skill_records(is_active);
         CREATE INDEX IF NOT EXISTS idx_skill_records_category ON skill_records(category);
         CREATE INDEX IF NOT EXISTS idx_skill_records_parent ON skill_records(parent_skill_id);

         -- Recreate the 3 FTS triggers verbatim (their bodies never referenced user_id).
         CREATE TRIGGER IF NOT EXISTS skills_fts_insert AFTER INSERT ON skill_records BEGIN
             INSERT INTO skills_fts(rowid, name, description, code)
             VALUES (new.id, new.name, new.description, new.code);
         END;

         CREATE TRIGGER IF NOT EXISTS skills_fts_delete AFTER DELETE ON skill_records BEGIN
             INSERT INTO skills_fts(skills_fts, rowid, name, description, code)
             VALUES ('delete', old.id, old.name, old.description, old.code);
         END;

         CREATE TRIGGER IF NOT EXISTS skills_fts_update AFTER UPDATE ON skill_records BEGIN
             INSERT INTO skills_fts(skills_fts, rowid, name, description, code)
             VALUES ('delete', old.id, old.name, old.description, old.code);
             INSERT INTO skills_fts(rowid, name, description, code)
             VALUES (new.id, new.name, new.description, new.code);
         END;

         -- Rebuild the FTS shadow from the new skill_records content.
         INSERT INTO skills_fts(skills_fts) VALUES('rebuild');

         PRAGMA legacy_alter_table = 0;
         PRAGMA foreign_keys = ON;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!(
        "Migration 42 complete: user_id dropped from skill_records (Shape B + FTS shadow rebuild)"
    );
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

    // session_quality.user_id and behavioral_drift_events.user_id were dropped
    // in migration 39. The queries below would fail with "no such column: user_id"
    // on any DB at schema version 39+. Hardcode 0 so the struct fields and the
    // is_clean check remain intact for kleos-migrate API compat.
    let session_quality_zero_user = 0_i64;
    let behavioral_drift_zero_user = 0_i64;

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

fn run_migration_drop_user_id_episodes(conn: &rusqlite::Connection) -> Result<()> {
    // Idempotent guard: if user_id is already absent from episodes, skip.
    let has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('episodes') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    if has_user_id == 0 {
        info!("episodes user_id already absent, migration 43 is a no-op");
        return Ok(());
    }

    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         PRAGMA legacy_alter_table = 1;

         DROP INDEX IF EXISTS idx_episodes_user;

         ALTER TABLE episodes DROP COLUMN user_id;

         PRAGMA legacy_alter_table = 0;
         PRAGMA foreign_keys = ON;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!(
        "Migration 43 complete: user_id dropped from episodes (Shape A, FTS triggers unaffected)"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Migration 44: re-add user_id to projects (C-R3-004)
// ---------------------------------------------------------------------------

/// Migration 44: re-add user_id to monolith projects so single-DB deployments
/// are safe even when tenant sharding is disabled. Phase 5 dropped user_id
/// from projects (v31) on the assumption that every deployment ran with
/// `ENGRAM_TENANT_SHARDING=1`. The R-3 audit (C-R3-004) showed sharding was
/// opt-in, leaving multi-user single-DB deployments cross-tenant exposed.
///
/// Tenant shards (one DB per user) intentionally do NOT carry user_id; only
/// the monolith does. This migration is therefore monolith-only and reuses
/// the 12-step rebuild path because of the existing UNIQUE(name) on projects.
/// The new column is `NOT NULL DEFAULT 1` so legacy rows backfill to the
/// system user, which matches the pre-Phase-5 ownership.
fn run_migration_readd_user_id_projects(conn: &rusqlite::Connection) -> Result<()> {
    let has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('projects') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if has_user_id > 0 {
        info!("projects.user_id already present, migration 44 is a no-op");
        return Ok(());
    }

    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         PRAGMA legacy_alter_table = 1;

         ALTER TABLE projects RENAME TO _projects_old_v44;

         DROP INDEX IF EXISTS idx_projects_status;
         DROP INDEX IF EXISTS idx_projects_user;

         CREATE TABLE projects (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             name TEXT NOT NULL,
             description TEXT,
             status TEXT NOT NULL DEFAULT 'active',
             metadata TEXT,
             user_id INTEGER NOT NULL DEFAULT 1,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now')),
             UNIQUE(name, user_id)
         );

         INSERT INTO projects (id, name, description, status, metadata, user_id, created_at, updated_at)
         SELECT id, name, description, status, metadata, 1, created_at, updated_at
         FROM _projects_old_v44;

         DROP TABLE _projects_old_v44;

         CREATE INDEX idx_projects_status ON projects(status);
         CREATE INDEX idx_projects_user ON projects(user_id);

         PRAGMA legacy_alter_table = 0;
         PRAGMA foreign_keys = ON;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!("Migration 44 complete: user_id re-added to projects (defaults to 1 for legacy rows)");
    Ok(())
}

// ---------------------------------------------------------------------------
// Migration 45: re-add user_id to broca_actions (C-R3-004 / H-R3-006)
// ---------------------------------------------------------------------------

/// Migration 45: re-add user_id to monolith broca_actions. broca_actions has
/// no UNIQUE/FK on the column, so the simpler ALTER TABLE ADD COLUMN path
/// is sufficient. The column is non-nullable with DEFAULT 1 so legacy rows
/// backfill to the system user. An idx_broca_actions_user index is added so
/// per-user queries do not full-scan.
fn run_migration_readd_user_id_broca(conn: &rusqlite::Connection) -> Result<()> {
    let has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('broca_actions') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if has_user_id > 0 {
        info!("broca_actions.user_id already present, migration 45 is a no-op");
        return Ok(());
    }

    conn.execute_batch(
        "ALTER TABLE broca_actions ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1;
         CREATE INDEX IF NOT EXISTS idx_broca_actions_user ON broca_actions(user_id, created_at DESC);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!("Migration 45 complete: user_id re-added to broca_actions");
    Ok(())
}

// ---------------------------------------------------------------------------
// Migration 46: identity_keys + identities tables
// ---------------------------------------------------------------------------

fn run_migration_identity_tables(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS identity_keys (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            tier TEXT NOT NULL CHECK (tier IN ('piv', 'soft')),
            algo TEXT NOT NULL CHECK (algo IN ('ecdsa-p256', 'ed25519')),
            pubkey_pem TEXT NOT NULL,
            pubkey_fingerprint TEXT NOT NULL UNIQUE,
            host_label TEXT NOT NULL,
            label TEXT,
            serial TEXT,
            enrolled_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_seen_at TEXT,
            is_active BOOLEAN NOT NULL DEFAULT 1,
            revoked_at TEXT,
            revoke_reason TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_identity_keys_user ON identity_keys(user_id);
        CREATE INDEX IF NOT EXISTS idx_identity_keys_fpr ON identity_keys(pubkey_fingerprint);
        CREATE INDEX IF NOT EXISTS idx_identity_keys_active ON identity_keys(is_active);

        CREATE TABLE IF NOT EXISTS identities (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            identity_key_id INTEGER NOT NULL REFERENCES identity_keys(id) ON DELETE CASCADE,
            identity_hash TEXT NOT NULL UNIQUE,
            host_label TEXT NOT NULL,
            agent_label TEXT NOT NULL,
            model_label TEXT NOT NULL,
            first_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
            request_count INTEGER NOT NULL DEFAULT 0,
            is_active BOOLEAN NOT NULL DEFAULT 1
        );
        CREATE INDEX IF NOT EXISTS idx_identities_key ON identities(identity_key_id);
        CREATE INDEX IF NOT EXISTS idx_identities_hash ON identities(identity_hash);
        CREATE INDEX IF NOT EXISTS idx_identities_labels ON identities(host_label, agent_label, model_label);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!("Migration 46 complete: identity_keys + identities tables created");
    Ok(())
}

// ---------------------------------------------------------------------------
// Migration 47: audit_log identity columns
// ---------------------------------------------------------------------------

fn run_migration_audit_identity_columns(conn: &rusqlite::Connection) -> Result<()> {
    let has_identity_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('audit_log') WHERE name = 'identity_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    if has_identity_id > 0 {
        info!("audit_log.identity_id already present, migration 47 is a no-op");
        return Ok(());
    }

    conn.execute_batch(
        "ALTER TABLE audit_log ADD COLUMN identity_id INTEGER REFERENCES identities(id);
         ALTER TABLE audit_log ADD COLUMN tier TEXT;
         CREATE INDEX IF NOT EXISTS idx_audit_identity ON audit_log(identity_id);
         CREATE INDEX IF NOT EXISTS idx_audit_tier ON audit_log(tier);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!("Migration 47 complete: identity_id + tier columns added to audit_log");
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn open_test_db() -> rusqlite::Connection {
        rusqlite::Connection::open_in_memory().expect("open in-memory test db")
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
        let conn = open_test_db();

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
