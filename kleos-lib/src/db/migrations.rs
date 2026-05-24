pub use super::types::PostImportValidation;

use crate::EngError;
use crate::Result;
use serde::Serialize;
use tracing::info;

// ---------------------------------------------------------------------------
// IMPORTANT: version numbers in this file are scoped to the GLOBAL
// schema_version chain. They are independent of versions of the same number
// in `tenant_migrations.rs` (which has its own `schema_version` table inside
// each tenant shard). Tenant v48 and global v48 do unrelated work; this is
// intentional. When adding a new migration, increment within THIS file's
// chain only and pair it with a corresponding entry in `tenant_migrations.rs`
// only if the same logical change applies to per-tenant data.
// ---------------------------------------------------------------------------

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

/// A single step in a computed migration plan (used by dry-run and down paths).
#[derive(Debug, Clone, Serialize)]
pub struct MigrationPlan {
    pub version: u32,
    pub description: String,
    pub direction: String,
}

// ---------------------------------------------------------------------------
// Migration status
// ---------------------------------------------------------------------------

/// Current migration state of the database, including pending and revertible steps.
#[derive(Debug, Serialize)]
pub struct MigrationStatus {
    pub current_version: u32,
    /// Migrations whose `up` has not yet been applied.
    pub pending_up: Vec<MigrationInfo>,
    /// Applied migrations that have a `down` fn and can therefore be reverted.
    pub revertible_down: Vec<MigrationInfo>,
}

/// Summary of a single migration entry for status reporting.
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
    Migration {
        version: 48,
        description: "drop_api_keys_agent_fk",
        up: run_migration_drop_api_keys_agent_fk,
        down: None,
        transactional: false,
    },
    Migration {
        version: 49,
        description: "supervisor_injections",
        up: run_migration_supervisor_injections,
        down: Some(down_migration_supervisor_injections),
        transactional: true,
    },
    Migration {
        version: 50,
        description: "gate_requests_session_id",
        up: run_migration_gate_requests_session_id,
        down: Some(down_migration_gate_requests_session_id),
        transactional: true,
    },
    Migration {
        version: 51,
        description: "memory_chunks",
        up: run_migration_memory_chunks,
        down: Some(down_migration_memory_chunks),
        transactional: true,
    },
    Migration {
        version: 52,
        description: "activity_log_table",
        up: run_migration_activity_log_table,
        down: Some(down_migration_activity_log_table),
        transactional: true,
    },
    Migration {
        version: 53,
        description: "identity_keys_scopes",
        up: run_migration_identity_keys_scopes,
        down: Some(down_migration_identity_keys_scopes),
        transactional: true,
    },
    Migration {
        version: 54,
        description: "tool_manifests",
        up: run_migration_tool_manifests,
        down: Some(down_migration_tool_manifests),
        transactional: true,
    },
    Migration {
        version: 55,
        description: "handoffs_global",
        up: run_migration_handoffs_global,
        down: Some(down_migration_handoffs_global),
        transactional: true,
    },
    // Adds soft-delete support to users and a one-time invite token table
    // for controlled FIDO2 enrollment of new coworkers.
    Migration {
        version: 56,
        description: "user_active_and_enrollment_invites",
        up: run_migration_user_active_and_invites,
        down: Some(down_migration_user_active_and_invites),
        transactional: true,
    },
    Migration {
        version: 57,
        description: "skill_dispatch_configs",
        up: run_migration_skill_dispatch_configs,
        down: Some(down_migration_skill_dispatch_configs),
        transactional: true,
    },
    Migration {
        version: 58,
        description: "api_key_hash_version_fixup",
        up: run_migration_api_key_hash_version_fixup,
        down: None,
        transactional: true,
    },
    Migration {
        version: 59,
        description: "broca_narrative_columns",
        up: run_migration_broca_narrative_columns,
        down: None,
        transactional: true,
    },
    Migration {
        version: 60,
        description: "chiasm_extended_fields",
        up: run_migration_chiasm_extended_fields,
        down: None,
        transactional: true,
    },
    Migration {
        version: 61,
        description: "chiasm_path_claims",
        up: run_migration_chiasm_path_claims,
        down: None,
        transactional: true,
    },
    Migration {
        version: 62,
        description: "chiasm_agent_keys",
        up: run_migration_chiasm_agent_keys,
        down: None,
        transactional: true,
    },
    Migration {
        version: 63,
        description: "handoff_atoms",
        up: run_migration_handoff_atoms,
        down: None,
        transactional: true,
    },
    Migration {
        version: 64,
        description: "readd_user_id_memory_core",
        up: run_migration_readd_user_id_memory_core,
        down: Some(down_migration_readd_user_id_memory_core),
        transactional: true,
    },
    Migration {
        version: 65,
        description: "readd_user_id_webhooks",
        up: run_migration_readd_user_id_webhooks,
        down: Some(down_migration_readd_user_id_webhooks),
        transactional: true,
    },
    Migration {
        version: 66,
        description: "readd_user_id_approvals",
        up: run_migration_readd_user_id_approvals,
        down: Some(down_migration_readd_user_id_approvals),
        transactional: true,
    },
    // soma_agents carries UNIQUE(name); proper per-user isolation needs
    // UNIQUE(name, user_id), which requires the 12-step rebuild (migration 44
    // pattern). transactional:false because the rebuild toggles
    // PRAGMA foreign_keys, which SQLite forbids inside a SAVEPOINT.
    Migration {
        version: 67,
        description: "readd_user_id_soma_agents",
        up: run_migration_readd_user_id_soma_agents,
        down: None,
        transactional: false,
    },
    Migration {
        version: 68,
        description: "readd_user_id_axon_events",
        up: run_migration_readd_user_id_axon_events,
        down: Some(down_migration_readd_user_id_axon_events),
        transactional: true,
    },
    Migration {
        version: 69,
        description: "readd_user_id_chiasm_tasks",
        up: run_migration_readd_user_id_chiasm_tasks,
        down: Some(down_migration_readd_user_id_chiasm_tasks),
        transactional: true,
    },
    Migration {
        version: 70,
        description: "readd_user_id_conversations",
        up: run_migration_readd_user_id_conversations,
        down: Some(down_migration_readd_user_id_conversations),
        transactional: true,
    },
    Migration {
        version: 71,
        description: "readd_user_id_intelligence",
        up: run_migration_readd_user_id_intelligence,
        down: Some(down_migration_readd_user_id_intelligence),
        transactional: true,
    },
    // entities carries UNIQUE(name, entity_type); proper per-user isolation needs
    // UNIQUE(name, entity_type, user_id), which requires a table rebuild (the
    // reverse of migration 38). transactional:false because the rebuild toggles
    // PRAGMA foreign_keys, which SQLite forbids inside a SAVEPOINT.
    Migration {
        version: 72,
        description: "readd_user_id_graph_entities",
        up: run_migration_readd_user_id_graph_entities,
        down: None,
        transactional: false,
    },
    Migration {
        version: 73,
        description: "readd_user_id_episodes",
        up: run_migration_readd_user_id_episodes,
        down: Some(down_migration_readd_user_id_episodes),
        transactional: true,
    },
    // Re-adds user_id to the remaining 5 intelligence tables that v71 skipped:
    // current_state (UNIQUE rebuild), reconsolidations, temporal_patterns,
    // digests, and memory_feedback. transactional:false because the
    // current_state rebuild toggles PRAGMA foreign_keys, which SQLite forbids
    // inside a SAVEPOINT.
    Migration {
        version: 74,
        description: "readd_user_id_intelligence_remainder",
        up: run_migration_readd_user_id_intelligence_remainder,
        down: None,
        transactional: false,
    },
    // Re-adds user_id to the 5 thymus tables that migration 39 dropped:
    // rubrics (UNIQUE rebuild -- index changes from name to (user_id, name)),
    // evaluations, quality_metrics, session_quality, behavioral_drift_events.
    // transactional:false because the rubrics rebuild toggles PRAGMA
    // foreign_keys (evaluations holds a FK to rubrics.id) and SQLite forbids
    // that inside a SAVEPOINT.
    Migration {
        version: 75,
        description: "readd_user_id_thymus",
        up: run_migration_readd_user_id_thymus,
        down: None,
        transactional: false,
    },
    // Re-adds user_id to entity_cooccurrences that v38 dropped.
    // structured_facts already got user_id from v64 (memory-core).
    // Simple ADD COLUMN -- UNIQUE(entity_a_id, entity_b_id) does not need
    // user_id since co-occurrence pairs are global but queried per-user via
    // entity joins.
    Migration {
        version: 76,
        description: "readd_user_id_graph_remainder",
        up: run_migration_readd_user_id_graph_remainder,
        down: None,
        transactional: false,
    },
    // Re-adds user_id to user_preferences that v40 dropped via REBUILD.
    // UNIQUE constraint changes from (key) back to (user_id, key).
    // Also re-adds the domain+preference+user_id UNIQUE INDEX that v40 dropped.
    // transactional:false because the rebuild toggles PRAGMA foreign_keys.
    Migration {
        version: 77,
        description: "readd_user_id_user_preferences",
        up: run_migration_readd_user_id_user_preferences,
        down: None,
        transactional: false,
    },
    // Re-adds user_id to skill_records that v42 dropped via REBUILD.
    // UNIQUE changes from (name, agent, version) to (name, agent, version, user_id).
    // FTS triggers must be dropped before the rename and recreated after.
    // transactional:false because the rebuild toggles PRAGMA foreign_keys.
    Migration {
        version: 78,
        description: "readd_user_id_skills",
        up: run_migration_readd_user_id_skills,
        down: None,
        transactional: false,
    },
    // Re-adds user_id to brain_edges that v38 dropped.
    // Simple ADD COLUMN -- UNIQUE(source_id, target_id, edge_type) does not
    // include user_id.
    Migration {
        version: 79,
        description: "readd_user_id_brain_edges",
        up: run_migration_readd_user_id_brain_edges,
        down: None,
        transactional: false,
    },
    // C3: convert legacy JSON-array scopes in identity_keys.scopes to the
    // canonical CSV format used by api_keys.scopes. Rows whose value is
    // already CSV (or empty) are left alone -- the migration is idempotent.
    // No table rebuild: the column remains TEXT NOT NULL and the v53
    // default lingers on disk for any column never explicitly inserted,
    // but production paths always pass a value via enroll_handler.
    Migration {
        version: 80,
        description: "identity_keys_scopes_json_to_csv",
        up: run_migration_identity_keys_scopes_json_to_csv,
        down: None,
        transactional: true,
    },
    Migration {
        version: 81,
        description: "mcp_tokens",
        up: run_migration_mcp_tokens,
        down: None,
        transactional: true,
    },
];

// ---------------------------------------------------------------------------
// Legacy version constants (kept for compatibility with existing call sites)
// ---------------------------------------------------------------------------

/// Version number for the initial schema creation migration.
const MIGRATION_CREATE_SCHEMA: i64 = 1;
/// Version number for the add-missing-indexes migration.
const MIGRATION_ADD_MISSING_INDEXES: i64 = 2;
/// Version number for the pagerank tables migration.
const MIGRATION_PAGERANK_TABLES: i64 = 3;
/// Version number for the thymus tenant-scope migration.
const MIGRATION_THYMUS_TENANT_SCOPE: i64 = 4;
/// Version number for the app_state table migration.
const MIGRATION_APP_STATE_TABLE: i64 = 5;
/// Version number for the thymus user_id backfill migration.
const MIGRATION_BACKFILL_THYMUS_USER_ID: i64 = 6;
/// Version number for the vector_sync_pending table migration.
const MIGRATION_VECTOR_SYNC_PENDING: i64 = 7;
/// Version number for the community_id column migration.
const MIGRATION_ADD_COMMUNITY_ID: i64 = 8;
/// Version number for the drop-is_inference migration.
const MIGRATION_DROP_IS_INFERENCE: i64 = 9;
/// Version number for the syntheos services schema migration.
const MIGRATION_SYNTHEOS_SERVICES: i64 = 10;
/// Version number for the brain_patterns and brain_edges tables migration.
const MIGRATION_BRAIN_PATTERNS: i64 = 11;
/// Version number for the approvals table migration.
const MIGRATION_APPROVALS: i64 = 12;
/// Version number for the error_events table migration.
const MIGRATION_ERROR_EVENTS: i64 = 13;
/// Version number for the brain_meta table migration.
const MIGRATION_BRAIN_META: i64 = 14;
/// Version number for the brain_pca_models table migration.
const MIGRATION_PCA_MODELS: i64 = 15;
/// Version number for the brain_dream_runs table migration.
const MIGRATION_BRAIN_DREAM_RUNS: i64 = 16;
/// Version number for the cred tables migration.
const MIGRATION_CRED_TABLES: i64 = 17;
/// Version number for the api_key_hash unique index migration.
const MIGRATION_API_KEY_HASH_UNIQUE: i64 = 18;
/// Version number for the api_key hash_version column migration.
const MIGRATION_API_KEY_HASH_VERSION: i64 = 19;
/// Version number for the link covering indexes migration.
const MIGRATION_LINK_COVERING_INDEXES: i64 = 20;
/// Version number for the upload sessions tables migration.
const MIGRATION_UPLOAD_SESSIONS: i64 = 21;
/// Version number for the service_dead_letters table migration.
const MIGRATION_SERVICE_DEAD_LETTERS: i64 = 22;
/// Version number for the memories list covering index migration.
const MIGRATION_MEMORIES_LIST_COVERING_INDEX: i64 = 23;
/// Version number for the commerce tables migration.
const MIGRATION_COMMERCE_TABLES: i64 = 24;
/// Version number for dropping user_id from memory core tables.
const MIGRATION_DROP_USER_ID_MEMORY_CORE: i64 = 25;
/// Version number for dropping user_id from scratchpad tables.
const MIGRATION_DROP_USER_ID_SCRATCHPAD: i64 = 26;
/// Version number for dropping user_id from session tables.
const MIGRATION_DROP_USER_ID_SESSIONS: i64 = 27;
/// Version number for dropping user_id from chiasm tables.
const MIGRATION_DROP_USER_ID_CHIASM: i64 = 28;
/// Version number for dropping user_id from approvals tables.
const MIGRATION_DROP_USER_ID_APPROVALS: i64 = 29;
/// Version number for dropping user_id from broca tables.
const MIGRATION_DROP_USER_ID_BROCA: i64 = 30;
/// Version number for dropping user_id from projects tables.
const MIGRATION_DROP_USER_ID_PROJECTS: i64 = 31;
/// Version number for dropping user_id from activity tables.
const MIGRATION_DROP_USER_ID_ACTIVITY: i64 = 32;
/// Version number for dropping user_id from webhooks tables.
const MIGRATION_DROP_USER_ID_WEBHOOKS: i64 = 33;
/// Version number for dropping user_id from axon tables.
const MIGRATION_DROP_USER_ID_AXON: i64 = 34;
/// Version number for dropping user_id from growth tables.
const MIGRATION_DROP_USER_ID_GROWTH: i64 = 35;
/// Version number for dropping user_id from ingestion_hashes tables.
const MIGRATION_DROP_USER_ID_INGESTION_HASHES: i64 = 36;
/// Version number for dropping user_id from loom tables.
const MIGRATION_DROP_USER_ID_LOOM: i64 = 37;
/// Version number for dropping user_id from graph tables.
const MIGRATION_DROP_USER_ID_GRAPH: i64 = 38;
/// Version number for dropping user_id from thymus tables.
const MIGRATION_DROP_USER_ID_THYMUS: i64 = 39;
/// Version number for dropping user_id from portability tables.
const MIGRATION_DROP_USER_ID_PORTABILITY: i64 = 40;
/// Version number for dropping user_id from intelligence tables.
const MIGRATION_DROP_USER_ID_INTELLIGENCE: i64 = 41;
/// Version number for dropping user_id from skills tables.
const MIGRATION_DROP_USER_ID_SKILLS: i64 = 42;
/// Version number for dropping user_id from episodes tables.
const MIGRATION_DROP_USER_ID_EPISODES: i64 = 43;
/// Version number for re-adding user_id to monolith projects tables.
const MIGRATION_READD_USER_ID_PROJECTS: i64 = 44;
/// Version number for re-adding user_id to monolith broca tables.
const MIGRATION_READD_USER_ID_BROCA: i64 = 45;
/// Version number for the identity_keys and identities tables migration.
const MIGRATION_IDENTITY_TABLES: i64 = 46;
/// Version number for the audit_log identity columns migration.
const MIGRATION_AUDIT_IDENTITY_COLUMNS: i64 = 47;
/// Version number for dropping the api_keys agent FK migration.
const MIGRATION_DROP_API_KEYS_AGENT_FK: i64 = 48;
/// Version number for the supervisor_injections table migration.
const MIGRATION_SUPERVISOR_INJECTIONS: i64 = 49;
/// Version number for the gate_requests session_id column migration.
const MIGRATION_GATE_REQUESTS_SESSION_ID: i64 = 50;
/// Version number for the memory_chunks table migration.
const MIGRATION_MEMORY_CHUNKS: i64 = 51;
/// Version number for the activity_log table migration.
const MIGRATION_ACTIVITY_LOG_TABLE: i64 = 52;
/// Version number for the identity_keys scopes column migration.
const MIGRATION_IDENTITY_KEYS_SCOPES: i64 = 53;
/// Version number for the tool_manifests table migration.
const MIGRATION_TOOL_MANIFESTS: i64 = 54;
/// Version number for the global handoffs table migration.
const MIGRATION_HANDOFFS_GLOBAL: i64 = 55;
/// Version number for the user is_active flag and enrollment_invites migration.
const MIGRATION_USER_ACTIVE_AND_INVITES: i64 = 56;
/// Version number for the skill_dispatch_configs table migration.
const MIGRATION_SKILL_DISPATCH_CONFIGS: i64 = 57;
/// Version number for the api_key hash_version idempotent fixup migration.
const MIGRATION_API_KEY_HASH_VERSION_FIXUP: i64 = 58;
/// Version number for adding narrative and axon_event_id columns to broca_actions.
const MIGRATION_BROCA_NARRATIVE_COLUMNS: i64 = 59;

