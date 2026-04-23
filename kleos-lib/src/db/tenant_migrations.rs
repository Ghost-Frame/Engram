//! Tenant-database migration chain.
//!
//! Each tenant shard has its own independent migration version tracked in the
//! `schema_migrations` table inside the tenant's own SQLite file. The version
//! sequence here is NOT related to the system/main migration sequence in
//! `super::migrations` -- system and tenant schemas evolve on separate
//! timelines.
//!
//! Migrations run lazily on tenant load (via `Database::open_tenant`). A new
//! tenant gets v1 applied; an existing tenant at v1 gets nothing until a new
//! version is appended to `TENANT_MIGRATIONS`.

use crate::{EngError, Result};
use rusqlite::Connection;
use tracing::info;

/// A single tenant-schema migration.
pub struct TenantMigration {
    pub version: i64,
    pub description: &'static str,
    pub up: fn(&Connection) -> Result<()>,
}

/// The canonical ordered list of tenant migrations.
///
/// Append-only. Never renumber, never edit a past entry.
pub static TENANT_MIGRATIONS: &[TenantMigration] = &[
    TenantMigration {
        version: 1,
        description: "initial_tenant_schema",
        up: apply_schema_v1,
    },
    TenantMigration {
        version: 2,
        description: "scratchpad_user_id_shim",
        up: apply_schema_v2_scratchpad_shim,
    },
    TenantMigration {
        version: 3,
        description: "sessions_user_id_shim",
        up: apply_schema_v3_sessions_shim,
    },
    TenantMigration {
        version: 4,
        description: "chiasm_tasks_shim",
        up: apply_schema_v4_chiasm_shim,
    },
    TenantMigration {
        version: 5,
        description: "approvals_shim",
        up: apply_schema_v5_approvals_shim,
    },
    TenantMigration {
        version: 6,
        description: "broca_actions_shim",
        up: apply_schema_v6_broca_shim,
    },
    TenantMigration {
        version: 7,
        description: "projects_shim",
        up: apply_schema_v7_projects_shim,
    },
    TenantMigration {
        version: 8,
        description: "axon_events_and_soma_agents_shim",
        up: apply_schema_v8_activity_shim,
    },
    TenantMigration {
        version: 9,
        description: "webhooks_shim",
        up: apply_schema_v9_webhooks_shim,
    },
    TenantMigration {
        version: 10,
        description: "ingestion_shim",
        up: apply_schema_v10_ingestion_shim,
    },
    TenantMigration {
        version: 11,
        description: "axon_family_shim",
        up: apply_schema_v11_axon_shim,
    },
    TenantMigration {
        version: 12,
        description: "soma_family_shim",
        up: apply_schema_v12_soma_shim,
    },
    TenantMigration {
        version: 13,
        description: "loom_family_shim",
        up: apply_schema_v13_loom_shim,
    },
    TenantMigration {
        version: 14,
        description: "graph_family_shim",
        up: apply_schema_v14_graph_shim,
    },
    TenantMigration {
        version: 15,
        description: "thymus_family_shim",
        up: apply_schema_v15_thymus_shim,
    },
    TenantMigration {
        version: 16,
        description: "portability_family_shim",
        up: apply_schema_v16_portability_shim,
    },
];

fn apply_schema_v1(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v1.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v1 failed: {e}")))
}

fn apply_schema_v2_scratchpad_shim(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v2_scratchpad.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v2 failed: {e}")))
}

fn apply_schema_v3_sessions_shim(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v3_sessions.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v3 failed: {e}")))
}

fn apply_schema_v4_chiasm_shim(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v4_chiasm.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v4 failed: {e}")))
}

fn apply_schema_v5_approvals_shim(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v5_approvals.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v5 failed: {e}")))
}

fn apply_schema_v6_broca_shim(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v6_broca.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v6 failed: {e}")))
}

fn apply_schema_v7_projects_shim(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v7_projects.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v7 failed: {e}")))
}

fn apply_schema_v8_activity_shim(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v8_activity.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v8 failed: {e}")))
}

fn apply_schema_v9_webhooks_shim(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v9_webhooks.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v9 failed: {e}")))
}

fn apply_schema_v10_ingestion_shim(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v10_ingestion.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v10 failed: {e}")))
}

fn apply_schema_v11_axon_shim(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v11_axon.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v11 failed: {e}")))
}

fn apply_schema_v12_soma_shim(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v12_soma.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v12 failed: {e}")))
}

fn apply_schema_v13_loom_shim(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v13_loom.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v13 failed: {e}")))
}

fn apply_schema_v14_graph_shim(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v14_graph.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v14 failed: {e}")))
}

fn apply_schema_v15_thymus_shim(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v15_thymus.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v15 failed: {e}")))
}

fn apply_schema_v16_portability_shim(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v16_portability.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v16 failed: {e}")))
}

/// Run all pending tenant migrations against `conn`.
///
/// Idempotent: safe to call on every tenant load. A freshly created tenant
/// database lands at the latest version; an existing one catches up.
pub fn run_tenant_migrations(conn: &Connection) -> Result<()> {
    // Tenant schema uses the `schema_migrations` table (as defined in v1).
    // Ensure it exists so we can read current_version even before v1 runs.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    let current: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    for m in TENANT_MIGRATIONS.iter() {
        if m.version <= current {
            continue;
        }
        info!(
            "applying tenant migration {} ({})",
            m.version, m.description
        );
        (m.up)(conn)?;
        conn.execute(
            "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
            rusqlite::params![m.version],
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    }

    Ok(())
}