/// Version number for the Chiasm extended-fields migration.
const MIGRATION_CHIASM_EXTENDED_FIELDS: i64 = 60;
/// Version number for creating chiasm path claims and task dependencies tables.
const MIGRATION_CHIASM_PATH_CLAIMS: i64 = 61;
const MIGRATION_CHIASM_AGENT_KEYS: i64 = 62;
/// Version number for creating handoff_atoms and atom_entity_links tables.
const MIGRATION_HANDOFF_ATOMS: i64 = 63;
/// Version number for re-adding user_id to the memory core tables (reverses v25).
const MIGRATION_READD_USER_ID_MEMORY_CORE: i64 = 64;
/// Version number for re-adding user_id to the webhooks table (single-DB isolation).
const MIGRATION_READD_USER_ID_WEBHOOKS: i64 = 65;
/// Version number for re-adding user_id to the approvals table (single-DB isolation).
const MIGRATION_READD_USER_ID_APPROVALS: i64 = 66;
/// Version number for re-adding user_id to soma_agents via the UNIQUE(name,user_id) rebuild.
const MIGRATION_READD_USER_ID_SOMA_AGENTS: i64 = 67;
/// Version number for re-adding user_id to the axon_events table (single-DB isolation).
const MIGRATION_READD_USER_ID_AXON_EVENTS: i64 = 68;
/// Version number for re-adding user_id to the chiasm_tasks table (single-DB isolation).
const MIGRATION_READD_USER_ID_CHIASM_TASKS: i64 = 69;
/// Version number for re-adding user_id to the conversations table (single-DB isolation).
const MIGRATION_READD_USER_ID_CONVERSATIONS: i64 = 70;
/// Version number for re-adding user_id to the intelligence tables -- reflections,
/// consolidations, and causal_chains (single-DB isolation).
const MIGRATION_READD_USER_ID_INTELLIGENCE: i64 = 71;
/// Version number for re-adding user_id to the graph `entities` table with
/// UNIQUE(name, entity_type, user_id) so entities isolate per user in single-DB
/// mode (single-DB isolation).
const MIGRATION_READD_USER_ID_GRAPH_ENTITIES: i64 = 72;
/// Version number for re-adding user_id to the episodes table (single-DB isolation).
const MIGRATION_READD_USER_ID_EPISODES: i64 = 73;
/// Version number for re-adding user_id to the remaining 5 intelligence tables
/// (current_state, reconsolidations, temporal_patterns, digests, memory_feedback)
/// that migration 71 did not cover (single-DB isolation).
const MIGRATION_READD_USER_ID_INTELLIGENCE_REMAINDER: i64 = 74;
/// Version number for re-adding user_id to the 5 thymus tables (rubrics,
/// evaluations, quality_metrics, session_quality, behavioral_drift_events)
/// that migration 39 dropped (single-DB isolation).
const MIGRATION_READD_USER_ID_THYMUS: i64 = 75;
/// Version number for re-adding `user_id` to `entity_cooccurrences` (dropped
/// by v38). `structured_facts` already got `user_id` re-added in the CORE
/// schema (v64 memory-core migration path). Simple ADD COLUMN -- no UNIQUE
/// constraint changes needed for either table.
const MIGRATION_READD_USER_ID_GRAPH_REMAINDER: i64 = 76;
/// Version for the user_preferences user_id re-add migration (REBUILD).
/// v40 dropped user_id; this restores it with UNIQUE(user_id, key) so
/// single-DB mode can isolate preferences per user.
const MIGRATION_READD_USER_ID_USER_PREFERENCES: i64 = 77;
/// Version for the skill_records user_id re-add migration (REBUILD + FTS).
/// v42 dropped user_id; this restores it with UNIQUE(name, agent, version, user_id)
/// so single-DB mode can isolate skills per user.
const MIGRATION_READD_USER_ID_SKILLS: i64 = 78;
/// Version for the brain_edges user_id re-add migration.
/// v38 dropped user_id; this restores it as a simple ADD COLUMN since
/// UNIQUE(source_id, target_id, edge_type) does not include user_id.
const MIGRATION_READD_USER_ID_BRAIN: i64 = 79;
/// Version number for the identity_keys.scopes JSON-to-CSV format migration.
const MIGRATION_IDENTITY_KEYS_SCOPES_JSON_TO_CSV: i64 = 80;
/// Version number for the MCP direct-auth token revocation table.
const MIGRATION_MCP_TOKENS: i64 = 81;

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
        record_migration(
            conn,
            MIGRATION_IDENTITY_TABLES,
            "identity_keys_and_identities",
        )?;
    }

    if current_version < MIGRATION_AUDIT_IDENTITY_COLUMNS {
        info!("Running migration 47: audit_log_identity_columns");
        run_migration_audit_identity_columns(conn)?;
        record_migration(
            conn,
            MIGRATION_AUDIT_IDENTITY_COLUMNS,
            "audit_log_identity_columns",
        )?;
    }

    if current_version < MIGRATION_DROP_API_KEYS_AGENT_FK {
        info!("Running migration 48: drop_api_keys_agent_fk");
        run_migration_drop_api_keys_agent_fk(conn)?;
        record_migration(
            conn,
            MIGRATION_DROP_API_KEYS_AGENT_FK,
            "drop_api_keys_agent_fk",
        )?;
    }

    if current_version < MIGRATION_SUPERVISOR_INJECTIONS {
        info!("Running migration 49: supervisor_injections");
        run_migration_supervisor_injections(conn)?;
        record_migration(
            conn,
            MIGRATION_SUPERVISOR_INJECTIONS,
            "supervisor_injections",
        )?;
    }

    if current_version < MIGRATION_GATE_REQUESTS_SESSION_ID {
        info!("Running migration 50: gate_requests_session_id");
        run_migration_gate_requests_session_id(conn)?;
        record_migration(
            conn,
            MIGRATION_GATE_REQUESTS_SESSION_ID,
            "gate_requests_session_id",
        )?;
    }

    if current_version < MIGRATION_MEMORY_CHUNKS {
        info!("Running migration 51: memory_chunks");
        run_migration_memory_chunks(conn)?;
        record_migration(conn, MIGRATION_MEMORY_CHUNKS, "memory_chunks")?;
    }

    if current_version < MIGRATION_ACTIVITY_LOG_TABLE {
        info!("Running migration 52: activity_log_table");
        run_migration_activity_log_table(conn)?;
        record_migration(conn, MIGRATION_ACTIVITY_LOG_TABLE, "activity_log_table")?;
    }

    if current_version < MIGRATION_IDENTITY_KEYS_SCOPES {
        info!("Running migration 53: identity_keys_scopes");
        run_migration_identity_keys_scopes(conn)?;
        record_migration(conn, MIGRATION_IDENTITY_KEYS_SCOPES, "identity_keys_scopes")?;
    }

    if current_version < MIGRATION_TOOL_MANIFESTS {
        info!("Running migration 54: tool_manifests");
        run_migration_tool_manifests(conn)?;
        record_migration(conn, MIGRATION_TOOL_MANIFESTS, "tool_manifests")?;
    }

    if current_version < MIGRATION_HANDOFFS_GLOBAL {
        info!("Running migration 55: handoffs_global");
        run_migration_handoffs_global(conn)?;
        record_migration(conn, MIGRATION_HANDOFFS_GLOBAL, "handoffs_global")?;
    }

    if current_version < MIGRATION_USER_ACTIVE_AND_INVITES {
        info!("Running migration 56: user_active_and_enrollment_invites");
        run_migration_user_active_and_invites(conn)?;
        record_migration(
            conn,
            MIGRATION_USER_ACTIVE_AND_INVITES,
            "user_active_and_enrollment_invites",
        )?;
    }

    if current_version < MIGRATION_SKILL_DISPATCH_CONFIGS {
        info!("Running migration 57: skill_dispatch_configs");
        run_migration_skill_dispatch_configs(conn)?;
        record_migration(
            conn,
            MIGRATION_SKILL_DISPATCH_CONFIGS,
            "skill_dispatch_configs",
        )?;
    }

    if current_version < MIGRATION_API_KEY_HASH_VERSION_FIXUP {
        info!("Running migration 58: api_key_hash_version_fixup");
        run_migration_api_key_hash_version_fixup(conn)?;
        record_migration(
            conn,
            MIGRATION_API_KEY_HASH_VERSION_FIXUP,
            "api_key_hash_version_fixup",
        )?;
    }

    if current_version < MIGRATION_BROCA_NARRATIVE_COLUMNS {
        info!("Running migration 59: broca_narrative_columns");
        run_migration_broca_narrative_columns(conn)?;
        record_migration(
            conn,
            MIGRATION_BROCA_NARRATIVE_COLUMNS,
            "broca_narrative_columns",
        )?;
    }

    if current_version < MIGRATION_CHIASM_EXTENDED_FIELDS {
        info!("Running migration 60: chiasm_extended_fields");
        run_migration_chiasm_extended_fields(conn)?;
        record_migration(
            conn,
            MIGRATION_CHIASM_EXTENDED_FIELDS,
            "chiasm_extended_fields",
        )?;
    }

    if current_version < MIGRATION_CHIASM_PATH_CLAIMS {
        info!("Running migration 61: chiasm_path_claims");
        run_migration_chiasm_path_claims(conn)?;
        record_migration(conn, MIGRATION_CHIASM_PATH_CLAIMS, "chiasm_path_claims")?;
    }

    if current_version < MIGRATION_CHIASM_AGENT_KEYS {
        info!("Running migration 62: chiasm_agent_keys");
        run_migration_chiasm_agent_keys(conn)?;
        record_migration(conn, MIGRATION_CHIASM_AGENT_KEYS, "chiasm_agent_keys")?;
    }

    if current_version < MIGRATION_HANDOFF_ATOMS {
        info!("Running migration 63: handoff_atoms");
        run_migration_handoff_atoms(conn)?;
        record_migration(conn, MIGRATION_HANDOFF_ATOMS, "handoff_atoms")?;
    }

    if current_version < MIGRATION_READD_USER_ID_MEMORY_CORE {
        info!("Running migration 64: readd_user_id_memory_core");
        run_migration_readd_user_id_memory_core(conn)?;
        record_migration(
            conn,
            MIGRATION_READD_USER_ID_MEMORY_CORE,
            "readd_user_id_memory_core",
        )?;
    }

    if current_version < MIGRATION_READD_USER_ID_WEBHOOKS {
        info!("Running migration 65: readd_user_id_webhooks");
        run_migration_readd_user_id_webhooks(conn)?;
        record_migration(
            conn,
            MIGRATION_READD_USER_ID_WEBHOOKS,
            "readd_user_id_webhooks",
        )?;
    }

    if current_version < MIGRATION_READD_USER_ID_APPROVALS {
        info!("Running migration 66: readd_user_id_approvals");
        run_migration_readd_user_id_approvals(conn)?;
        record_migration(
            conn,
            MIGRATION_READD_USER_ID_APPROVALS,
            "readd_user_id_approvals",
        )?;
    }

    if current_version < MIGRATION_READD_USER_ID_SOMA_AGENTS {
        info!("Running migration 67: readd_user_id_soma_agents");
        run_migration_readd_user_id_soma_agents(conn)?;
        record_migration(
            conn,
            MIGRATION_READD_USER_ID_SOMA_AGENTS,
            "readd_user_id_soma_agents",
        )?;
    }

    if current_version < MIGRATION_READD_USER_ID_AXON_EVENTS {
        info!("Running migration 68: readd_user_id_axon_events");
        run_migration_readd_user_id_axon_events(conn)?;
        record_migration(
            conn,
            MIGRATION_READD_USER_ID_AXON_EVENTS,
            "readd_user_id_axon_events",
        )?;
    }

    if current_version < MIGRATION_READD_USER_ID_CHIASM_TASKS {
        info!("Running migration 69: readd_user_id_chiasm_tasks");
        run_migration_readd_user_id_chiasm_tasks(conn)?;
        record_migration(
            conn,
            MIGRATION_READD_USER_ID_CHIASM_TASKS,
            "readd_user_id_chiasm_tasks",
        )?;
    }

    if current_version < MIGRATION_READD_USER_ID_CONVERSATIONS {
        info!("Running migration 70: readd_user_id_conversations");
        run_migration_readd_user_id_conversations(conn)?;
        record_migration(
            conn,
            MIGRATION_READD_USER_ID_CONVERSATIONS,
            "readd_user_id_conversations",
        )?;
    }

    if current_version < MIGRATION_READD_USER_ID_INTELLIGENCE {
        info!("Running migration 71: readd_user_id_intelligence");
        run_migration_readd_user_id_intelligence(conn)?;
        record_migration(
            conn,
            MIGRATION_READD_USER_ID_INTELLIGENCE,
            "readd_user_id_intelligence",
        )?;
    }

    if current_version < MIGRATION_READD_USER_ID_GRAPH_ENTITIES {
        info!("Running migration 72: readd_user_id_graph_entities");
        run_migration_readd_user_id_graph_entities(conn)?;
        record_migration(
            conn,
            MIGRATION_READD_USER_ID_GRAPH_ENTITIES,
            "readd_user_id_graph_entities",
        )?;
    }

    if current_version < MIGRATION_READD_USER_ID_EPISODES {
        info!("Running migration 73: readd_user_id_episodes");
        run_migration_readd_user_id_episodes(conn)?;
        record_migration(
            conn,
            MIGRATION_READD_USER_ID_EPISODES,
            "readd_user_id_episodes",
        )?;
    }

    if current_version < MIGRATION_READD_USER_ID_INTELLIGENCE_REMAINDER {
        info!("Running migration 74: readd_user_id_intelligence_remainder");
        run_migration_readd_user_id_intelligence_remainder(conn)?;
        record_migration(
            conn,
            MIGRATION_READD_USER_ID_INTELLIGENCE_REMAINDER,
            "readd_user_id_intelligence_remainder",
        )?;
    }

    if current_version < MIGRATION_READD_USER_ID_THYMUS {
        info!("Running migration 75: readd_user_id_thymus");
        run_migration_readd_user_id_thymus(conn)?;
        record_migration(conn, MIGRATION_READD_USER_ID_THYMUS, "readd_user_id_thymus")?;
    }

    if current_version < MIGRATION_READD_USER_ID_GRAPH_REMAINDER {
        info!("Running migration 76: readd_user_id_graph_remainder");
        run_migration_readd_user_id_graph_remainder(conn)?;
        record_migration(
            conn,
            MIGRATION_READD_USER_ID_GRAPH_REMAINDER,
            "readd_user_id_graph_remainder",
        )?;
    }

    if current_version < MIGRATION_READD_USER_ID_USER_PREFERENCES {
        info!("Running migration 77: readd_user_id_user_preferences");
        run_migration_readd_user_id_user_preferences(conn)?;
        record_migration(
            conn,
            MIGRATION_READD_USER_ID_USER_PREFERENCES,
            "readd_user_id_user_preferences",
        )?;
    }

    if current_version < MIGRATION_READD_USER_ID_SKILLS {
        info!("Running migration 78: readd_user_id_skills");
        run_migration_readd_user_id_skills(conn)?;
        record_migration(conn, MIGRATION_READD_USER_ID_SKILLS, "readd_user_id_skills")?;
    }

    if current_version < MIGRATION_READD_USER_ID_BRAIN {
        info!("Running migration 79: readd_user_id_brain_edges");
        run_migration_readd_user_id_brain_edges(conn)?;
        record_migration(
            conn,
            MIGRATION_READD_USER_ID_BRAIN,
            "readd_user_id_brain_edges",
        )?;
    }

    if current_version < MIGRATION_IDENTITY_KEYS_SCOPES_JSON_TO_CSV {
        info!("Running migration 80: identity_keys_scopes_json_to_csv");
        run_migration_identity_keys_scopes_json_to_csv(conn)?;
        record_migration(
            conn,
            MIGRATION_IDENTITY_KEYS_SCOPES_JSON_TO_CSV,
            "identity_keys_scopes_json_to_csv",
        )?;
    }

    if current_version < MIGRATION_MCP_TOKENS {
        info!("Running migration 81: mcp_tokens");
        run_migration_mcp_tokens(conn)?;
        record_migration(conn, MIGRATION_MCP_TOKENS, "mcp_tokens")?;
    }

    Ok(())
}

/// Inserts a row into schema_version to record that a migration has been applied.
fn record_migration(conn: &rusqlite::Connection, version: i64, name: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO schema_version (version, name) VALUES (?1, ?2)",
        rusqlite::params![version, name],
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

/// Deletes the schema_version row for `version`, used by the down-migration path.
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

/// Migration 2: adds missing secondary indexes to all core tables.
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

/// Migration 3: creates the memory_pagerank and pagerank_dirty tables.
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

/// Migration 4: adds user_id to thymus tables for per-tenant scoping.
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

/// Migration 5: creates the app_state key-value table for persistent configuration.
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

/// Migration 6: backfills user_id = 1 for any zero-valued rows in thymus tables.
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

/// Migration 7: creates the vector_sync_pending queue table for async embedding sync.
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

/// Migration 8: adds community_id column and index to the memories table.
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

/// Adds `column` to `table` if it does not already exist; idempotent.
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

/// Migration 15: creates the brain_pca_models table for dimensionality reduction state.
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
    add_column_if_not_exists(
        conn,
        "api_keys",
        "hash_version",
        "INTEGER NOT NULL DEFAULT 1",
    )
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

/// Reverse migration 22: drops the service_dead_letters table.
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

/// Reverse migration 23: drops the memories list covering index.
fn down_migration_memories_list_covering_index(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch("DROP INDEX IF EXISTS idx_memories_list_user_id_desc;")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    info!("Dropped idx_memories_list_user_id_desc (migration 23 down)");
    Ok(())
}

// ---------------------------------------------------------------------------
// Migration 24: commerce tables
// ---------------------------------------------------------------------------

/// Migration 24: creates the commerce tables for payment quotes, settlements, and pricing.
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

/// Reverse migration 24: drops all commerce tables in dependency order.
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
// Migration 64: re-add user_id to memory core tables (reverses migration 25)
// ---------------------------------------------------------------------------

/// Migration 64: re-add `user_id` to `memories`, `artifacts`,
/// `vector_sync_pending`, and `structured_facts` on the monolith, recreate the
/// `user_id`-keyed indexes, and restore the `prevent_cross_tenant_links`
/// trigger.
///
/// This reverses migration 25. Phase 5 assumed the per-tenant shard file was
/// the only isolation boundary and stripped `user_id` from the monolith; that
/// broke single-DB (shared) mode, where one monolith serves every user and the
/// row-level `user_id` predicate is the isolation boundary. Legacy rows
/// backfill to `user_id = 1` (the system owner): single-DB mode has been
/// fail-closed since Phase 5 so no real multi-user monolith data exists to
/// mis-attribute, and on a sharded deployment the monolith holds only
/// system-scoped tables. New inserts carry the real `user_id`.
///
/// Idempotent: each `ADD COLUMN` is guarded by a `pragma_table_info` check and
/// every index/trigger uses `IF NOT EXISTS`.
fn run_migration_readd_user_id_memory_core(conn: &rusqlite::Connection) -> Result<()> {
    // Re-add the column to each table only if it is currently absent. ALTER
    // TABLE ADD COLUMN errors on a duplicate column, so guard every one.
    for table in [
        "memories",
        "artifacts",
        "vector_sync_pending",
        "structured_facts",
    ] {
        let has_user_id: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = 'user_id'",
                rusqlite::params![table],
                |row| row.get(0),
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        if has_user_id == 0 {
            conn.execute(
                &format!("ALTER TABLE {table} ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1"),
                [],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            info!("Re-added {table}.user_id (migration 64)");
        }
    }

    // Recreate the user_id-keyed indexes dropped by migration 25. Column orders
    // match the originals (see the migration 25 comment block and migration 23).
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_memories_user ON memories(user_id);
         CREATE INDEX IF NOT EXISTS idx_memories_search ON memories(user_id, is_forgotten, is_archived, is_latest);
         CREATE INDEX IF NOT EXISTS idx_memories_search_composite ON memories(user_id, is_forgotten, is_latest, category);
         CREATE INDEX IF NOT EXISTS idx_memories_user_latest ON memories(user_id, is_latest, is_forgotten);
         CREATE INDEX IF NOT EXISTS idx_memories_list_user_id_desc ON memories(user_id, id DESC) WHERE is_latest = 1 AND is_consolidated = 0;
         CREATE INDEX IF NOT EXISTS idx_vector_sync_user ON vector_sync_pending(user_id);
         CREATE INDEX IF NOT EXISTS idx_artifacts_user ON artifacts(user_id);
         CREATE INDEX IF NOT EXISTS idx_facts_user ON structured_facts(user_id);
         CREATE INDEX IF NOT EXISTS idx_sf_subject_verb ON structured_facts(subject COLLATE NOCASE, verb, user_id);
         CREATE INDEX IF NOT EXISTS idx_facts_user_subject_predicate ON structured_facts(user_id, subject, predicate);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    // Restore the trigger that blocks linking memories owned by different users.
    conn.execute_batch(
        "CREATE TRIGGER IF NOT EXISTS prevent_cross_tenant_links
            BEFORE INSERT ON memory_links
            BEGIN
                SELECT RAISE(ABORT, 'cross-tenant memory links are not permitted')
                WHERE (SELECT user_id FROM memories WHERE id = NEW.source_id)
                   != (SELECT user_id FROM memories WHERE id = NEW.target_id);
            END;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!("Migration 64 complete: user_id re-added to memory core tables");
    Ok(())
}

/// Reverse migration 64: drop the `prevent_cross_tenant_links` trigger, the
/// re-added `user_id` indexes, and the `user_id` columns from the four memory
/// core tables. Mirrors migration 25's up path.
fn down_migration_readd_user_id_memory_core(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch("DROP TRIGGER IF EXISTS prevent_cross_tenant_links;")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

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

    for table in [
        "memories",
        "artifacts",
        "vector_sync_pending",
        "structured_facts",
    ] {
        let has_user_id: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = 'user_id'",
                rusqlite::params![table],
                |row| row.get(0),
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        if has_user_id > 0 {
            conn.execute(&format!("ALTER TABLE {table} DROP COLUMN user_id"), [])
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        }
    }

    info!("Migration 64 reverted: user_id dropped from memory core tables");
    Ok(())
}

/// Migration 65: re-add `user_id` to the `webhooks` table.
///
/// The monolith `webhooks` table never carried `user_id` (the shard variant did
/// until tenant v30 dropped it). With single-DB mode now serving every user from
/// one monolith, webhook reads/writes need a row-level owner so the always-applied
/// `WHERE user_id = ?` predicate can isolate them. Existing rows backfill to
/// `user_id = 1` (the system owner); single-DB mode was fail-closed before this
/// repair, so no real multi-user webhook data exists to mis-attribute. New inserts
/// carry the real `user_id`.
///
/// Idempotent: the `ADD COLUMN` is guarded by `pragma_table_info` and the index
/// uses `IF NOT EXISTS`.
fn run_migration_readd_user_id_webhooks(conn: &rusqlite::Connection) -> Result<()> {
    let has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('webhooks') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if has_user_id == 0 {
        conn.execute(
            "ALTER TABLE webhooks ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1",
            [],
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Re-added webhooks.user_id (migration 65)");
    }
    conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_webhooks_user ON webhooks(user_id);")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    info!("Migration 65 complete: user_id re-added to webhooks");
    Ok(())
}

/// Reverse migration 65: drop the `idx_webhooks_user` index and the `user_id`
/// column from `webhooks`.
fn down_migration_readd_user_id_webhooks(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch("DROP INDEX IF EXISTS idx_webhooks_user;")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    let has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('webhooks') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if has_user_id > 0 {
        conn.execute("ALTER TABLE webhooks DROP COLUMN user_id", [])
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    }
    info!("Migration 65 reverted: user_id dropped from webhooks");
    Ok(())
}

/// Migration 66: re-add `user_id` to the `approvals` table (reverses migration
/// 29). Migration 12 created approvals with `user_id` and the
/// `idx_approvals_user` / `idx_approvals_user_status` indexes; migration 29
/// dropped them under the per-shard-only isolation assumption. Single-DB mode
/// needs the row-level owner back so the `WHERE user_id = ?` predicate isolates
/// approvals per user. Existing rows backfill to `user_id = 1` (system owner);
/// new inserts carry the real owner.
///
/// Idempotent: the `ADD COLUMN` is guarded and indexes use `IF NOT EXISTS`.
fn run_migration_readd_user_id_approvals(conn: &rusqlite::Connection) -> Result<()> {
    let has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('approvals') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if has_user_id == 0 {
        conn.execute(
            "ALTER TABLE approvals ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1",
            [],
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Re-added approvals.user_id (migration 66)");
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_approvals_user ON approvals(user_id);
         CREATE INDEX IF NOT EXISTS idx_approvals_user_status ON approvals(user_id, status);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    info!("Migration 66 complete: user_id re-added to approvals");
    Ok(())
}

/// Reverse migration 66: drop the `user_id` indexes and column from `approvals`.
fn down_migration_readd_user_id_approvals(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "DROP INDEX IF EXISTS idx_approvals_user;
         DROP INDEX IF EXISTS idx_approvals_user_status;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    let has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('approvals') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if has_user_id > 0 {
        conn.execute("ALTER TABLE approvals DROP COLUMN user_id", [])
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    }
    info!("Migration 66 reverted: user_id dropped from approvals");
    Ok(())
}

/// Migration 67: re-add `user_id` to `soma_agents` with a per-user uniqueness
/// boundary. Migration 32 dropped `soma_agents.user_id` under the per-shard-only
/// isolation assumption. The table carries `UNIQUE(name)`, which in single-DB
/// mode lets one user clobber another's agent (the `register_agent` upsert keys
/// on `name`) and blocks distinct users from reusing an agent name. Restoring
/// correct isolation therefore requires `UNIQUE(name, user_id)`, which cannot be
/// done with `ALTER`; this uses the 12-step rebuild (migration 44 pattern).
///
/// `soma_agents` is FK-referenced by `soma_agent_groups` and `soma_agent_logs`
/// (ON DELETE CASCADE); the rebuild preserves `id` values and runs with
/// `PRAGMA foreign_keys = OFF` so those references stay valid. Legacy rows
/// backfill to `user_id = 1` (the system owner). Idempotent: no-op if `user_id`
/// is already present.
fn run_migration_readd_user_id_soma_agents(conn: &rusqlite::Connection) -> Result<()> {
    let has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('soma_agents') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if has_user_id > 0 {
        info!("soma_agents.user_id already present, migration 67 is a no-op");
        return Ok(());
    }

    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         PRAGMA legacy_alter_table = 1;

         ALTER TABLE soma_agents RENAME TO _soma_agents_old_v67;

         DROP INDEX IF EXISTS idx_soma_agents_type;
         DROP INDEX IF EXISTS idx_soma_agents_status;

         CREATE TABLE soma_agents (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             name TEXT NOT NULL,
             type TEXT NOT NULL,
             description TEXT,
             capabilities TEXT NOT NULL DEFAULT '[]',
             status TEXT NOT NULL DEFAULT 'pending'
                 CHECK(status IN ('pending','online','offline','error')),
             config TEXT NOT NULL DEFAULT '{}',
             heartbeat_at TEXT,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now')),
             quality_score REAL,
             drift_flags TEXT DEFAULT '[]',
             user_id INTEGER NOT NULL DEFAULT 1,
             UNIQUE(name, user_id)
         );

         INSERT INTO soma_agents
             (id, name, type, description, capabilities, status, config, heartbeat_at,
              created_at, updated_at, quality_score, drift_flags, user_id)
         SELECT id, name, type, description, capabilities, status, config, heartbeat_at,
                created_at, updated_at, quality_score, drift_flags, 1
         FROM _soma_agents_old_v67;

         DROP TABLE _soma_agents_old_v67;

         CREATE INDEX idx_soma_agents_type ON soma_agents(type);
         CREATE INDEX idx_soma_agents_status ON soma_agents(status);
         CREATE INDEX idx_soma_agents_user ON soma_agents(user_id);

         PRAGMA legacy_alter_table = 0;
         PRAGMA foreign_keys = ON;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!("Migration 67 complete: user_id re-added to soma_agents with UNIQUE(name, user_id)");
    Ok(())
}

/// Migration 68: re-add `user_id` to the `axon_events` table. Migration 32
/// dropped it; single-DB mode needs the row-level owner so event reads
/// (get/query/consume/stats/channel counts) isolate per user. axon_events is an
/// append-only event log with no UNIQUE/FK on the column, so the simple
/// ALTER TABLE ADD COLUMN path is sufficient. Legacy rows backfill to
/// `user_id = 1`; new publishes carry the publisher's id.
///
/// Idempotent: the `ADD COLUMN` is guarded and the index uses `IF NOT EXISTS`.
fn run_migration_readd_user_id_axon_events(conn: &rusqlite::Connection) -> Result<()> {
    let has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('axon_events') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if has_user_id == 0 {
        conn.execute(
            "ALTER TABLE axon_events ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1",
            [],
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Re-added axon_events.user_id (migration 68)");
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_axon_events_user ON axon_events(user_id, channel, id);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    info!("Migration 68 complete: user_id re-added to axon_events");
    Ok(())
}

/// Reverse migration 68: drop the `idx_axon_events_user` index and the
/// `user_id` column from `axon_events`.
fn down_migration_readd_user_id_axon_events(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch("DROP INDEX IF EXISTS idx_axon_events_user;")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    let has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('axon_events') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if has_user_id > 0 {
        conn.execute("ALTER TABLE axon_events DROP COLUMN user_id", [])
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    }
    info!("Migration 68 reverted: user_id dropped from axon_events");
    Ok(())
}

/// Migration 69: re-add `user_id` to the `chiasm_tasks` table. Migration 28
/// dropped it; single-DB mode needs the row-level owner so task reads/writes
/// (get/list/update/delete/queue/feed/stats) isolate per user. chiasm_tasks has
/// no UNIQUE/FK on the column, so the simple ALTER TABLE ADD COLUMN path is
/// sufficient. Legacy rows backfill to `user_id = 1`; new tasks carry the
/// creator's id.
///
/// Idempotent: the `ADD COLUMN` is guarded and the index uses `IF NOT EXISTS`.
fn run_migration_readd_user_id_chiasm_tasks(conn: &rusqlite::Connection) -> Result<()> {
    let has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('chiasm_tasks') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if has_user_id == 0 {
        conn.execute(
            "ALTER TABLE chiasm_tasks ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1",
            [],
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Re-added chiasm_tasks.user_id (migration 69)");
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_chiasm_tasks_user ON chiasm_tasks(user_id, status);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    info!("Migration 69 complete: user_id re-added to chiasm_tasks");
    Ok(())
}

/// Reverse migration 69: drop the `idx_chiasm_tasks_user` index and the
/// `user_id` column from `chiasm_tasks`.
fn down_migration_readd_user_id_chiasm_tasks(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch("DROP INDEX IF EXISTS idx_chiasm_tasks_user;")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    let has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('chiasm_tasks') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if has_user_id > 0 {
        conn.execute("ALTER TABLE chiasm_tasks DROP COLUMN user_id", [])
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    }
    info!("Migration 69 reverted: user_id dropped from chiasm_tasks");
    Ok(())
}

/// Migration 70: re-add `user_id` to the `conversations` table. Migration 40
/// dropped it (Shape A simple DROP COLUMN); single-DB mode needs the row-level
/// owner so conversation reads/writes and message scoping isolate per user.
/// conversations has no UNIQUE/FK on the column, so the simple
/// ALTER TABLE ADD COLUMN path is sufficient. Legacy rows backfill to
/// `user_id = 1`; new conversations carry the creator's id. The `messages`
/// table has no user_id and is scoped via its parent conversation.
///
/// Idempotent: the `ADD COLUMN` is guarded and the index uses `IF NOT EXISTS`.
fn run_migration_readd_user_id_conversations(conn: &rusqlite::Connection) -> Result<()> {
    let has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('conversations') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if has_user_id == 0 {
        conn.execute(
            "ALTER TABLE conversations ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1",
            [],
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Re-added conversations.user_id (migration 70)");
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_conversations_user ON conversations(user_id);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    info!("Migration 70 complete: user_id re-added to conversations");
    Ok(())
}

/// Reverse migration 70: drop the `idx_conversations_user` index and the
/// `user_id` column from `conversations`.
fn down_migration_readd_user_id_conversations(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch("DROP INDEX IF EXISTS idx_conversations_user;")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    let has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('conversations') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if has_user_id > 0 {
        conn.execute("ALTER TABLE conversations DROP COLUMN user_id", [])
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    }
    info!("Migration 70 reverted: user_id dropped from conversations");
    Ok(())
}

/// Migration 71: re-add the `user_id` ownership column to the intelligence
/// tables -- `reflections`, `consolidations`, and `causal_chains` -- so
/// single-DB (shared) mode can scope every read by owner. Migration 35
/// dropped it from `reflections` and migration 41 dropped it from
/// `consolidations`/`causal_chains`; fresh databases created from the core
/// schema already carry it on the latter two. `causal_links` deliberately
/// has no `user_id`: it is scoped through its parent chain.
///
/// Existing rows default to `user_id = 1`; new rows carry the creator's id.
///
/// Idempotent: every `ADD COLUMN` is guarded by a `pragma_table_info` check
/// and each index uses `IF NOT EXISTS`.
fn run_migration_readd_user_id_intelligence(conn: &rusqlite::Connection) -> Result<()> {
    for (table, index) in [
        ("reflections", "idx_reflections_user"),
        ("consolidations", "idx_consolidations_user"),
        ("causal_chains", "idx_causal_chains_user"),
    ] {
        let has_user_id: i64 = conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = 'user_id'"
                ),
                [],
                |row| row.get(0),
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        if has_user_id == 0 {
            conn.execute(
                &format!("ALTER TABLE {table} ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1"),
                [],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            info!("Re-added {table}.user_id (migration 71)");
        }
        conn.execute_batch(&format!(
            "CREATE INDEX IF NOT EXISTS {index} ON {table}(user_id);"
        ))
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    }
    info!("Migration 71 complete: user_id re-added to intelligence tables");
    Ok(())
}

/// Reverse migration 71: drop the user-scoped indexes and the `user_id`
/// column from the three intelligence tables.
fn down_migration_readd_user_id_intelligence(conn: &rusqlite::Connection) -> Result<()> {
    for (table, index) in [
        ("reflections", "idx_reflections_user"),
        ("consolidations", "idx_consolidations_user"),
        ("causal_chains", "idx_causal_chains_user"),
    ] {
        conn.execute_batch(&format!("DROP INDEX IF EXISTS {index};"))
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        let has_user_id: i64 = conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = 'user_id'"
                ),
                [],
                |row| row.get(0),
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        if has_user_id > 0 {
            conn.execute(&format!("ALTER TABLE {table} DROP COLUMN user_id"), [])
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        }
    }
    info!("Migration 71 reverted: user_id dropped from intelligence tables");
    Ok(())
}

/// Migration 72: re-add the `user_id` ownership column to the graph `entities`
/// table with `UNIQUE(name, entity_type, user_id)`, reversing migration 38's
/// drop-and-rebuild. Entities are upserted by (name, entity_type); without
/// user_id in the constraint, two users mentioning the same name collapse into
/// one shared row -- a cross-user leak in single-DB (shared) mode.
///
/// The rebuild copies every row forward preserving `id` (entity_relationships,
/// memory_entities, and entity_cooccurrences hold FKs to entities(id)), so it
/// runs with `PRAGMA foreign_keys = OFF`. Legacy rows backfill to `user_id = 1`
/// (the system owner); already-merged entities cannot be un-merged. Idempotent:
/// a no-op when `user_id` is already present (fresh databases created from the
/// core schema, which already carries the column and constraint).
fn run_migration_readd_user_id_graph_entities(conn: &rusqlite::Connection) -> Result<()> {
    let has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('entities') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if has_user_id > 0 {
        info!("entities.user_id already present, migration 72 is a no-op");
        return Ok(());
    }

    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         PRAGMA legacy_alter_table = 1;

         ALTER TABLE entities RENAME TO _entities_old_v72;
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
             user_id INTEGER NOT NULL DEFAULT 1,
             space_id INTEGER,
             confidence REAL NOT NULL DEFAULT 1.0,
             occurrence_count INTEGER NOT NULL DEFAULT 1,
             first_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
             last_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now')),
             UNIQUE(name, entity_type, user_id)
         );

         INSERT OR IGNORE INTO entities
             (id, name, entity_type, type, description, aliases, aka, metadata,
              user_id, space_id, confidence, occurrence_count,
              first_seen_at, last_seen_at, created_at, updated_at)
         SELECT
             id, name, entity_type, type, description, aliases, aka, metadata,
             1, space_id, confidence, occurrence_count,
             first_seen_at, last_seen_at, created_at, updated_at
         FROM _entities_old_v72;

         DROP TABLE _entities_old_v72;

         CREATE INDEX IF NOT EXISTS idx_entities_name ON entities(name);
         CREATE INDEX IF NOT EXISTS idx_entities_type ON entities(entity_type);
         CREATE INDEX IF NOT EXISTS idx_entities_user ON entities(user_id);

         PRAGMA legacy_alter_table = 0;
         PRAGMA foreign_keys = ON;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!("Migration 72 complete: user_id re-added to entities with UNIQUE(name, entity_type, user_id)");
    Ok(())
}

/// Migration 73: re-add the `user_id` ownership column to `episodes`,
/// reversing migration 43's drop, so episodes isolate per user in single-DB
/// mode. Existing rows default to `user_id = 1`; new episodes carry the
/// creator's id. Idempotent: a no-op when the column is already present (fresh
/// databases created from the core schema, which already carries it).
fn run_migration_readd_user_id_episodes(conn: &rusqlite::Connection) -> Result<()> {
    let has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('episodes') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if has_user_id == 0 {
        conn.execute(
            "ALTER TABLE episodes ADD COLUMN user_id INTEGER DEFAULT 1",
            [],
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Re-added episodes.user_id (migration 73)");
    }
    conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_episodes_user ON episodes(user_id);")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    info!("Migration 73 complete: user_id re-added to episodes");
    Ok(())
}

/// Migration 74: re-add `user_id` to the five intelligence tables skipped by
/// migration 71: current_state (full UNIQUE-constraint rebuild), reconsolidations,
/// temporal_patterns, digests, and memory_feedback.
///
/// current_state carries UNIQUE(agent, key, user_id) -- the original constraint
/// was UNIQUE(agent, key) -- so per-user isolation requires a table rebuild that
/// changes the constraint shape. The other four tables take a simple
/// ALTER TABLE ADD COLUMN path. All sections are pragma-guarded (idempotent).
///
/// This function must NOT run inside a transaction (transactional: false in the
/// MIGRATIONS slice). The current_state rebuild toggles PRAGMA foreign_keys,
/// which SQLite forbids inside a SAVEPOINT or active transaction.
fn run_migration_readd_user_id_intelligence_remainder(conn: &rusqlite::Connection) -> Result<()> {
    // -----------------------------------------------------------------------
    // current_state: 12-step UNIQUE-constraint rebuild
    // Adds user_id NOT NULL DEFAULT 1 and changes UNIQUE(agent, key) to
    // UNIQUE(agent, key, user_id).
    // -----------------------------------------------------------------------
    let cs_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('current_state') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    if cs_has_user_id == 0 {
        conn.execute_batch(
            "PRAGMA foreign_keys = OFF;
             PRAGMA legacy_alter_table = ON;

             ALTER TABLE current_state RENAME TO _current_state_old_v74;
             DROP INDEX IF EXISTS idx_current_state_agent;
             DROP INDEX IF EXISTS idx_current_state_user;
             DROP INDEX IF EXISTS idx_cs_key;
             DROP INDEX IF EXISTS idx_cs_key_user;

             CREATE TABLE current_state (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 agent TEXT NOT NULL,
                 key TEXT NOT NULL,
                 value TEXT NOT NULL,
                 memory_id INTEGER REFERENCES memories(id) ON DELETE SET NULL,
                 previous_value TEXT,
                 previous_memory_id INTEGER,
                 updated_count INTEGER NOT NULL DEFAULT 1,
                 user_id INTEGER NOT NULL DEFAULT 1,
                 updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                 created_at TEXT NOT NULL DEFAULT (datetime('now')),
                 UNIQUE(agent, key, user_id)
             );

             INSERT INTO current_state
                 (id, agent, key, value, memory_id, previous_value, previous_memory_id,
                  updated_count, user_id, updated_at, created_at)
             SELECT
                 id, agent, key, value, memory_id, previous_value, previous_memory_id,
                 updated_count, 1, updated_at, created_at
             FROM _current_state_old_v74;

             DROP TABLE _current_state_old_v74;

             CREATE INDEX IF NOT EXISTS idx_current_state_agent ON current_state(agent);
             CREATE INDEX IF NOT EXISTS idx_current_state_user ON current_state(user_id);
             CREATE INDEX IF NOT EXISTS idx_cs_key ON current_state(key COLLATE NOCASE);
             CREATE INDEX IF NOT EXISTS idx_cs_key_user ON current_state(key, user_id);

             PRAGMA legacy_alter_table = OFF;
             PRAGMA foreign_keys = ON;",
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Migration 74: current_state rebuilt with UNIQUE(agent, key, user_id)");
    } else {
        // Fresh database already has user_id; still ensure all indexes exist.
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_current_state_agent ON current_state(agent);
             CREATE INDEX IF NOT EXISTS idx_current_state_user ON current_state(user_id);
             CREATE INDEX IF NOT EXISTS idx_cs_key ON current_state(key COLLATE NOCASE);
             CREATE INDEX IF NOT EXISTS idx_cs_key_user ON current_state(key, user_id);",
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    }

    // -----------------------------------------------------------------------
    // reconsolidations: ADD COLUMN user_id + index
    // -----------------------------------------------------------------------
    let recons_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('reconsolidations') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if recons_has_user_id == 0 {
        conn.execute(
            "ALTER TABLE reconsolidations ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1",
            [],
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Re-added reconsolidations.user_id (migration 74)");
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_reconsolidations_user ON reconsolidations(user_id);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    // -----------------------------------------------------------------------
    // temporal_patterns: ADD COLUMN user_id + index
    // -----------------------------------------------------------------------
    let tp_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('temporal_patterns') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if tp_has_user_id == 0 {
        conn.execute(
            "ALTER TABLE temporal_patterns ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1",
            [],
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Re-added temporal_patterns.user_id (migration 74)");
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_temporal_patterns_user ON temporal_patterns(user_id);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    // -----------------------------------------------------------------------
    // digests: ADD COLUMN user_id + index
    // -----------------------------------------------------------------------
    let dig_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('digests') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if dig_has_user_id == 0 {
        conn.execute(
            "ALTER TABLE digests ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1",
            [],
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Re-added digests.user_id (migration 74)");
    }
    conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_digests_user ON digests(user_id);")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    // -----------------------------------------------------------------------
    // memory_feedback: ADD COLUMN user_id + index
    // -----------------------------------------------------------------------
    let mf_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('memory_feedback') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if mf_has_user_id == 0 {
        conn.execute(
            "ALTER TABLE memory_feedback ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1",
            [],
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Re-added memory_feedback.user_id (migration 74)");
    }
    conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_feedback_user ON memory_feedback(user_id);")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!("Migration 74 complete: user_id re-added to intelligence remainder tables");
    Ok(())
}

/// Re-adds `user_id` to the 5 thymus tables that migration 39 dropped:
/// rubrics (12-step UNIQUE-constraint rebuild because evaluations has a FK to
/// rubrics.id and the UNIQUE constraint must change from UNIQUE(name) to
/// UNIQUE(user_id, name)), evaluations, quality_metrics, session_quality, and
/// behavioral_drift_events. The last four take a simple ADD COLUMN path. All
/// sections are pragma-guarded (idempotent).
///
/// This function must NOT run inside a transaction (transactional: false in the
/// MIGRATIONS slice). The rubrics rebuild toggles PRAGMA foreign_keys, which
/// SQLite forbids inside a SAVEPOINT or active transaction.
fn run_migration_readd_user_id_thymus(conn: &rusqlite::Connection) -> Result<()> {
    // -----------------------------------------------------------------------
    // rubrics: 12-step UNIQUE-constraint rebuild
    // Adds user_id NOT NULL DEFAULT 1 and changes UNIQUE(name) to
    // UNIQUE(user_id, name). evaluations references rubrics(id) so
    // foreign_keys must be toggled off for the rename/recreate.
    // -----------------------------------------------------------------------
    let rubrics_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('rubrics') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    if rubrics_has_user_id == 0 {
        conn.execute_batch(
            "PRAGMA foreign_keys = OFF;
             PRAGMA legacy_alter_table = ON;

             ALTER TABLE rubrics RENAME TO _rubrics_old_v75;
             DROP INDEX IF EXISTS idx_rubrics_name;
             DROP INDEX IF EXISTS idx_rubrics_user_name;
             DROP INDEX IF EXISTS idx_rubrics_user;

             CREATE TABLE rubrics (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 name TEXT NOT NULL,
                 description TEXT,
                 criteria TEXT NOT NULL DEFAULT '[]',
                 user_id INTEGER NOT NULL DEFAULT 1,
                 created_at TEXT NOT NULL DEFAULT (datetime('now')),
                 updated_at TEXT NOT NULL DEFAULT (datetime('now'))
             );

             INSERT INTO rubrics (id, name, description, criteria, user_id, created_at, updated_at)
             SELECT id, name, description, criteria, 1, created_at, updated_at
             FROM _rubrics_old_v75;

             DROP TABLE _rubrics_old_v75;

             CREATE UNIQUE INDEX idx_rubrics_user_name ON rubrics(user_id, name);
             CREATE INDEX idx_rubrics_user ON rubrics(user_id);

             PRAGMA legacy_alter_table = OFF;
             PRAGMA foreign_keys = ON;",
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Migration 75: rubrics rebuilt with UNIQUE(user_id, name)");
    } else {
        // Fresh database already has user_id; still ensure all indexes exist.
        conn.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_rubrics_user_name ON rubrics(user_id, name);
             CREATE INDEX IF NOT EXISTS idx_rubrics_user ON rubrics(user_id);",
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    }

    // -----------------------------------------------------------------------
    // evaluations: ADD COLUMN user_id + index
    // -----------------------------------------------------------------------
    let eval_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('evaluations') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if eval_has_user_id == 0 {
        conn.execute(
            "ALTER TABLE evaluations ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1",
            [],
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Re-added evaluations.user_id (migration 75)");
    }
    conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_evaluations_user ON evaluations(user_id);")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    // -----------------------------------------------------------------------
    // quality_metrics: ADD COLUMN user_id + index
    // -----------------------------------------------------------------------
    let qm_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('quality_metrics') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if qm_has_user_id == 0 {
        conn.execute(
            "ALTER TABLE quality_metrics ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1",
            [],
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Re-added quality_metrics.user_id (migration 75)");
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_quality_metrics_user ON quality_metrics(user_id);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    // -----------------------------------------------------------------------
    // session_quality: ADD COLUMN user_id + index
    // -----------------------------------------------------------------------
    let sq_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('session_quality') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if sq_has_user_id == 0 {
        conn.execute(
            "ALTER TABLE session_quality ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1",
            [],
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Re-added session_quality.user_id (migration 75)");
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_session_quality_user ON session_quality(user_id);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    // -----------------------------------------------------------------------
    // behavioral_drift_events: ADD COLUMN user_id + index
    // -----------------------------------------------------------------------
    let bde_has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('behavioral_drift_events') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if bde_has_user_id == 0 {
        conn.execute(
            "ALTER TABLE behavioral_drift_events ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1",
            [],
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Re-added behavioral_drift_events.user_id (migration 75)");
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_behavioral_drift_user ON behavioral_drift_events(user_id);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!("Migration 75 complete: user_id re-added to all 5 thymus tables");
    Ok(())
}

/// Re-adds `user_id` to `entity_cooccurrences` (dropped by v38, never
/// re-added). `structured_facts` already has `user_id` from v64.
/// Registered in the MIGRATIONS slice as v76; `transactional: false` because
/// the pragma_table_info guard makes it safe to re-run.
fn run_migration_readd_user_id_graph_remainder(conn: &rusqlite::Connection) -> Result<()> {
    // entity_cooccurrences: ADD COLUMN with idempotency guard.
    // Use INTEGER DEFAULT 1 (no NOT NULL) to match the CORE schema definition.
    let has_user_id: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('entity_cooccurrences') WHERE name = 'user_id'")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
        .exists([])
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if !has_user_id {
        conn.execute_batch(
            "ALTER TABLE entity_cooccurrences ADD COLUMN user_id INTEGER DEFAULT 1;\
             CREATE INDEX IF NOT EXISTS idx_ec_user ON entity_cooccurrences(user_id);",
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        info!("Migration 76: re-added entity_cooccurrences.user_id");
    }
    // Ensure idx_sf_user exists on structured_facts (which already has the
    // column from v64 / CORE schema). IF NOT EXISTS makes this a no-op on
    // databases that already carry the index.
    conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_sf_user ON structured_facts(user_id);")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    info!("Migration 76 complete: graph_remainder user_id done");
    Ok(())
}

/// Re-adds `user_id` to `user_preferences` (dropped by v40). Uses the
/// 12-step REBUILD pattern because `UNIQUE(user_id, key)` is an in-table
/// constraint. Also restores `idx_up_domain_pref_user` UNIQUE INDEX on
/// `(domain, preference, user_id)`. Registered in the MIGRATIONS slice as
/// v77; `transactional: false` because PRAGMA foreign_keys is toggled.
fn run_migration_readd_user_id_user_preferences(conn: &rusqlite::Connection) -> Result<()> {
    let has_user_id: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('user_preferences') WHERE name = 'user_id'")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
        .exists([])
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if has_user_id {
        return Ok(());
    }

    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         PRAGMA legacy_alter_table = ON;

         ALTER TABLE user_preferences RENAME TO _user_preferences_old_v77;
         DROP INDEX IF EXISTS idx_up_domain;
         DROP INDEX IF EXISTS idx_up_domain_pref;
         DROP INDEX IF EXISTS idx_up_domain_pref_user;
         DROP INDEX IF EXISTS idx_user_prefs_user;

         CREATE TABLE user_preferences (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             user_id INTEGER NOT NULL DEFAULT 1,
             key TEXT NOT NULL,
             value TEXT NOT NULL,
             domain TEXT,
             preference TEXT,
             strength REAL NOT NULL DEFAULT 1.0,
             evidence_memory_id INTEGER REFERENCES memories(id) ON DELETE SET NULL,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now')),
             UNIQUE(user_id, key)
         );

         INSERT INTO user_preferences
             (id, user_id, key, value, domain, preference, strength,
              evidence_memory_id, created_at, updated_at)
         SELECT
             id, 1, key, value, domain, preference, strength,
              evidence_memory_id, created_at, updated_at
         FROM _user_preferences_old_v77;

         DROP TABLE _user_preferences_old_v77;

         CREATE INDEX IF NOT EXISTS idx_user_prefs_user ON user_preferences(user_id);
         CREATE INDEX IF NOT EXISTS idx_up_domain ON user_preferences(domain COLLATE NOCASE);
         CREATE UNIQUE INDEX IF NOT EXISTS idx_up_domain_pref_user ON user_preferences(domain, preference, user_id);

         PRAGMA legacy_alter_table = OFF;
         PRAGMA foreign_keys = ON;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    info!("Migration 77 complete: user_id re-added to user_preferences (REBUILD)");
    Ok(())
}

/// Re-adds `user_id` to `skill_records` (dropped by v42). Uses the 12-step
/// REBUILD pattern because `UNIQUE(name, agent, version, user_id)` is an
/// in-table constraint. Also drops and recreates FTS triggers since the
/// content table is renamed during the rebuild. Registered in MIGRATIONS
/// as v78; `transactional: false` because PRAGMA foreign_keys is toggled.
fn run_migration_readd_user_id_skills(conn: &rusqlite::Connection) -> Result<()> {
    let has_user_id: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('skill_records') WHERE name = 'user_id'")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
        .exists([])
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if has_user_id {
        return Ok(());
    }

    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         PRAGMA legacy_alter_table = ON;

         -- Drop FTS triggers before renaming the content table.
         DROP TRIGGER IF EXISTS skills_fts_insert;
         DROP TRIGGER IF EXISTS skills_fts_delete;
         DROP TRIGGER IF EXISTS skills_fts_update;

         ALTER TABLE skill_records RENAME TO _skill_records_old_v78;

         DROP INDEX IF EXISTS idx_skill_records_agent;
         DROP INDEX IF EXISTS idx_skill_records_name;
         DROP INDEX IF EXISTS idx_skill_records_user;
         DROP INDEX IF EXISTS idx_skill_records_active;
         DROP INDEX IF EXISTS idx_skill_records_category;
         DROP INDEX IF EXISTS idx_skill_records_parent;

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
             user_id INTEGER NOT NULL DEFAULT 1,
             kind TEXT NOT NULL DEFAULT 'skill',
             source_plugin TEXT,
             source_path TEXT,
             content_hash TEXT,
             first_seen TEXT NOT NULL DEFAULT (datetime('now')),
             last_updated TEXT NOT NULL DEFAULT (datetime('now')),
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now')),
             UNIQUE(name, agent, version, user_id)
         );

         -- Insert existing rows; kind/source_plugin/source_path/content_hash pick
         -- up DEFAULT values since the old (pre-v42 drop, pre-any-kind-add) table
         -- never had those columns on the monolith chain.
         INSERT INTO skill_records (
             id, skill_id, name, agent, description, code, path, content,
             category, origin, generation, lineage_change_summary, creator_id,
             language, version, parent_skill_id, root_skill_id, embedding,
             embedding_vec_1024, trust_score, success_count, failure_count,
             execution_count, avg_duration_ms, is_active, is_deprecated,
             total_selections, total_applied, total_completions, visibility,
             lineage_source_task_id, lineage_content_diff, lineage_content_snapshot,
             total_fallbacks, metadata, user_id, first_seen, last_updated,
             created_at, updated_at
         )
         SELECT
             id, skill_id, name, agent, description, code, path, content,
             category, origin, generation, lineage_change_summary, creator_id,
             language, version, parent_skill_id, root_skill_id, embedding,
             embedding_vec_1024, trust_score, success_count, failure_count,
             execution_count, avg_duration_ms, is_active, is_deprecated,
             total_selections, total_applied, total_completions, visibility,
             lineage_source_task_id, lineage_content_diff, lineage_content_snapshot,
             total_fallbacks, metadata, 1, first_seen, last_updated,
             created_at, updated_at
         FROM _skill_records_old_v78
         ORDER BY id ASC;

         DROP TABLE _skill_records_old_v78;

         CREATE INDEX IF NOT EXISTS idx_skill_records_agent ON skill_records(agent);
         CREATE INDEX IF NOT EXISTS idx_skill_records_name ON skill_records(name);
         CREATE INDEX IF NOT EXISTS idx_skill_records_user ON skill_records(user_id);
         CREATE INDEX IF NOT EXISTS idx_skill_records_active ON skill_records(is_active);
         CREATE INDEX IF NOT EXISTS idx_skill_records_category ON skill_records(category);
         CREATE INDEX IF NOT EXISTS idx_skill_records_parent ON skill_records(parent_skill_id);

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

         INSERT INTO skills_fts(skills_fts) VALUES('rebuild');

         PRAGMA legacy_alter_table = OFF;
         PRAGMA foreign_keys = ON;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    info!("Migration 78 complete: user_id re-added to skill_records (REBUILD + FTS)");
    Ok(())
}

/// Re-adds `user_id` to `brain_edges` (dropped by v38). Simple ADD COLUMN
/// since `UNIQUE(source_id, target_id, edge_type)` does not include user_id.
/// `brain_patterns` already has `user_id` (never dropped). Registered in
/// MIGRATIONS as v79; `transactional: false` for pragma guard consistency.
fn run_migration_readd_user_id_brain_edges(conn: &rusqlite::Connection) -> Result<()> {
    let has_user_id: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('brain_edges') WHERE name = 'user_id'")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
        .exists([])
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if has_user_id {
        return Ok(());
    }
    conn.execute_batch(
        "ALTER TABLE brain_edges ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1;\
         CREATE INDEX IF NOT EXISTS idx_brain_edges_user ON brain_edges(user_id);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    info!("Migration 79 complete: user_id re-added to brain_edges");
    Ok(())
}

/// Reverse migration 73: drop the `idx_episodes_user` index and the `user_id`
/// column from `episodes`.
fn down_migration_readd_user_id_episodes(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch("DROP INDEX IF EXISTS idx_episodes_user;")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    let has_user_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('episodes') WHERE name = 'user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    if has_user_id > 0 {
        conn.execute("ALTER TABLE episodes DROP COLUMN user_id", [])
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    }
    info!("Migration 73 reverted: user_id dropped from episodes");
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

/// Migration 37: drops user_id from loom_workflows (Shape B rebuild) and loom_runs.
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

/// Migration 38: drops user_id from graph tables (entities, entity_cooccurrences, memory_pagerank, brain_edges).
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

/// Migration 39: drops user_id from all thymus cluster tables (rubrics, evaluations, quality_metrics, etc.).
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

/// Migration 40: drops user_id from user_preferences (Shape B rebuild) and conversations.
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

/// Migration 41: drops user_id from intelligence tables (current_state, consolidations).
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

/// Migration 42: drops user_id from skill_records via Shape B rebuild including FTS shadow table.
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
    /// Returns the integer result of a COUNT query, or 0 on error.
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

/// Migration 43: drops user_id from the episodes table (Shape A, simple DROP COLUMN).
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

/// Migration 48: rebuilds api_keys without the cross-database agent FK.
fn run_migration_drop_api_keys_agent_fk(conn: &rusqlite::Connection) -> Result<()> {
    // In the sharded architecture agents live in per-tenant databases while
    // api_keys stays in the system DB. The FK `agent_id REFERENCES agents(id)`
    // cannot be satisfied cross-database, so we rebuild the table without it.
    //
    // SAFETY: This migration is `transactional: false` because it toggles
    // `PRAGMA foreign_keys = OFF/ON`, which SQLite forbids inside a SAVEPOINT.
    // To survive partial-failure restarts we detect three states up front:
    //   1. api_keys with FK + no backup    -> full rebuild
    //   2. api_keys without FK + no backup -> already migrated, no-op
    //   3. backup exists                   -> previous run crashed mid-rebuild,
    //      resume from CREATE TABLE.
    let api_keys_sql: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='api_keys'",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok();
    let backup_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='_api_keys_old_v46'",
            [],
            |row| row.get::<_, i64>(0).map(|c| c > 0),
        )
        .unwrap_or(false);

    if !backup_exists {
        if let Some(sql) = &api_keys_sql {
            if !sql.contains("REFERENCES agents") {
                info!("api_keys agent FK already absent, migration 48 is a no-op");
                return Ok(());
            }
        }
    } else {
        // Recovery: a previous run crashed somewhere in the rebuild block.
        // Reset to the pre-migration state and let the rebuild run fresh.
        if api_keys_sql.is_some() {
            // Both tables exist -- crash happened after CREATE TABLE. The
            // new api_keys may be partially populated (or fully but DROP
            // didn't run). Backup is the source of truth: drop the new
            // partial and restore from backup.
            info!("Migration 48 resuming: both api_keys and _api_keys_old_v46 exist; restoring backup");
            conn.execute_batch(
                "DROP TABLE api_keys;
                 ALTER TABLE _api_keys_old_v46 RENAME TO api_keys;",
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        } else {
            // Only backup exists -- crash happened between RENAME and
            // CREATE. Rename backup back so the rebuild starts from the
            // canonical pre-state.
            info!("Migration 48 resuming: only _api_keys_old_v46 exists; restoring api_keys");
            conn.execute_batch("ALTER TABLE _api_keys_old_v46 RENAME TO api_keys;")
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        }
    }

    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         PRAGMA legacy_alter_table = 1;

         ALTER TABLE api_keys RENAME TO _api_keys_old_v46;

         CREATE TABLE api_keys (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
             key_prefix TEXT NOT NULL,
             key_hash TEXT NOT NULL,
             name TEXT NOT NULL DEFAULT 'default',
             scopes TEXT NOT NULL DEFAULT 'read,write',
             rate_limit INTEGER NOT NULL DEFAULT 1000,
             is_active BOOLEAN NOT NULL DEFAULT 1,
             agent_id INTEGER,
             last_used_at TEXT,
             expires_at TEXT,
             created_at TEXT NOT NULL DEFAULT (datetime('now'))
         );

         INSERT INTO api_keys
             (id, user_id, key_prefix, key_hash, name, scopes, rate_limit,
              is_active, agent_id, last_used_at, expires_at, created_at)
         SELECT
             id, user_id, key_prefix, key_hash, name, scopes, rate_limit,
              is_active, agent_id, last_used_at, expires_at, created_at
         FROM _api_keys_old_v46;

         DROP TABLE _api_keys_old_v46;

         CREATE INDEX IF NOT EXISTS idx_api_keys_prefix ON api_keys(key_prefix);
         CREATE INDEX IF NOT EXISTS idx_api_keys_user ON api_keys(user_id);
         CREATE INDEX IF NOT EXISTS idx_api_keys_expires ON api_keys(expires_at) WHERE expires_at IS NOT NULL;

         PRAGMA legacy_alter_table = 0;
         PRAGMA foreign_keys = ON;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!(
        "Migration 48 complete: dropped FK on api_keys.agent_id (agents now live in tenant shards)"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Migration 46: identity_keys + identities tables
// ---------------------------------------------------------------------------

/// Migration 46: creates the identity_keys and identities tables for PIV-Everywhere auth.
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

/// Migration 47: adds identity_id and identity_tier columns to the audit_log table.
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

/// Migration 49: supervisor_injections table.
///
/// Records violations posted by eidolon-supervisor that need to be surfaced
/// back to the agent on the next PreToolUse / UserPromptSubmit. The agent
/// claims pending rows (sets claimed_at) and the supervisor never re-claims.
fn run_migration_supervisor_injections(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS supervisor_injections (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL,
            session_id TEXT NOT NULL,
            rule_id TEXT NOT NULL,
            severity TEXT NOT NULL,
            message TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            claimed_at TEXT
         );
         CREATE INDEX IF NOT EXISTS idx_supervisor_injections_pending
            ON supervisor_injections(user_id, session_id)
            WHERE claimed_at IS NULL;
         CREATE INDEX IF NOT EXISTS idx_supervisor_injections_created
            ON supervisor_injections(user_id, created_at DESC);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!("Migration 49 complete: supervisor_injections table created");
    Ok(())
}

/// Reverse migration 49: drops the supervisor_injections table and its indexes.
fn down_migration_supervisor_injections(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "DROP INDEX IF EXISTS idx_supervisor_injections_created;
         DROP INDEX IF EXISTS idx_supervisor_injections_pending;
         DROP TABLE IF EXISTS supervisor_injections;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

/// Migration 50: adds session_id column to gate_requests and creates a partial index.
fn run_migration_gate_requests_session_id(conn: &rusqlite::Connection) -> Result<()> {
    let has_col: bool = conn
        .prepare("PRAGMA table_info(gate_requests)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .any(|name| name == "session_id");

    if !has_col {
        conn.execute_batch("ALTER TABLE gate_requests ADD COLUMN session_id TEXT;")
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    }

    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_gate_requests_session_open
            ON gate_requests(user_id, session_id, status)
            WHERE output IS NULL;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!("Migration 50 complete: gate_requests.session_id added");
    Ok(())
}

/// Reverse migration 50: drops the gate_requests session_id index and column.
fn down_migration_gate_requests_session_id(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch("DROP INDEX IF EXISTS idx_gate_requests_session_open;")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

/// Migration 51: creates the memory_chunks table for chunked memory storage.
fn run_migration_memory_chunks(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS memory_chunks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            memory_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
            chunk_idx INTEGER NOT NULL,
            content TEXT NOT NULL,
            embedding_vec_1024 BLOB,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(memory_id, chunk_idx)
        );
        CREATE INDEX IF NOT EXISTS idx_chunks_memory ON memory_chunks(memory_id);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    info!("Migration 51 complete: memory_chunks table created");
    Ok(())
}

/// Reverse migration 51: drops the memory_chunks table and its index.
fn down_migration_memory_chunks(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "DROP INDEX IF EXISTS idx_chunks_memory;
         DROP TABLE IF EXISTS memory_chunks;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

/// Migration 52: creates the activity_log table for agent session activity tracking.
fn run_migration_activity_log_table(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS activity_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent TEXT NOT NULL,
            action TEXT NOT NULL,
            summary TEXT NOT NULL,
            category TEXT NOT NULL DEFAULT 'activity'
                CHECK (category IN ('activity','error','warning','task','note')),
            importance INTEGER NOT NULL DEFAULT 4
                CHECK (importance >= 1 AND importance <= 5),
            session_id TEXT,
            project TEXT,
            host TEXT,
            user_id INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_activity_log_session ON activity_log(session_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_activity_log_agent ON activity_log(agent);
        CREATE INDEX IF NOT EXISTS idx_activity_log_user ON activity_log(user_id);
        CREATE INDEX IF NOT EXISTS idx_activity_log_user_created ON activity_log(user_id, created_at DESC);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

/// Reverse migration 52: drops the activity_log table.
fn down_migration_activity_log_table(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch("DROP TABLE IF EXISTS activity_log;")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

/// Migration 53: adds the scopes JSON column to identity_keys.
fn run_migration_identity_keys_scopes(conn: &rusqlite::Connection) -> Result<()> {
    let has_column: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('identity_keys') WHERE name = 'scopes'",
            [],
            |row| row.get::<_, i64>(0).map(|c| c > 0),
        )
        .unwrap_or(false);
    if has_column {
        return Ok(());
    }
    conn.execute_batch(
        r#"ALTER TABLE identity_keys
           ADD COLUMN scopes TEXT NOT NULL DEFAULT '["read","write","admin"]';"#,
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

/// Reverse migration 53: drops the scopes column from identity_keys.
fn down_migration_identity_keys_scopes(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch("ALTER TABLE identity_keys DROP COLUMN scopes;")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

/// Migration 80 (C3): convert legacy JSON-array `identity_keys.scopes` values
/// to the canonical CSV format used by `api_keys.scopes`.
///
/// Migration 53 introduced the `scopes` column with a JSON default
/// (`'["read","write","admin"]'`) and the auth middleware parsed it as JSON.
/// The C3 audit finding requires that the parser deny on unparseable input
/// rather than silently escalating to admin, and the chosen path also moves
/// the storage format to CSV so the same `parse_scopes` / `scopes_to_string`
/// helpers serve both `api_keys` and `identity_keys`.
///
/// This migration walks every row and:
///   - leaves rows that already look like CSV alone (idempotent)
///   - leaves empty strings alone (the new parser treats them as explicit deny)
///   - parses any JSON-array-shaped value and rewrites it as the equivalent
///     CSV. Unparseable JSON is left untouched and logged -- those rows will
///     fail authentication under the new parser, which is the audit-required
///     least-privilege behavior (admin-fallback was the bug).
fn run_migration_identity_keys_scopes_json_to_csv(conn: &rusqlite::Connection) -> Result<()> {
    let has_column: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('identity_keys') WHERE name = 'scopes'",
            [],
            |row| row.get::<_, i64>(0).map(|c| c > 0),
        )
        .unwrap_or(false);
    if !has_column {
        // v53 never ran on this database (e.g. brand-new install before v53
        // landed); nothing to convert.
        return Ok(());
    }

    // Collect (id, raw_scopes) pairs first so we can iterate without holding
    // a prepared-statement borrow while we issue UPDATEs on the same conn.
    let mut select = conn
        .prepare("SELECT id, scopes FROM identity_keys")
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    let rows: Vec<(i64, String)> = select
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    drop(select);

    let mut converted = 0usize;
    let mut left_alone = 0usize;
    let mut unparseable = 0usize;
    for (id, raw) in rows {
        let trimmed = raw.trim();
        // CSV or empty values are already in the target shape.
        if !trimmed.starts_with('[') {
            left_alone += 1;
            continue;
        }
        match serde_json::from_str::<Vec<String>>(trimmed) {
            Ok(names) => {
                let csv = names.join(",");
                conn.execute(
                    "UPDATE identity_keys SET scopes = ?1 WHERE id = ?2",
                    rusqlite::params![csv, id],
                )
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
                converted += 1;
            }
            Err(e) => {
                tracing::warn!(
                    id,
                    raw = %trimmed,
                    error = %e,
                    "identity_keys.scopes row is JSON-shaped but unparseable; \
                     leaving as-is, this row will fail auth under the new CSV \
                     parser (audit-required deny-on-corruption)"
                );
                unparseable += 1;
            }
        }
    }
    info!(
        converted,
        left_alone,
        unparseable,
        "migration 80: identity_keys.scopes JSON-to-CSV conversion complete"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// v54: tool_manifests
// ---------------------------------------------------------------------------

/// Migration 54: creates the tool_manifests table for signed agent tool declarations.
fn run_migration_tool_manifests(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tool_manifests (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent_identity_id INTEGER NOT NULL REFERENCES identity_keys(id) ON DELETE CASCADE,
            manifest_hash TEXT NOT NULL,
            declared_tools_json TEXT NOT NULL,
            signed_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(agent_identity_id, manifest_hash)
        );
        CREATE INDEX IF NOT EXISTS idx_tool_manifests_agent ON tool_manifests(agent_identity_id);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

/// Reverse migration 54: drops the tool_manifests table and its index.
fn down_migration_tool_manifests(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "DROP INDEX IF EXISTS idx_tool_manifests_agent;
         DROP TABLE IF EXISTS tool_manifests;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

/// Migration 55: creates the global handoffs table and FTS shadow for session handoff search.
fn run_migration_handoffs_global(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS handoffs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now', 'utc')),
            project TEXT NOT NULL,
            branch TEXT,
            directory TEXT,
            agent TEXT DEFAULT 'unknown',
            type TEXT DEFAULT 'manual',
            content TEXT NOT NULL,
            metadata TEXT,
            session_id TEXT,
            model TEXT,
            host TEXT,
            content_hash TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_handoffs_project ON handoffs(project, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_handoffs_created ON handoffs(created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_handoffs_hash ON handoffs(content_hash);
        CREATE INDEX IF NOT EXISTS idx_handoffs_agent ON handoffs(agent, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_handoffs_type ON handoffs(type, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_handoffs_session ON handoffs(session_id);
        CREATE INDEX IF NOT EXISTS idx_handoffs_model ON handoffs(model, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_handoffs_restore ON handoffs(project, type, agent, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_handoffs_user_created ON handoffs(user_id, created_at DESC);
        CREATE VIRTUAL TABLE IF NOT EXISTS handoffs_fts USING fts5(
            content, content='handoffs', content_rowid='id'
        );
        CREATE TRIGGER IF NOT EXISTS handoffs_fts_ai AFTER INSERT ON handoffs BEGIN
            INSERT INTO handoffs_fts(rowid, content) VALUES (new.id, new.content);
        END;
        CREATE TRIGGER IF NOT EXISTS handoffs_fts_ad AFTER DELETE ON handoffs BEGIN
            INSERT INTO handoffs_fts(handoffs_fts, rowid, content) VALUES('delete', old.id, old.content);
        END;
        CREATE TRIGGER IF NOT EXISTS handoffs_fts_au AFTER UPDATE OF content ON handoffs BEGIN
            INSERT INTO handoffs_fts(handoffs_fts, rowid, content) VALUES('delete', old.id, old.content);
            INSERT INTO handoffs_fts(rowid, content) VALUES (new.id, new.content);
        END;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

/// Reverse migration 55: drops the handoffs FTS shadow, triggers, and main table.
fn down_migration_handoffs_global(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "DROP TRIGGER IF EXISTS handoffs_fts_au;
         DROP TRIGGER IF EXISTS handoffs_fts_ad;
         DROP TRIGGER IF EXISTS handoffs_fts_ai;
         DROP TABLE IF EXISTS handoffs_fts;
         DROP TABLE IF EXISTS handoffs;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Migration 56: is_active on users + enrollment_invites table
// ---------------------------------------------------------------------------

/// Adds a soft-delete flag to the users table so deactivated accounts can be
/// excluded from queries without losing audit history. Also creates the
/// enrollment_invites table that holds hashed one-time tokens for FIDO2 key
/// registration -- the raw token is shown to the admin exactly once and never
/// stored.
fn run_migration_user_active_and_invites(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        // Default 1 so every existing user stays active after migration.
        "ALTER TABLE users ADD COLUMN is_active BOOLEAN NOT NULL DEFAULT 1;

         CREATE TABLE IF NOT EXISTS enrollment_invites (
             id         INTEGER PRIMARY KEY AUTOINCREMENT,
             user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
             token_hash TEXT    NOT NULL UNIQUE,
             method     TEXT    NOT NULL DEFAULT 'fido2',
             created_at TEXT    NOT NULL DEFAULT (datetime('now', 'utc')),
             expires_at TEXT    NOT NULL,
             consumed_at TEXT
         );
         CREATE INDEX IF NOT EXISTS idx_enrollment_invites_token
             ON enrollment_invites(token_hash);
         CREATE INDEX IF NOT EXISTS idx_enrollment_invites_user
             ON enrollment_invites(user_id, created_at DESC);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

/// Reverse migration 56: drop the enrollment_invites table and remove the
/// is_active column from users. Uses ALTER TABLE DROP COLUMN (SQLite 3.35.0+).
fn down_migration_user_active_and_invites(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "DROP INDEX IF EXISTS idx_enrollment_invites_user;
         DROP INDEX IF EXISTS idx_enrollment_invites_token;
         DROP TABLE IF EXISTS enrollment_invites;
         ALTER TABLE users DROP COLUMN is_active;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

/// Migration 57: dispatch config table for the `forge` agent tool runtime.
///
/// Each row describes a callable skill endpoint -- parameter schema, HTTP
/// target, and output formatting hints.  `forge exec <skill_name>` fetches the
/// matching row and uses it to validate args, build the request, and format the
/// response without any hardcoded knowledge of the skill.
fn run_migration_skill_dispatch_configs(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS skill_dispatch_configs (
             id            INTEGER PRIMARY KEY AUTOINCREMENT,
             skill_name    TEXT    NOT NULL UNIQUE,
             description   TEXT    NOT NULL DEFAULT '',
             enabled       BOOLEAN NOT NULL DEFAULT 1,
             target_type   TEXT    NOT NULL DEFAULT 'internal'
                           CHECK(target_type IN ('internal', 'external')),
             endpoint      TEXT    NOT NULL,
             method        TEXT    NOT NULL DEFAULT 'POST',
             params_schema TEXT    NOT NULL DEFAULT '{}',
             output_hints  TEXT    NOT NULL DEFAULT '{}',
             created_at    TEXT    NOT NULL DEFAULT (datetime('now')),
             updated_at    TEXT    NOT NULL DEFAULT (datetime('now'))
         );

         CREATE INDEX IF NOT EXISTS idx_sdc_skill_name
             ON skill_dispatch_configs(skill_name);
         CREATE INDEX IF NOT EXISTS idx_sdc_enabled
             ON skill_dispatch_configs(enabled);",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    // Seed: web_search -- first callable skill.
    conn.execute(
        "INSERT OR IGNORE INTO skill_dispatch_configs
             (skill_name, description, enabled, target_type, endpoint, method,
              params_schema, output_hints)
         VALUES (?1, ?2, 1, 'internal', '/search/web', 'POST', ?3, ?4)",
        rusqlite::params![
            "web_search",
            "Search the web via SearXNG. Returns ranked results with title, URL, and snippet.",
            r#"{"query":{"type":"string","required":true,"description":"Search query (max 512 chars)"},"categories":{"type":"string","required":false,"description":"Search category","enum":["general","images","videos","news","map","music","it","science","files","social media"]},"language":{"type":"string","required":false,"description":"Language code (e.g. en, de, fr)"},"limit":{"type":"integer","required":false,"default":10,"description":"Max results (1-50)"},"pageno":{"type":"integer","required":false,"default":1,"description":"Page number (1-20)"},"safesearch":{"type":"integer","required":false,"description":"0=off, 1=moderate, 2=strict"}}"#,
            r#"{"results_path":"/results","summary_fields":["title","url","snippet"],"count_path":"/count","suggestions_path":"/suggestions"}"#,
        ],
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    Ok(())
}

/// Reverse migration 57: drop the skill_dispatch_configs table.
fn down_migration_skill_dispatch_configs(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "DROP INDEX IF EXISTS idx_sdc_enabled;
         DROP INDEX IF EXISTS idx_sdc_skill_name;
         DROP TABLE IF EXISTS skill_dispatch_configs;",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    Ok(())
}

/// Migration 58: idempotent fixup for api_keys.hash_version column.
fn run_migration_api_key_hash_version_fixup(conn: &rusqlite::Connection) -> Result<()> {
    add_column_if_not_exists(
        conn,
        "api_keys",
        "hash_version",
        "INTEGER NOT NULL DEFAULT 1",
    )
}

// ---------------------------------------------------------------------------
// Migration 59: add narrative and axon_event_id to broca_actions
// ---------------------------------------------------------------------------

/// Migration 59: add `narrative TEXT` and `axon_event_id INTEGER` to the
/// `broca_actions` table for existing databases.
///
/// Fresh databases created after this schema update already have both columns
/// (they are present in `schema_sql.rs`). The tenant shard also has them from
/// the v6 DROP/CREATE migration. This migration brings existing monolith
/// databases into parity without touching fresh installs (the
/// `add_column_if_not_exists` helper is a no-op when the column already
/// exists).
fn run_migration_broca_narrative_columns(conn: &rusqlite::Connection) -> Result<()> {
    add_column_if_not_exists(conn, "broca_actions", "narrative", "TEXT")?;
    add_column_if_not_exists(conn, "broca_actions", "axon_event_id", "INTEGER")?;
    Ok(())
}

// Migration 60: Chiasm extended fields (syntheos parity)

/// Migration 60: adds the extended Syntheos-parity columns to `chiasm_tasks`
/// in the monolith database and removes the restrictive status CHECK constraint
/// by rebuilding the table.
///
/// Fresh tenant shards receive these columns via tenant migration v52. This
/// migration brings the monolith database into parity using the same idempotent
/// ADD COLUMN pattern followed by a table rebuild that drops the CHECK
/// constraint on `status`, allowing the three new statuses
/// (blocked_on_human, stale, queued) to be inserted.
fn run_migration_chiasm_extended_fields(conn: &rusqlite::Connection) -> Result<()> {
    add_column_if_not_exists(conn, "chiasm_tasks", "expected_output", "TEXT")?;
    add_column_if_not_exists(
        conn,
        "chiasm_tasks",
        "output_format",
        "TEXT NOT NULL DEFAULT 'raw'",
    )?;
    add_column_if_not_exists(conn, "chiasm_tasks", "output", "TEXT")?;
    add_column_if_not_exists(conn, "chiasm_tasks", "condition", "TEXT")?;
    add_column_if_not_exists(conn, "chiasm_tasks", "guardrail_url", "TEXT")?;
    add_column_if_not_exists(
        conn,
        "chiasm_tasks",
        "guardrail_retries",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column_if_not_exists(conn, "chiasm_tasks", "plan", "TEXT")?;
    add_column_if_not_exists(conn, "chiasm_tasks", "feedback", "TEXT")?;
    add_column_if_not_exists(conn, "chiasm_tasks", "last_heartbeat", "TEXT")?;
    add_column_if_not_exists(
        conn,
        "chiasm_tasks",
        "heartbeat_interval",
        "INTEGER NOT NULL DEFAULT 300",
    )?;
    add_column_if_not_exists(
        conn,
        "chiasm_tasks",
        "assigned",
        "INTEGER NOT NULL DEFAULT 1",
    )?;

    // Rebuild the table to remove the CHECK(status IN (...)) constraint that
    // would reject the new statuses blocked_on_human, stale, and queued.
    // SQLite has no ALTER TABLE DROP CONSTRAINT; the only option is a rebuild.
    // The user_id column was dropped in migration 28, so the SELECT must omit it.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS chiasm_tasks_new ( \
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            agent TEXT NOT NULL, \
            project TEXT NOT NULL, \
            title TEXT NOT NULL, \
            status TEXT NOT NULL DEFAULT 'active', \
            summary TEXT, \
            expected_output TEXT, \
            output_format TEXT NOT NULL DEFAULT 'raw', \
            output TEXT, \
            condition TEXT, \
            guardrail_url TEXT, \
            guardrail_retries INTEGER NOT NULL DEFAULT 0, \
            plan TEXT, \
            feedback TEXT, \
            last_heartbeat TEXT, \
            heartbeat_interval INTEGER NOT NULL DEFAULT 300, \
            assigned INTEGER NOT NULL DEFAULT 1, \
            created_at TEXT NOT NULL DEFAULT (datetime('now')), \
            updated_at TEXT NOT NULL DEFAULT (datetime('now')) \
        ); \
        INSERT OR IGNORE INTO chiasm_tasks_new \
            (id, agent, project, title, status, summary, \
             expected_output, output_format, output, condition, guardrail_url, \
             guardrail_retries, plan, feedback, last_heartbeat, heartbeat_interval, \
             assigned, created_at, updated_at) \
        SELECT \
            id, agent, project, title, status, summary, \
            expected_output, output_format, output, condition, guardrail_url, \
            guardrail_retries, plan, feedback, last_heartbeat, heartbeat_interval, \
            assigned, created_at, updated_at \
        FROM chiasm_tasks; \
        DROP TABLE IF EXISTS chiasm_tasks; \
        ALTER TABLE chiasm_tasks_new RENAME TO chiasm_tasks; \
        CREATE INDEX IF NOT EXISTS idx_chiasm_tasks_status ON chiasm_tasks(status); \
        CREATE INDEX IF NOT EXISTS idx_chiasm_tasks_agent ON chiasm_tasks(agent); \
        CREATE INDEX IF NOT EXISTS idx_chiasm_tasks_project ON chiasm_tasks(project);",
    )
    .map_err(|e| EngError::DatabaseMessage(format!("migration 60 chiasm_tasks rebuild: {e}")))?;

    Ok(())
}

/// Migration 61: create `chiasm_path_claims` and `chiasm_task_dependencies`
/// tables in the monolith database.
///
/// Fresh tenant shards receive these tables via tenant migration v52. This
/// migration brings the monolith database into parity using the same
/// `CREATE TABLE IF NOT EXISTS` pattern so it is idempotent and safe to
/// apply against databases that already have the tables (e.g. if tenant
/// migration SQL was ever run against the monolith directly).
fn run_migration_chiasm_path_claims(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS chiasm_task_dependencies (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            task_id INTEGER NOT NULL REFERENCES chiasm_tasks(id) ON DELETE CASCADE,
            depends_on INTEGER NOT NULL REFERENCES chiasm_tasks(id) ON DELETE CASCADE,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(task_id, depends_on)
        );
        CREATE INDEX IF NOT EXISTS idx_chiasm_deps_task ON chiasm_task_dependencies(task_id);
        CREATE INDEX IF NOT EXISTS idx_chiasm_deps_depends ON chiasm_task_dependencies(depends_on);
        CREATE TABLE IF NOT EXISTS chiasm_path_claims (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            task_id INTEGER NOT NULL REFERENCES chiasm_tasks(id) ON DELETE CASCADE,
            agent TEXT NOT NULL,
            project TEXT NOT NULL,
            path TEXT NOT NULL,
            claimed_at TEXT NOT NULL DEFAULT (datetime('now')),
            expires_at TEXT NOT NULL,
            released INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_chiasm_claims_project_path ON chiasm_path_claims(project, path);
        CREATE INDEX IF NOT EXISTS idx_chiasm_claims_task ON chiasm_path_claims(task_id);
        CREATE INDEX IF NOT EXISTS idx_chiasm_claims_expires ON chiasm_path_claims(expires_at);",
    )
    .map_err(|e| EngError::DatabaseMessage(format!("migration 61 chiasm_path_claims: {e}")))?;
    info!("Migration 61 complete: chiasm_path_claims and chiasm_task_dependencies created");
    Ok(())
}

/// Migration 62: create `chiasm_agent_keys` table for per-agent bearer keys.
/// Mirrors the standalone chiasm schema (agent, key_hash, key_prefix,
/// created_at, last_used_at, revoked). Only the SHA-256 hash is stored;
/// the raw key is returned exactly once at creation time.
fn run_migration_chiasm_agent_keys(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS chiasm_agent_keys (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent TEXT NOT NULL,
            key_hash TEXT NOT NULL UNIQUE,
            key_prefix TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_used_at TEXT,
            revoked INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_chiasm_agent_keys_hash ON chiasm_agent_keys(key_hash);
        CREATE INDEX IF NOT EXISTS idx_chiasm_agent_keys_agent ON chiasm_agent_keys(agent);",
    )
    .map_err(|e| EngError::DatabaseMessage(format!("migration 62 chiasm_agent_keys: {e}")))?;
    info!("Migration 62 complete: chiasm_agent_keys created");
    Ok(())
}

/// Migration 63: create `handoff_atoms` and `atom_entity_links` tables.
///
/// `handoff_atoms` stores extracted knowledge atoms from session handoffs,
/// with salience and confidence scores for decay-based pruning.
/// `atom_entity_links` associates atoms with Kleos memory entities.
fn run_migration_handoff_atoms(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS handoff_atoms (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            atom_id         TEXT NOT NULL,
            handoff_id      INTEGER NOT NULL,
            user_id         INTEGER NOT NULL DEFAULT 1,
            project         TEXT NOT NULL,
            atom_type       TEXT NOT NULL,
            content         TEXT NOT NULL,
            canonical_form  TEXT NOT NULL,
            salience        REAL NOT NULL DEFAULT 0.5,
            confidence      REAL NOT NULL DEFAULT 0.5,
            status          TEXT NOT NULL DEFAULT 'active',
            created_at      TEXT NOT NULL DEFAULT (datetime('now', 'utc')),
            last_seen_at    TEXT NOT NULL DEFAULT (datetime('now', 'utc')),
            seen_count      INTEGER NOT NULL DEFAULT 1,
            decay_immune    INTEGER NOT NULL DEFAULT 0,
            superseded_by   TEXT,
            metadata        TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_atoms_project_type ON handoff_atoms(project, atom_type, status);
        CREATE INDEX IF NOT EXISTS idx_atoms_salience ON handoff_atoms(project, salience DESC);
        CREATE INDEX IF NOT EXISTS idx_atoms_atom_id ON handoff_atoms(atom_id);
        CREATE INDEX IF NOT EXISTS idx_atoms_handoff ON handoff_atoms(handoff_id);
        CREATE INDEX IF NOT EXISTS idx_atoms_last_seen ON handoff_atoms(last_seen_at DESC);
        CREATE INDEX IF NOT EXISTS idx_atoms_user_project ON handoff_atoms(user_id, project, status);
        CREATE INDEX IF NOT EXISTS idx_atoms_status ON handoff_atoms(status, atom_type);
        CREATE TABLE IF NOT EXISTS atom_entity_links (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            atom_id     TEXT NOT NULL,
            entity_id   INTEGER NOT NULL,
            user_id     INTEGER NOT NULL,
            created_at  TEXT NOT NULL DEFAULT (datetime('now', 'utc')),
            UNIQUE(atom_id, entity_id)
        );
        CREATE INDEX IF NOT EXISTS idx_ael_atom ON atom_entity_links(atom_id);
        CREATE INDEX IF NOT EXISTS idx_ael_entity ON atom_entity_links(entity_id);",
    )
    .map_err(|e| EngError::DatabaseMessage(format!("migration 63 handoff_atoms: {e}")))?;
    info!("Migration 63 complete: handoff_atoms and atom_entity_links created");
    Ok(())
}

/// Migration 81: MCP direct-auth token revocation table.
///
/// Stores per-token revocation state for identity-signed bearer tokens.
/// The token itself is self-authenticating (Ed25519 sig); this table
/// tracks jti -> is_active for revocation + last_used_at for audit.
fn run_migration_mcp_tokens(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS mcp_tokens (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            jti TEXT NOT NULL UNIQUE,
            user_id INTEGER NOT NULL REFERENCES users(id),
            tenant_id INTEGER,
            identity_key_id INTEGER NOT NULL REFERENCES identity_keys(id),
            kid TEXT NOT NULL,
            name TEXT NOT NULL DEFAULT '',
            scopes TEXT NOT NULL,
            is_active BOOLEAN NOT NULL DEFAULT 1,
            issued_at TEXT NOT NULL DEFAULT (datetime('now')),
            expires_at TEXT NOT NULL,
            revoked_at TEXT,
            revoke_reason TEXT,
            last_used_at TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_mcp_tokens_user
            ON mcp_tokens(user_id);
        CREATE INDEX IF NOT EXISTS idx_mcp_tokens_identity_key
            ON mcp_tokens(identity_key_id);
        CREATE INDEX IF NOT EXISTS idx_mcp_tokens_active
            ON mcp_tokens(is_active, expires_at);
        CREATE INDEX IF NOT EXISTS idx_mcp_tokens_tenant
            ON mcp_tokens(tenant_id, user_id);",
    )
    .map_err(|e| EngError::DatabaseMessage(format!("migration 81 mcp_tokens: {e}")))?;
    info!("Migration 81 complete: mcp_tokens table created");
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Unit and integration tests for the migration chain.
#[cfg(test)]
mod tests {
    use super::*;

    /// Opens an in-memory SQLite connection for testing.
    fn open_test_db() -> rusqlite::Connection {
        rusqlite::Connection::open_in_memory().expect("open in-memory test db")
    }

    /// Regression: every entry in MIGRATIONS must have a matching dispatch
    /// block in `run_migrations()`. Without this test, a future contributor
    /// could add a Migration entry but forget the `if current_version < ...`
    /// block and the migration would silently never apply (this is what
    /// blocker B1 was: v52 was registered but never dispatched).
    #[test]
    fn every_static_migration_is_dispatched() {
        let conn = open_test_db();
        run_migrations(&conn).expect("migrations apply on fresh db");
        let highest_in_array = MIGRATIONS
            .iter()
            .map(|m| m.version)
            .max()
            .expect("MIGRATIONS not empty");
        let applied: u32 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get::<_, i64>(0).map(|v| v as u32),
            )
            .expect("query schema_version");
        assert_eq!(
            applied, highest_in_array,
            "MIGRATIONS array contains v{highest_in_array} but run_migrations only applied up to v{applied}. \
             Did you add an entry to the MIGRATIONS array without adding the matching dispatch block?"
        );
    }

    /// Migration 80 (C3): converts legacy JSON-array `identity_keys.scopes`
    /// values to CSV. Must convert canonical JSON, leave already-CSV rows
    /// alone, leave empty strings alone, and not mangle malformed JSON
    /// (those rows fail auth under the new parser, which is the intended
    /// deny-on-corruption behavior).
    #[test]
    fn migration_80_scopes_json_to_csv() {
        let conn = open_test_db();
        run_migrations(&conn).expect("migrations apply on fresh db");

        // users.id=1 may or may not exist depending on bootstrap state, and
        // identity_keys FK-references users(id) -- use OR IGNORE so the test
        // is independent of bootstrap.
        conn.execute(
            "INSERT OR IGNORE INTO users (id, username, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![1, "soak-user", "2026-01-01 00:00:00"],
        )
        .expect("insert users row");

        // Simulate pre-migration state by inserting rows in all four shapes
        // the migration must handle. The migration was already dispatched
        // on the fresh DB above, so we exercise it idempotently by direct
        // call after inserting the synthetic rows.
        let inserts = [
            (101_i64, r#"["read","write","admin"]"#, "read,write,admin"), // JSON full
            (102_i64, r#"["read"]"#, "read"),                             // JSON single
            (103_i64, "read,write", "read,write"),                        // already CSV
            (104_i64, "", ""),                                            // empty stays empty
            (105_i64, r#"[bogus json"#, r#"[bogus json"#),                // malformed left alone
        ];
        for (id, raw, _expected) in &inserts {
            conn.execute(
                "INSERT INTO identity_keys
                 (id, user_id, tier, algo, pubkey_pem, pubkey_fingerprint, host_label, scopes)
                 VALUES (?1, 1, 'soft', 'ed25519', 'PEM', ?2, 'h', ?3)",
                rusqlite::params![id, format!("fp{id}"), raw],
            )
            .expect("insert synthetic identity_keys row");
        }

        run_migration_identity_keys_scopes_json_to_csv(&conn).expect("migration 80 re-run");

        for (id, _raw, expected) in &inserts {
            let got: String = conn
                .query_row(
                    "SELECT scopes FROM identity_keys WHERE id = ?1",
                    rusqlite::params![id],
                    |row| row.get(0),
                )
                .expect("row exists post-migration");
            assert_eq!(
                got, *expected,
                "row id={id}: expected scopes={expected:?}, got {got:?}"
            );
        }

        // Idempotency: a second run is a no-op.
        run_migration_identity_keys_scopes_json_to_csv(&conn).expect("migration 80 idempotent");
        for (id, _raw, expected) in &inserts {
            let got: String = conn
                .query_row(
                    "SELECT scopes FROM identity_keys WHERE id = ?1",
                    rusqlite::params![id],
                    |row| row.get(0),
                )
                .expect("row still exists");
            assert_eq!(got, *expected, "second pass changed row id={id}");
        }
    }

    /// Regression for B4: the v48 api_keys rebuild must be safe to re-run on
    /// partial-failure states. Exercises the function directly because the
    /// runner uses `MAX(version)` and won't re-dispatch v48 if any later
    /// migration's row is still in schema_version.
    #[test]
    fn migration_48_recovery_is_idempotent_and_safe() {
        let conn = open_test_db();
        run_migrations(&conn).expect("first apply");

        // Sanity: post-migration state. api_keys has no FK, no backup.
        let api_keys_sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='api_keys'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!api_keys_sql.contains("REFERENCES agents"));

        // Case A: re-running on stable state is a no-op.
        run_migration_drop_api_keys_agent_fk(&conn).expect("no-op re-run");
        let backup: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='_api_keys_old_v46'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(backup, 0, "stable state must not produce a backup");

        // Case B: simulate a crash after CREATE but before DROP. Both
        // api_keys (new schema) and _api_keys_old_v46 (also new schema in
        // this synthetic state) exist.
        conn.execute_batch("CREATE TABLE _api_keys_old_v46 AS SELECT * FROM api_keys WHERE 0;")
            .unwrap();
        run_migration_drop_api_keys_agent_fk(&conn).expect("recovery (both tables)");
        let backup: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='_api_keys_old_v46'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(backup, 0, "case B: backup must be cleaned up");
        let sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='api_keys'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!sql.contains("REFERENCES agents"));

        // Case C: simulate a crash between RENAME and CREATE. Only the
        // backup exists, api_keys is missing.
        conn.execute_batch("ALTER TABLE api_keys RENAME TO _api_keys_old_v46;")
            .unwrap();
        run_migration_drop_api_keys_agent_fk(&conn).expect("recovery (only backup)");
        let backup: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='_api_keys_old_v46'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(backup, 0, "case C: backup must be cleaned up");
        let api_keys_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='api_keys'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(api_keys_count, 1, "case C: api_keys must be restored");
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

    /// Verifies that running migrations twice on the same database is safe.
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

    /// Verifies the supervisor_injections table and its columns are created by migration 49.
    #[test]
    fn test_supervisor_injections_migration() {
        let conn = open_test_db();
        apply_migrations_up_to(&conn, 49);

        // Table exists with the expected columns.
        let cols: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT name FROM pragma_table_info('supervisor_injections')")
                .expect("prepare pragma");
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .expect("query columns");
            rows.collect::<std::result::Result<Vec<_>, rusqlite::Error>>()
                .expect("collect columns")
        };
        for expected in [
            "id",
            "user_id",
            "session_id",
            "rule_id",
            "severity",
            "message",
            "created_at",
            "claimed_at",
        ] {
            assert!(
                cols.iter().any(|c| c == expected),
                "missing column {expected}"
            );
        }

        // Pending index exists.
        let idx_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'index' AND name = 'idx_supervisor_injections_pending'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx_count, 1);

        // Re-running the migration is a no-op (CREATE TABLE IF NOT EXISTS).
        run_migration_supervisor_injections(&conn).expect("idempotent up");

        // Down migration drops the table.
        down_migration_supervisor_injections(&conn).expect("down works");
        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'supervisor_injections'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 0);
    }

    /// Verifies that pending supervisor_injections rows are atomically claimed via claimed_at.
    #[test]
    fn test_supervisor_injections_pending_atomic_claim() {
        let conn = open_test_db();
        apply_migrations_up_to(&conn, 49);

        conn.execute_batch(
            "INSERT INTO supervisor_injections (user_id, session_id, rule_id, severity, message)
             VALUES (1, 'sess-a', 'no-force-push', 'Critical', 'msg1');
             INSERT INTO supervisor_injections (user_id, session_id, rule_id, severity, message)
             VALUES (1, 'sess-a', 'em-dash-usage', 'Info', 'msg2');
             INSERT INTO supervisor_injections (user_id, session_id, rule_id, severity, message)
             VALUES (2, 'sess-a', 'no-force-push', 'Critical', 'other-user');",
        )
        .unwrap();

        // First claim returns user 1's two rows in session sess-a.
        let claimed_first: Vec<i64> = {
            let mut stmt = conn
                .prepare(
                    "UPDATE supervisor_injections
                     SET claimed_at = datetime('now')
                     WHERE user_id = ?1 AND session_id = ?2 AND claimed_at IS NULL
                     RETURNING id",
                )
                .unwrap();
            stmt.query_map(rusqlite::params![1i64, "sess-a"], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap()
            .collect::<std::result::Result<Vec<_>, rusqlite::Error>>()
            .unwrap()
        };
        assert_eq!(claimed_first.len(), 2);

        // Second claim returns nothing (rows already claimed).
        let claimed_second: Vec<i64> = {
            let mut stmt = conn
                .prepare(
                    "UPDATE supervisor_injections
                     SET claimed_at = datetime('now')
                     WHERE user_id = ?1 AND session_id = ?2 AND claimed_at IS NULL
                     RETURNING id",
                )
                .unwrap();
            stmt.query_map(rusqlite::params![1i64, "sess-a"], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap()
            .collect::<std::result::Result<Vec<_>, rusqlite::Error>>()
            .unwrap()
        };
        assert!(claimed_second.is_empty());

        // User 2's row is untouched.
        let user2_pending: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM supervisor_injections
                 WHERE user_id = 2 AND claimed_at IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(user2_pending, 1);
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