/// Latest declared tenant schema version.
pub fn latest_version() -> i64 {
    TENANT_MIGRATIONS
        .iter()
        .map(|m| m.version)
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_db_lands_at_latest() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        let v: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, latest_version());
    }

    #[test]
    fn idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();
        run_tenant_migrations(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn memories_table_exists_after_v1() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='memories'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1);
    }

    #[test]
    fn scratchpad_has_user_id_after_v2() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        // Column present: confirms v2 ran and reshaped scratchpad.
        let user_id_present: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('scratchpad') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            user_id_present, 1,
            "tenant scratchpad is missing the user_id shim column after v2"
        );

        // INSERT ... ON CONFLICT(user_id, session, entry_key) must match
        // an actual unique index on (user_id, session, entry_key).
        // Duplicate triggers the upsert path; no duplicate row results.
        conn.execute(
            "INSERT INTO scratchpad (user_id, session, agent, model, entry_key, value, expires_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now', '+5 minutes')) \
             ON CONFLICT(user_id, session, entry_key) DO UPDATE SET value = excluded.value",
            rusqlite::params![4_i64, "s1", "agent", "model", "key1", "v1"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO scratchpad (user_id, session, agent, model, entry_key, value, expires_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now', '+5 minutes')) \
             ON CONFLICT(user_id, session, entry_key) DO UPDATE SET value = excluded.value",
            rusqlite::params![4_i64, "s1", "agent", "model", "key1", "v2"],
        )
        .unwrap();

        let (count, value): (i64, String) = conn
            .query_row(
                "SELECT COUNT(*), MAX(value) FROM scratchpad WHERE user_id = 4",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(count, 1, "upsert collapsed into one row");
        assert_eq!(value, "v2");
    }

    #[test]
    fn v1_only_db_upgrades_cleanly_to_v2() {
        let conn = Connection::open_in_memory().unwrap();
        // Simulate an existing tenant at v1 (before v2 existed): apply v1
        // only, stamp schema_migrations, then call the runner.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        apply_schema_v1(&conn).unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO schema_migrations (version) VALUES (1)",
            [],
        )
        .unwrap();

        // The v1 scratchpad has no user_id column.
        let pre: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('scratchpad') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pre, 0);

        // Run the chain; v2 should catch it up.
        run_tenant_migrations(&conn).unwrap();

        let post: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('scratchpad') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(post, 1);
    }

    #[test]
    fn sessions_has_user_id_after_v3() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        // user_id column present on sessions.
        let user_id_present: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            user_id_present, 1,
            "tenant sessions is missing the user_id shim column after v3"
        );

        // session_output table exists.
        let output_table: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='session_output'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(output_table, 1, "tenant session_output table missing after v3");

        // Exercise the SQL shape kleos-lib sessions.rs actually uses.
        conn.execute(
            "INSERT INTO sessions (id, agent, user_id) VALUES (?1, ?2, ?3)",
            rusqlite::params!["sess-1", "claude-code", 4_i64],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_output (session_id, line) VALUES (?1, ?2)",
            rusqlite::params!["sess-1", "hello"],
        )
        .unwrap();

        let (id, agent, uid): (String, String, i64) = conn
            .query_row(
                "SELECT id, agent, user_id FROM sessions WHERE id = ?1 AND user_id = ?2",
                rusqlite::params!["sess-1", 4_i64],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(id, "sess-1");
        assert_eq!(agent, "claude-code");
        assert_eq!(uid, 4);

        let line: String = conn
            .query_row(
                "SELECT line FROM session_output WHERE session_id = ?1",
                rusqlite::params!["sess-1"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(line, "hello");
    }

    #[test]
    fn chiasm_tasks_usable_after_v4() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        // Both tables exist.
        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('chiasm_tasks', 'chiasm_task_updates')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 2, "chiasm_tasks and/or chiasm_task_updates missing after v4");

        // Exercise the SQL shape kleos-lib chiasm.rs actually uses.
        conn.execute(
            "INSERT INTO chiasm_tasks (agent, project, title, status, summary, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params!["claude-code", "engram-rust", "Phase 3.4", "active", None::<String>, 4_i64],
        )
        .unwrap();
        let task_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO chiasm_task_updates (task_id, agent, status, summary, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![task_id, "claude-code", "active", "started", 4_i64],
        )
        .unwrap();

        let (agent, project, uid): (String, String, i64) = conn
            .query_row(
                "SELECT agent, project, user_id FROM chiasm_tasks WHERE id = ?1 AND user_id = ?2",
                rusqlite::params![task_id, 4_i64],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(agent, "claude-code");
        assert_eq!(project, "engram-rust");
        assert_eq!(uid, 4);

        let update_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chiasm_task_updates WHERE task_id = ?1 AND user_id = ?2",
                rusqlite::params![task_id, 4_i64],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(update_count, 1);
    }

    #[test]
    fn v3_db_upgrades_cleanly_to_v4() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        apply_schema_v1(&conn).unwrap();
        apply_schema_v2_scratchpad_shim(&conn).unwrap();
        apply_schema_v3_sessions_shim(&conn).unwrap();
        conn.execute_batch(
            "INSERT OR IGNORE INTO schema_migrations (version) VALUES (1);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (2);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (3);",
        )
        .unwrap();

        // Pre: chiasm tables do not exist.
        let pre: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('chiasm_tasks', 'chiasm_task_updates')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pre, 0);

        // Run chain; v4 catches it up.
        run_tenant_migrations(&conn).unwrap();

        let post: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('chiasm_tasks', 'chiasm_task_updates')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(post, 2);
    }

    #[test]
    fn approvals_usable_after_v5() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        let table: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='approvals'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(table, 1, "approvals table missing after v5");

        // Exercise the SQL shape kleos-lib approvals/mod.rs actually uses.
        conn.execute(
            "INSERT INTO approvals (id, action, context, requester, status, created_at, expires_at, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                "appr-1",
                "DELETE /memories/1",
                None::<String>,
                "test-agent",
                "pending",
                "2026-04-22T00:00:00Z",
                "2026-04-22T00:02:00Z",
                4_i64,
            ],
        )
        .unwrap();

        let (id, status, uid): (String, String, i64) = conn
            .query_row(
                "SELECT id, status, user_id FROM approvals WHERE id = ?1 AND user_id = ?2",
                rusqlite::params!["appr-1", 4_i64],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(id, "appr-1");
        assert_eq!(status, "pending");
        assert_eq!(uid, 4);

        // Pending listing also works.
        let pending_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM approvals WHERE user_id = ?1 AND status = 'pending'",
                rusqlite::params![4_i64],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pending_count, 1);
    }

    #[test]
    fn v4_db_upgrades_cleanly_to_v5() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        apply_schema_v1(&conn).unwrap();
        apply_schema_v2_scratchpad_shim(&conn).unwrap();
        apply_schema_v3_sessions_shim(&conn).unwrap();
        apply_schema_v4_chiasm_shim(&conn).unwrap();
        conn.execute_batch(
            "INSERT OR IGNORE INTO schema_migrations (version) VALUES (1);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (2);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (3);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (4);",
        )
        .unwrap();

        let pre: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='approvals'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pre, 0);

        run_tenant_migrations(&conn).unwrap();

        let post: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='approvals'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(post, 1);
    }

    #[test]
    fn broca_actions_usable_after_v6() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        let table: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='broca_actions'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(table, 1, "broca_actions table missing after v6");

        // Exercise the INSERT shape kleos-lib services/broca.rs uses.
        conn.execute(
            "INSERT INTO broca_actions (agent, service, action, payload, narrative, axon_event_id, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                "claude-code",
                "cred",
                "resolve",
                r#"{"svc":"engram-rust","key":"claude-code-wsl"}"#,
                None::<String>,
                None::<i64>,
                4_i64,
            ],
        )
        .unwrap();

        let (agent, service, uid): (String, String, i64) = conn
            .query_row(
                "SELECT agent, service, user_id FROM broca_actions WHERE user_id = ?1",
                rusqlite::params![4_i64],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(agent, "claude-code");
        assert_eq!(service, "cred");
        assert_eq!(uid, 4);
    }

    #[test]
    fn v5_db_upgrades_cleanly_to_v6() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        apply_schema_v1(&conn).unwrap();
        apply_schema_v2_scratchpad_shim(&conn).unwrap();
        apply_schema_v3_sessions_shim(&conn).unwrap();
        apply_schema_v4_chiasm_shim(&conn).unwrap();
        apply_schema_v5_approvals_shim(&conn).unwrap();
        conn.execute_batch(
            "INSERT OR IGNORE INTO schema_migrations (version) VALUES (1);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (2);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (3);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (4);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (5);",
        )
        .unwrap();

        let pre: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='broca_actions'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pre, 0);

        run_tenant_migrations(&conn).unwrap();

        let post: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='broca_actions'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(post, 1);
    }

    #[test]
    fn projects_usable_after_v7() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('projects', 'memory_projects')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 2, "projects/memory_projects missing after v7");

        // Seed a memory so the FK target exists for memory_projects.
        conn.execute(
            "INSERT INTO memories (content, category, source, user_id) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["seed", "general", "test", 4_i64],
        )
        .unwrap();
        let memory_id = conn.last_insert_rowid();

        // Exercise the INSERT + SELECT shape projects.rs uses.
        let (project_id, _created_at): (i64, String) = conn
            .query_row(
                "INSERT INTO projects (name, description, status, metadata, user_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5) RETURNING id, created_at",
                rusqlite::params!["p1", None::<String>, "active", None::<String>, 4_i64],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();

        conn.execute(
            "INSERT OR IGNORE INTO memory_projects (memory_id, project_id) VALUES (?1, ?2)",
            rusqlite::params![memory_id, project_id],
        )
        .unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memory_projects WHERE project_id = ?1",
                rusqlite::params![project_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        let (name, uid): (String, i64) = conn
            .query_row(
                "SELECT name, user_id FROM projects WHERE id = ?1 AND user_id = ?2",
                rusqlite::params![project_id, 4_i64],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(name, "p1");
        assert_eq!(uid, 4);
    }

    #[test]
    fn v6_db_upgrades_cleanly_to_v7() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        apply_schema_v1(&conn).unwrap();
        apply_schema_v2_scratchpad_shim(&conn).unwrap();
        apply_schema_v3_sessions_shim(&conn).unwrap();
        apply_schema_v4_chiasm_shim(&conn).unwrap();
        apply_schema_v5_approvals_shim(&conn).unwrap();
        apply_schema_v6_broca_shim(&conn).unwrap();
        conn.execute_batch(
            "INSERT OR IGNORE INTO schema_migrations (version) VALUES (1);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (2);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (3);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (4);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (5);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (6);",
        )
        .unwrap();

        let pre: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('projects', 'memory_projects')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pre, 0);

        run_tenant_migrations(&conn).unwrap();

        let post: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('projects', 'memory_projects')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(post, 2);
    }

    #[test]
    fn activity_tables_usable_after_v8() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('axon_events', 'soma_agents')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 2, "axon_events or soma_agents missing after v8");

        // axon_events INSERT matches services/axon.rs publish_event.
        conn.execute(
            "INSERT INTO axon_events (channel, source, type, payload, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params!["agent.reports", "activity", "task.completed", "{}", 4_i64],
        )
        .unwrap();

        // soma_agents upsert / heartbeat shape.
        conn.execute(
            "INSERT INTO soma_agents (name, type, description, capabilities, config, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                "claude-code",
                "cli",
                None::<String>,
                "[]",
                "{}",
                4_i64,
            ],
        )
        .unwrap();

        let (event_count, agent_count): (i64, i64) = conn
            .query_row(
                "SELECT \
                   (SELECT COUNT(*) FROM axon_events WHERE user_id = ?1), \
                   (SELECT COUNT(*) FROM soma_agents WHERE user_id = ?1)",
                rusqlite::params![4_i64],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(event_count, 1);
        assert_eq!(agent_count, 1);
    }

    #[test]
    fn v7_db_upgrades_cleanly_to_v8() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        apply_schema_v1(&conn).unwrap();
        apply_schema_v2_scratchpad_shim(&conn).unwrap();
        apply_schema_v3_sessions_shim(&conn).unwrap();
        apply_schema_v4_chiasm_shim(&conn).unwrap();
        apply_schema_v5_approvals_shim(&conn).unwrap();
        apply_schema_v6_broca_shim(&conn).unwrap();
        apply_schema_v7_projects_shim(&conn).unwrap();
        conn.execute_batch(
            "INSERT OR IGNORE INTO schema_migrations (version) VALUES (1);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (2);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (3);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (4);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (5);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (6);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (7);",
        )
        .unwrap();

        let pre: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('axon_events', 'soma_agents')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pre, 0);

        run_tenant_migrations(&conn).unwrap();

        let post: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('axon_events', 'soma_agents')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(post, 2);
    }

    #[test]
    fn webhooks_usable_after_v9() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('webhooks', 'webhook_dead_letters')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 2, "webhooks/webhook_dead_letters missing after v9");

        conn.execute(
            "INSERT INTO webhooks (user_id, url, events, secret) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![4_i64, "https://example.test/hook", "memory.created", None::<String>],
        )
        .unwrap();
        let webhook_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO webhook_dead_letters (webhook_id, event, payload, attempts, last_error, last_status_code) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![webhook_id, "memory.created", "{}", 3_i64, "timeout", 504_i64],
        )
        .unwrap();

        let (url, uid): (String, i64) = conn
            .query_row(
                "SELECT url, user_id FROM webhooks WHERE id = ?1",
                rusqlite::params![webhook_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(url, "https://example.test/hook");
        assert_eq!(uid, 4);

        let dl_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM webhook_dead_letters WHERE webhook_id = ?1",
                rusqlite::params![webhook_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dl_count, 1);
    }

    #[test]
    fn v8_db_upgrades_cleanly_to_v9() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        apply_schema_v1(&conn).unwrap();
        apply_schema_v2_scratchpad_shim(&conn).unwrap();
        apply_schema_v3_sessions_shim(&conn).unwrap();
        apply_schema_v4_chiasm_shim(&conn).unwrap();
        apply_schema_v5_approvals_shim(&conn).unwrap();
        apply_schema_v6_broca_shim(&conn).unwrap();
        apply_schema_v7_projects_shim(&conn).unwrap();
        apply_schema_v8_activity_shim(&conn).unwrap();
        conn.execute_batch(
            "INSERT OR IGNORE INTO schema_migrations (version) VALUES (1);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (2);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (3);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (4);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (5);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (6);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (7);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (8);",
        )
        .unwrap();

        let pre: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('webhooks', 'webhook_dead_letters')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pre, 0);

        run_tenant_migrations(&conn).unwrap();

        let post: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('webhooks', 'webhook_dead_letters')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(post, 2);
    }

    #[test]
    fn ingestion_tables_usable_after_v10() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('upload_sessions', 'upload_chunks', 'ingestion_hashes')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 3, "ingestion tables missing after v10");

        // Exercise upload_sessions INSERT shape routes/ingestion uses.
        conn.execute(
            "INSERT INTO upload_sessions
               (upload_id, user_id, filename, content_type, source,
                total_size, total_chunks, chunk_size, status, expires_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'active', ?9)",
            rusqlite::params![
                "upl-1",
                4_i64,
                None::<String>,
                None::<String>,
                "upload",
                None::<i64>,
                None::<i64>,
                1_048_576_i64,
                "2099-01-01 00:00:00",
            ],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO upload_chunks (upload_id, chunk_index, chunk_hash, size, data) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params!["upl-1", 0_i64, "abc123", 3_i64, vec![1u8, 2, 3]],
        )
        .unwrap();

        conn.execute(
            "INSERT OR IGNORE INTO ingestion_hashes (sha256, user_id, job_id) VALUES (?1, ?2, ?3)",
            rusqlite::params!["deadbeef", 4_i64, "job-1"],
        )
        .unwrap();

        let (session_count, chunk_count, hash_count): (i64, i64, i64) = conn
            .query_row(
                "SELECT \
                   (SELECT COUNT(*) FROM upload_sessions WHERE user_id = ?1), \
                   (SELECT COUNT(*) FROM upload_chunks WHERE upload_id = ?2), \
                   (SELECT COUNT(*) FROM ingestion_hashes WHERE user_id = ?1)",
                rusqlite::params![4_i64, "upl-1"],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(session_count, 1);
        assert_eq!(chunk_count, 1);
        assert_eq!(hash_count, 1);
    }

    #[test]
    fn v9_db_upgrades_cleanly_to_v10() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        apply_schema_v1(&conn).unwrap();
        apply_schema_v2_scratchpad_shim(&conn).unwrap();
        apply_schema_v3_sessions_shim(&conn).unwrap();
        apply_schema_v4_chiasm_shim(&conn).unwrap();
        apply_schema_v5_approvals_shim(&conn).unwrap();
        apply_schema_v6_broca_shim(&conn).unwrap();
        apply_schema_v7_projects_shim(&conn).unwrap();
        apply_schema_v8_activity_shim(&conn).unwrap();
        apply_schema_v9_webhooks_shim(&conn).unwrap();
        conn.execute_batch(
            "INSERT OR IGNORE INTO schema_migrations (version) VALUES (1);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (2);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (3);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (4);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (5);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (6);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (7);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (8);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (9);",
        )
        .unwrap();

        let pre: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('upload_sessions', 'upload_chunks', 'ingestion_hashes')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pre, 0);

        run_tenant_migrations(&conn).unwrap();

        let post: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('upload_sessions', 'upload_chunks', 'ingestion_hashes')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(post, 3);
    }

    #[test]
    fn axon_family_usable_after_v11() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('axon_channels', 'axon_subscriptions', 'axon_cursors')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 3, "axon family tables missing after v11");

        conn.execute(
            "INSERT INTO axon_channels (name, description) VALUES (?1, ?2)",
            rusqlite::params!["system", "System events"],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO axon_subscriptions (agent, channel, filter_type, webhook_url, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params!["claude-code", "system", None::<String>, None::<String>, 4_i64],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO axon_cursors (agent, channel, last_event_id, user_id) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["claude-code", "system", 0_i64, 4_i64],
        )
        .unwrap();

        let (ch, sub, cur): (i64, i64, i64) = conn
            .query_row(
                "SELECT \
                   (SELECT COUNT(*) FROM axon_channels), \
                   (SELECT COUNT(*) FROM axon_subscriptions WHERE user_id = ?1), \
                   (SELECT COUNT(*) FROM axon_cursors WHERE user_id = ?1)",
                rusqlite::params![4_i64],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(ch, 1);
        assert_eq!(sub, 1);
        assert_eq!(cur, 1);
    }

    #[test]
    fn v10_db_upgrades_cleanly_to_v11() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        apply_schema_v1(&conn).unwrap();
        apply_schema_v2_scratchpad_shim(&conn).unwrap();
        apply_schema_v3_sessions_shim(&conn).unwrap();
        apply_schema_v4_chiasm_shim(&conn).unwrap();
        apply_schema_v5_approvals_shim(&conn).unwrap();
        apply_schema_v6_broca_shim(&conn).unwrap();
        apply_schema_v7_projects_shim(&conn).unwrap();
        apply_schema_v8_activity_shim(&conn).unwrap();
        apply_schema_v9_webhooks_shim(&conn).unwrap();
        apply_schema_v10_ingestion_shim(&conn).unwrap();
        conn.execute_batch(
            "INSERT OR IGNORE INTO schema_migrations (version) VALUES (1);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (2);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (3);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (4);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (5);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (6);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (7);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (8);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (9);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (10);",
        )
        .unwrap();

        let pre: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('axon_channels', 'axon_subscriptions', 'axon_cursors')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pre, 0);

        run_tenant_migrations(&conn).unwrap();

        let post: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('axon_channels', 'axon_subscriptions', 'axon_cursors')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(post, 3);
    }

    #[test]
    fn soma_family_usable_after_v12() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('soma_groups', 'soma_agent_groups', 'soma_agent_logs')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 3, "soma family tables missing after v12");

        // Seed a soma_agents row so FKs have a target.
        conn.execute(
            "INSERT INTO soma_agents (name, type, description, capabilities, config, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params!["claude-code", "cli", None::<String>, "[]", "{}", 4_i64],
        )
        .unwrap();
        let agent_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO soma_groups (name, description, user_id) VALUES (?1, ?2, ?3)",
            rusqlite::params!["infra", None::<String>, 4_i64],
        )
        .unwrap();
        let group_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO soma_agent_groups (agent_id, group_id) VALUES (?1, ?2)",
            rusqlite::params![agent_id, group_id],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO soma_agent_logs (agent_id, level, message, data) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![agent_id, "info", "heartbeat ok", "{}"],
        )
        .unwrap();

        let (g, ag, l): (i64, i64, i64) = conn
            .query_row(
                "SELECT \
                   (SELECT COUNT(*) FROM soma_groups WHERE user_id = ?1), \
                   (SELECT COUNT(*) FROM soma_agent_groups WHERE group_id = ?2), \
                   (SELECT COUNT(*) FROM soma_agent_logs WHERE agent_id = ?3)",
                rusqlite::params![4_i64, group_id, agent_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(g, 1);
        assert_eq!(ag, 1);
        assert_eq!(l, 1);
    }

    #[test]
    fn v11_db_upgrades_cleanly_to_v12() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        apply_schema_v1(&conn).unwrap();
        apply_schema_v2_scratchpad_shim(&conn).unwrap();
        apply_schema_v3_sessions_shim(&conn).unwrap();
        apply_schema_v4_chiasm_shim(&conn).unwrap();
        apply_schema_v5_approvals_shim(&conn).unwrap();
        apply_schema_v6_broca_shim(&conn).unwrap();
        apply_schema_v7_projects_shim(&conn).unwrap();
        apply_schema_v8_activity_shim(&conn).unwrap();
        apply_schema_v9_webhooks_shim(&conn).unwrap();
        apply_schema_v10_ingestion_shim(&conn).unwrap();
        apply_schema_v11_axon_shim(&conn).unwrap();
        conn.execute_batch(
            "INSERT OR IGNORE INTO schema_migrations (version) VALUES (1);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (2);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (3);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (4);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (5);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (6);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (7);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (8);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (9);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (10);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (11);",
        )
        .unwrap();

        let pre: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('soma_groups', 'soma_agent_groups', 'soma_agent_logs')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pre, 0);

        run_tenant_migrations(&conn).unwrap();

        let post: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('soma_groups', 'soma_agent_groups', 'soma_agent_logs')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(post, 3);
    }

    #[test]
    fn loom_family_usable_after_v13() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' \
                 AND name IN ('loom_workflows', 'loom_runs', 'loom_steps', 'loom_run_logs')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 4, "loom family tables missing after v13");

        // Exercise INSERT shapes services/loom.rs actually uses.
        conn.execute(
            "INSERT INTO loom_workflows (name, description, steps, user_id) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["wf-1", None::<String>, "[]", 4_i64],
        )
        .unwrap();
        let workflow_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO loom_runs (workflow_id, status, input, output, user_id) \
             VALUES (?1, 'pending', '{}', '{}', ?2)",
            rusqlite::params![workflow_id, 4_i64],
        )
        .unwrap();
        let run_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO loom_steps \
             (run_id, name, type, config, status, input, output, depends_on, retry_count, max_retries, timeout_ms) \
             VALUES (?1, ?2, ?3, ?4, 'pending', '{}', '{}', '[]', 0, 3, 30000)",
            rusqlite::params![run_id, "s1", "transform", "{}"],
        )
        .unwrap();
        let step_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO loom_run_logs (run_id, step_id, level, message, data) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![run_id, step_id, "info", "started", "{}"],
        )
        .unwrap();

        let (w, ru, st, lg): (i64, i64, i64, i64) = conn
            .query_row(
                "SELECT \
                   (SELECT COUNT(*) FROM loom_workflows WHERE user_id = ?1), \
                   (SELECT COUNT(*) FROM loom_runs WHERE user_id = ?1), \
                   (SELECT COUNT(*) FROM loom_steps WHERE run_id = ?2), \
                   (SELECT COUNT(*) FROM loom_run_logs WHERE run_id = ?2)",
                rusqlite::params![4_i64, run_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(w, 1);
        assert_eq!(ru, 1);
        assert_eq!(st, 1);
        assert_eq!(lg, 1);
    }

    #[test]
    fn v12_db_upgrades_cleanly_to_v13() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        apply_schema_v1(&conn).unwrap();
        apply_schema_v2_scratchpad_shim(&conn).unwrap();
        apply_schema_v3_sessions_shim(&conn).unwrap();
        apply_schema_v4_chiasm_shim(&conn).unwrap();
        apply_schema_v5_approvals_shim(&conn).unwrap();
        apply_schema_v6_broca_shim(&conn).unwrap();
        apply_schema_v7_projects_shim(&conn).unwrap();
        apply_schema_v8_activity_shim(&conn).unwrap();
        apply_schema_v9_webhooks_shim(&conn).unwrap();
        apply_schema_v10_ingestion_shim(&conn).unwrap();
        apply_schema_v11_axon_shim(&conn).unwrap();
        apply_schema_v12_soma_shim(&conn).unwrap();
        conn.execute_batch(
            "INSERT OR IGNORE INTO schema_migrations (version) VALUES (1);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (2);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (3);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (4);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (5);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (6);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (7);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (8);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (9);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (10);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (11);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (12);",
        )
        .unwrap();

        let pre: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' \
                 AND name IN ('loom_workflows', 'loom_runs', 'loom_steps', 'loom_run_logs')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pre, 0);

        run_tenant_migrations(&conn).unwrap();

        let post: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' \
                 AND name IN ('loom_workflows', 'loom_runs', 'loom_steps', 'loom_run_logs')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(post, 4);
    }

    #[test]
    fn graph_family_usable_after_v14() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' \
                 AND name IN ('entities', 'entity_relationships', 'memory_entities', \
                              'structured_facts', 'entity_cooccurrences', \
                              'memory_pagerank', 'pagerank_dirty')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 7, "graph family tables missing after v14");

        // Seed memory -> entities -> relationship -> memory_entities -> cooccurrence.
        conn.execute(
            "INSERT INTO memories (content, category, source, user_id) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["seed memory", "general", "test", 4_i64],
        )
        .unwrap();
        let memory_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO entities (name, entity_type, description, aliases, user_id, space_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params!["alpha", "concept", None::<String>, None::<String>, 4_i64, None::<i64>],
        )
        .unwrap();
        let a_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO entities (name, entity_type, description, aliases, user_id, space_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params!["beta", "concept", None::<String>, None::<String>, 4_i64, None::<i64>],
        )
        .unwrap();
        let b_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO entity_relationships \
             (source_entity_id, target_entity_id, relationship_type, strength) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![a_id, b_id, "related", 0.8_f64],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO memory_entities (memory_id, entity_id, salience) VALUES (?1, ?2, ?3)",
            rusqlite::params![memory_id, a_id, 1.0_f64],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO entity_cooccurrences (entity_a_id, entity_b_id, count, user_id) \
             VALUES (?1, ?2, 1, ?3) \
             ON CONFLICT(entity_a_id, entity_b_id) DO UPDATE SET count = count + 1",
            rusqlite::params![a_id, b_id, 4_i64],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO structured_facts (memory_id, subject, predicate, object, confidence, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![memory_id, "alpha", "relates_to", "beta", 0.9_f64, 4_i64],
        )
        .unwrap();

        // PageRank upsert (monolith shape: memory_id PK, user_id, INTEGER computed_at).
        conn.execute(
            "INSERT INTO memory_pagerank (memory_id, user_id, score, computed_at) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(memory_id) DO UPDATE SET score = excluded.score",
            rusqlite::params![memory_id, 4_i64, 0.5_f64, 1_700_000_000_i64],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO pagerank_dirty (user_id, dirty_count, last_refresh) VALUES (?1, ?2, ?3) \
             ON CONFLICT(user_id) DO UPDATE SET dirty_count = dirty_count + ?2",
            rusqlite::params![4_i64, 3_i64, 1_700_000_000_i64],
        )
        .unwrap();

        let (e, r, me, co, f, pr, pd): (i64, i64, i64, i64, i64, i64, i64) = conn
            .query_row(
                "SELECT \
                   (SELECT COUNT(*) FROM entities WHERE user_id = ?1), \
                   (SELECT COUNT(*) FROM entity_relationships), \
                   (SELECT COUNT(*) FROM memory_entities), \
                   (SELECT COUNT(*) FROM entity_cooccurrences WHERE user_id = ?1), \
                   (SELECT COUNT(*) FROM structured_facts WHERE user_id = ?1), \
                   (SELECT COUNT(*) FROM memory_pagerank WHERE user_id = ?1), \
                   (SELECT COUNT(*) FROM pagerank_dirty WHERE user_id = ?1)",
                rusqlite::params![4_i64],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(e, 2);
        assert_eq!(r, 1);
        assert_eq!(me, 1);
        assert_eq!(co, 1);
        assert_eq!(f, 1);
        assert_eq!(pr, 1);
        assert_eq!(pd, 1);
    }

    #[test]
    fn v13_db_upgrades_cleanly_to_v14() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        apply_schema_v1(&conn).unwrap();
        apply_schema_v2_scratchpad_shim(&conn).unwrap();
        apply_schema_v3_sessions_shim(&conn).unwrap();
        apply_schema_v4_chiasm_shim(&conn).unwrap();
        apply_schema_v5_approvals_shim(&conn).unwrap();
        apply_schema_v6_broca_shim(&conn).unwrap();
        apply_schema_v7_projects_shim(&conn).unwrap();
        apply_schema_v8_activity_shim(&conn).unwrap();
        apply_schema_v9_webhooks_shim(&conn).unwrap();
        apply_schema_v10_ingestion_shim(&conn).unwrap();
        apply_schema_v11_axon_shim(&conn).unwrap();
        apply_schema_v12_soma_shim(&conn).unwrap();
        apply_schema_v13_loom_shim(&conn).unwrap();
        conn.execute_batch(
            "INSERT OR IGNORE INTO schema_migrations (version) VALUES (1);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (2);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (3);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (4);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (5);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (6);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (7);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (8);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (9);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (10);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (11);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (12);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (13);",
        )
        .unwrap();

        // Pre: v1 `entities` still has the stale shape (no user_id, `type` instead of `entity_type`).
        let stale_user_id: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('entities') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stale_user_id, 0, "v1 entities shouldn't yet have user_id");

        let missing_tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' \
                 AND name IN ('memory_entities', 'entity_cooccurrences', 'pagerank_dirty')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            missing_tables, 0,
            "new graph tables shouldn't exist before v14"
        );

        run_tenant_migrations(&conn).unwrap();

        let post_user_id: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('entities') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(post_user_id, 1, "v14 should reshape entities with user_id");

        let post_tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' \
                 AND name IN ('memory_entities', 'entity_cooccurrences', 'pagerank_dirty')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(post_tables, 3);
    }

    #[test]
    fn thymus_family_usable_after_v15() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' \
                 AND name IN ('rubrics', 'evaluations', 'quality_metrics', \
                              'session_quality', 'behavioral_drift_events')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 5, "thymus family tables missing after v15");

        conn.execute(
            "INSERT INTO rubrics (name, description, criteria, user_id) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["r1", None::<String>, "[]", 4_i64],
        )
        .unwrap();
        let rubric_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO evaluations \
             (rubric_id, agent, subject, input, output, scores, overall_score, evaluator, user_id) \
             VALUES (?1, ?2, ?3, '{}', '{}', '{}', ?4, ?5, ?6)",
            rusqlite::params![rubric_id, "claude-code", "turn-1", 0.9_f64, "claude", 4_i64],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO quality_metrics (agent, metric, value, tags, user_id) \
             VALUES (?1, ?2, ?3, '{}', ?4)",
            rusqlite::params!["claude-code", "tokens", 1234_f64, 4_i64],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO session_quality (session_id, agent, turn_count, user_id) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["sess-1", "claude-code", 5_i64, 4_i64],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO behavioral_drift_events (agent, session_id, drift_type, signal, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params!["claude-code", "sess-1", "persona", "{}", 4_i64],
        )
        .unwrap();

        let (r, e, m, sq, d): (i64, i64, i64, i64, i64) = conn
            .query_row(
                "SELECT \
                   (SELECT COUNT(*) FROM rubrics WHERE user_id = ?1), \
                   (SELECT COUNT(*) FROM evaluations WHERE user_id = ?1), \
                   (SELECT COUNT(*) FROM quality_metrics WHERE user_id = ?1), \
                   (SELECT COUNT(*) FROM session_quality WHERE user_id = ?1), \
                   (SELECT COUNT(*) FROM behavioral_drift_events WHERE user_id = ?1)",
                rusqlite::params![4_i64],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(r, 1);
        assert_eq!(e, 1);
        assert_eq!(m, 1);
        assert_eq!(sq, 1);
        assert_eq!(d, 1);
    }

    #[test]
    fn v14_db_upgrades_cleanly_to_v15() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        apply_schema_v1(&conn).unwrap();
        apply_schema_v2_scratchpad_shim(&conn).unwrap();
        apply_schema_v3_sessions_shim(&conn).unwrap();
        apply_schema_v4_chiasm_shim(&conn).unwrap();
        apply_schema_v5_approvals_shim(&conn).unwrap();
        apply_schema_v6_broca_shim(&conn).unwrap();
        apply_schema_v7_projects_shim(&conn).unwrap();
        apply_schema_v8_activity_shim(&conn).unwrap();
        apply_schema_v9_webhooks_shim(&conn).unwrap();
        apply_schema_v10_ingestion_shim(&conn).unwrap();
        apply_schema_v11_axon_shim(&conn).unwrap();
        apply_schema_v12_soma_shim(&conn).unwrap();
        apply_schema_v13_loom_shim(&conn).unwrap();
        apply_schema_v14_graph_shim(&conn).unwrap();
        conn.execute_batch(
            "INSERT OR IGNORE INTO schema_migrations (version) VALUES (1);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (2);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (3);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (4);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (5);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (6);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (7);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (8);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (9);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (10);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (11);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (12);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (13);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (14);",
        )
        .unwrap();

        let pre: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' \
                 AND name IN ('rubrics', 'evaluations', 'quality_metrics', \
                              'session_quality', 'behavioral_drift_events')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pre, 0);

        run_tenant_migrations(&conn).unwrap();

        let post: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' \
                 AND name IN ('rubrics', 'evaluations', 'quality_metrics', \
                              'session_quality', 'behavioral_drift_events')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(post, 5);
    }

    #[test]
    fn portability_family_usable_after_v16() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' \
                 AND name IN ('user_preferences', 'conversations', 'app_state')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 3, "portability tables missing after v16");

        // user_preferences should now expose the KV shape preferences.rs expects.
        let has_key: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('user_preferences') WHERE name='key'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has_key, 1, "user_preferences missing 'key' column after v16");

        conn.execute(
            "INSERT INTO user_preferences (user_id, key, value) VALUES (?1, ?2, ?3) \
             ON CONFLICT(user_id, key) DO UPDATE SET value = excluded.value",
            rusqlite::params![4_i64, "persona", "gir"],
        )
        .unwrap();
        // Upsert collapses to one row.
        conn.execute(
            "INSERT INTO user_preferences (user_id, key, value) VALUES (?1, ?2, ?3) \
             ON CONFLICT(user_id, key) DO UPDATE SET value = excluded.value",
            rusqlite::params![4_i64, "persona", "technical"],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO conversations (agent, session_id, title, user_id) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["claude-code", "sess-1", "hello", 4_i64],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO app_state (key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params!["user:4:theme", "dark"],
        )
        .unwrap();

        let (pref_value, conv_count, state_value): (String, i64, String) = conn
            .query_row(
                "SELECT \
                   (SELECT value FROM user_preferences WHERE user_id = ?1 AND key = 'persona'), \
                   (SELECT COUNT(*) FROM conversations WHERE user_id = ?1), \
                   (SELECT value FROM app_state WHERE key = ?2)",
                rusqlite::params![4_i64, "user:4:theme"],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(pref_value, "technical");
        assert_eq!(conv_count, 1);
        assert_eq!(state_value, "dark");
    }

    #[test]
    fn v15_db_upgrades_cleanly_to_v16() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        apply_schema_v1(&conn).unwrap();
        apply_schema_v2_scratchpad_shim(&conn).unwrap();
        apply_schema_v3_sessions_shim(&conn).unwrap();
        apply_schema_v4_chiasm_shim(&conn).unwrap();
        apply_schema_v5_approvals_shim(&conn).unwrap();
        apply_schema_v6_broca_shim(&conn).unwrap();
        apply_schema_v7_projects_shim(&conn).unwrap();
        apply_schema_v8_activity_shim(&conn).unwrap();
        apply_schema_v9_webhooks_shim(&conn).unwrap();
        apply_schema_v10_ingestion_shim(&conn).unwrap();
        apply_schema_v11_axon_shim(&conn).unwrap();
        apply_schema_v12_soma_shim(&conn).unwrap();
        apply_schema_v13_loom_shim(&conn).unwrap();
        apply_schema_v14_graph_shim(&conn).unwrap();
        apply_schema_v15_thymus_shim(&conn).unwrap();
        conn.execute_batch(
            "INSERT OR IGNORE INTO schema_migrations (version) VALUES (1);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (2);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (3);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (4);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (5);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (6);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (7);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (8);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (9);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (10);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (11);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (12);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (13);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (14);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (15);",
        )
        .unwrap();

        // Pre: conversations and app_state do not exist; user_preferences
        // still has the v1 behavioral shape (no 'key' column).
        let pre_conv_app: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' \
                 AND name IN ('conversations', 'app_state')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pre_conv_app, 0);

        let pre_key: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('user_preferences') WHERE name='key'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            pre_key, 0,
            "v1 user_preferences should not yet have the KV 'key' column"
        );

        run_tenant_migrations(&conn).unwrap();

        let post_conv_app: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' \
                 AND name IN ('conversations', 'app_state')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(post_conv_app, 2);

        let post_key: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('user_preferences') WHERE name='key'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(post_key, 1);
    }

    #[test]
    fn v2_db_upgrades_cleanly_to_v3() {
        let conn = Connection::open_in_memory().unwrap();
        // Simulate an existing tenant at v2 (before v3 existed): apply v1+v2
        // only, stamp schema_migrations, then call the runner.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        apply_schema_v1(&conn).unwrap();
        apply_schema_v2_scratchpad_shim(&conn).unwrap();
        conn.execute_batch(
            "INSERT OR IGNORE INTO schema_migrations (version) VALUES (1);
             INSERT OR IGNORE INTO schema_migrations (version) VALUES (2);",
        )
        .unwrap();

        // Pre: v1 sessions has no user_id, and session_output does not exist.
        let pre_user: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pre_user, 0);

        let pre_output: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='session_output'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pre_output, 0);

        // Run chain; v3 catches it up.
        run_tenant_migrations(&conn).unwrap();

        let post_user: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(post_user, 1);

        let post_output: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='session_output'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(post_output, 1);
    }
}
