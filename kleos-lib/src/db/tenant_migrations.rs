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
    TenantMigration {
        version: 17,
        description: "growth_reflections_shim",
        up: apply_schema_v17_growth_shim,
    },
    TenantMigration {
        version: 18,
        description: "intelligence_family_shim",
        up: apply_schema_v18_intelligence_shim,
    },
    TenantMigration {
        version: 19,
        description: "skills_family_shim",
        up: apply_schema_v19_skills_shim,
    },
    TenantMigration {
        version: 20,
        description: "episodes_user_id_and_fts_shim",
        up: apply_schema_v20_episodes_shim,
    },
    TenantMigration {
        version: 21,
        description: "messages_and_fts_shim",
        up: apply_schema_v21_messages_shim,
    },
    TenantMigration {
        version: 22,
        description: "memories_user_id_drop",
        up: apply_schema_v22_memories_drop,
    },
    TenantMigration {
        version: 23,
        description: "scratchpad_user_id_drop",
        up: apply_schema_v23_scratchpad_drop,
    },
    TenantMigration {
        version: 24,
        description: "sessions_user_id_drop",
        up: apply_schema_v24_sessions_drop,
    },
    TenantMigration {
        version: 25,
        description: "chiasm_user_id_drop",
        up: apply_schema_v25_chiasm_drop,
    },
    TenantMigration {
        version: 26,
        description: "approvals_user_id_drop",
        up: apply_schema_v26_approvals_drop,
    },
    TenantMigration {
        version: 27,
        description: "broca_user_id_drop",
        up: apply_schema_v27_broca_drop,
    },
    TenantMigration {
        version: 28,
        description: "projects_user_id_drop",
        up: apply_schema_v28_projects_drop,
    },
    TenantMigration {
        version: 29,
        description: "activity_user_id_drop",
        up: apply_schema_v29_activity_drop,
    },
    TenantMigration {
        version: 30,
        description: "webhooks_user_id_drop",
        up: apply_schema_v30_webhooks_drop,
    },
    TenantMigration {
        version: 31,
        description: "axon_user_id_drop",
        up: apply_schema_v31_axon_drop,
    },
    TenantMigration {
        version: 32,
        description: "growth_user_id_drop",
        up: apply_schema_v32_growth_drop,
    },
    TenantMigration {
        version: 33,
        description: "ingestion_hashes_user_id_drop",
        up: apply_schema_v33_ingestion_hashes_drop,
    },
    TenantMigration {
        version: 34,
        description: "loom_user_id_drop",
        up: apply_schema_v34_loom_drop,
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

fn apply_schema_v17_growth_shim(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v17_growth.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v17 failed: {e}")))
}

fn apply_schema_v18_intelligence_shim(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v18_intelligence.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v18 failed: {e}")))
}

fn apply_schema_v19_skills_shim(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v19_skills.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v19 failed: {e}")))
}

fn apply_schema_v20_episodes_shim(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v20_episodes.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v20 failed: {e}")))
}

fn apply_schema_v21_messages_shim(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v21_messages.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v21 failed: {e}")))
}

fn apply_schema_v22_memories_drop(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v22_memories_drop.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v22 failed: {e}")))
}

fn apply_schema_v23_scratchpad_drop(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v23_scratchpad.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v23 failed: {e}")))
}

fn apply_schema_v24_sessions_drop(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v24_sessions_drop.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v24 failed: {e}")))
}

fn apply_schema_v25_chiasm_drop(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v25_chiasm_drop.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v25 failed: {e}")))
}

fn apply_schema_v26_approvals_drop(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v26_approvals_drop.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v26 failed: {e}")))
}

fn apply_schema_v27_broca_drop(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v27_broca_drop.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v27 failed: {e}")))
}

fn apply_schema_v28_projects_drop(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v28_projects_drop.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v28 failed: {e}")))
}

fn apply_schema_v29_activity_drop(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v29_activity_drop.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v29 failed: {e}")))
}

fn apply_schema_v30_webhooks_drop(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v30_webhooks_drop.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v30 failed: {e}")))
}

fn apply_schema_v31_axon_drop(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v31_axon_drop.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v31 failed: {e}")))
}

fn apply_schema_v32_growth_drop(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v32_growth_drop.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v32 failed: {e}")))
}

fn apply_schema_v33_ingestion_hashes_drop(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!(
        "../tenant/schema_v33_ingestion_hashes_drop.sql"
    ))
    .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v33 failed: {e}")))
}

fn apply_schema_v34_loom_drop(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v34_loom_drop.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v34 failed: {e}")))
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
        // v2 added the user_id shim; v23 drops it. This test locks v2's
        // behaviour by stopping the chain at v22 (before v23's rebuild).
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        for m in TENANT_MIGRATIONS.iter() {
            if m.version >= 23 {
                break;
            }
            (m.up)(&conn).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
                rusqlite::params![m.version],
            )
            .unwrap();
        }

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

    /// v23: scratchpad must NOT have a user_id column after the full
    /// migration chain completes.
    #[test]
    fn user_id_absent_from_scratchpad_after_v23() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('scratchpad') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        assert_eq!(count, 0, "scratchpad still has user_id column after v23");
    }

    /// v23: the new UNIQUE(session, agent, entry_key) supports per-agent
    /// upsert within a session, and collisions on that triple still collapse.
    #[test]
    fn scratchpad_constraint_reshaped_after_v23() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        // Two different agents in the same (session, entry_key) coexist.
        conn.execute(
            "INSERT INTO scratchpad (session, agent, model, entry_key, value, expires_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now', '+5 minutes')) \
             ON CONFLICT(session, agent, entry_key) DO UPDATE SET value = excluded.value",
            rusqlite::params!["s1", "agentA", "m", "k1", "vA"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO scratchpad (session, agent, model, entry_key, value, expires_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now', '+5 minutes')) \
             ON CONFLICT(session, agent, entry_key) DO UPDATE SET value = excluded.value",
            rusqlite::params!["s1", "agentB", "m", "k1", "vB"],
        )
        .unwrap();
        // Upsert on the same (session, agent, entry_key) collapses.
        conn.execute(
            "INSERT INTO scratchpad (session, agent, model, entry_key, value, expires_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now', '+5 minutes')) \
             ON CONFLICT(session, agent, entry_key) DO UPDATE SET value = excluded.value",
            rusqlite::params!["s1", "agentA", "m", "k1", "vA2"],
        )
        .unwrap();

        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM scratchpad", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 2, "two agents coexist; A's upsert stays collapsed");

        let value_a: String = conn
            .query_row(
                "SELECT value FROM scratchpad WHERE session='s1' AND agent='agentA' AND entry_key='k1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(value_a, "vA2");
    }

    /// v23: rows inserted under the v2 shim shape survive the rebuild intact.
    #[test]
    fn scratchpad_rows_preserved_through_v23() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();

        // Apply migrations v1..v22 (stop before v23).
        for m in TENANT_MIGRATIONS.iter() {
            if m.version >= 23 {
                break;
            }
            (m.up)(&conn).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
                rusqlite::params![m.version],
            )
            .unwrap();
        }

        // Insert a v2-shaped row carrying user_id.
        conn.execute(
            "INSERT INTO scratchpad (user_id, session, agent, model, entry_key, value, expires_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now', '+5 minutes'))",
            rusqlite::params![1_i64, "sess-pre", "gir", "gpt", "mission", "tacos"],
        )
        .unwrap();
        let pre_id: i64 = conn
            .query_row("SELECT last_insert_rowid()", [], |r| r.get(0))
            .unwrap();
        assert!(pre_id > 0);

        // Apply v23.
        apply_schema_v23_scratchpad_drop(&conn).unwrap();

        // Row still present with every non-user_id field intact.
        let (session, agent, model, entry_key, value): (String, String, String, String, String) =
            conn.query_row(
                "SELECT session, agent, model, entry_key, value FROM scratchpad WHERE id = ?1",
                rusqlite::params![pre_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(session, "sess-pre");
        assert_eq!(agent, "gir");
        assert_eq!(model, "gpt");
        assert_eq!(entry_key, "mission");
        assert_eq!(value, "tacos");

        // user_id column is gone.
        let col_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('scratchpad') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(col_count, 0, "user_id column must be absent after v23");
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

        // Run the chain; v2 adds user_id, v23 later drops it. End state: absent.
        run_tenant_migrations(&conn).unwrap();

        let post: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('scratchpad') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(post, 0);
    }

    #[test]
    fn sessions_has_user_id_after_v3() {
        // v3 added the user_id shim on sessions; v24 drops it. This test
        // locks v3's shape by capping the chain at v23 (before v24).
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        for m in TENANT_MIGRATIONS.iter() {
            if m.version >= 24 {
                break;
            }
            (m.up)(&conn).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
                rusqlite::params![m.version],
            )
            .unwrap();
        }

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
        assert_eq!(
            output_table, 1,
            "tenant session_output table missing after v3"
        );

        // Exercise the SQL shape kleos-lib sessions.rs used pre-v24.
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

    /// v24: sessions must NOT have a user_id column after the full chain.
    #[test]
    fn user_id_absent_from_sessions_after_v24() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        assert_eq!(count, 0, "sessions still has user_id column after v24");
    }

    /// v24: the post-drop sessions table supports the SQL shape kleos-lib
    /// sessions.rs now uses (no user_id on INSERT, no user_id predicate on
    /// SELECT). session_output remains untouched and writable.
    #[test]
    fn sessions_usable_after_v24() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        conn.execute(
            "INSERT INTO sessions (id, agent) VALUES (?1, ?2)",
            rusqlite::params!["sess-v24", "claude-code"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_output (session_id, line) VALUES (?1, ?2)",
            rusqlite::params!["sess-v24", "tacos"],
        )
        .unwrap();

        let (id, agent): (String, String) = conn
            .query_row(
                "SELECT id, agent FROM sessions WHERE id = ?1",
                rusqlite::params!["sess-v24"],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(id, "sess-v24");
        assert_eq!(agent, "claude-code");

        let line: String = conn
            .query_row(
                "SELECT line FROM session_output WHERE session_id = ?1",
                rusqlite::params!["sess-v24"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(line, "tacos");

        // idx_sessions_user is gone.
        let idx: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_sessions_user'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(idx, 0, "idx_sessions_user still present after v24");
    }

    /// v24: rows inserted under the v3 shim shape survive the drop with
    /// every non-user_id field intact.
    #[test]
    fn sessions_rows_preserved_through_v24() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();

        // Apply migrations v1..v23 (stop before v24).
        for m in TENANT_MIGRATIONS.iter() {
            if m.version >= 24 {
                break;
            }
            (m.up)(&conn).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
                rusqlite::params![m.version],
            )
            .unwrap();
        }

        // Insert a v3-shaped row carrying user_id.
        conn.execute(
            "INSERT INTO sessions (id, agent, user_id, status) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["sess-pre", "gir", 1_i64, "running"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_output (session_id, line) VALUES (?1, ?2)",
            rusqlite::params!["sess-pre", "first"],
        )
        .unwrap();

        // Apply v24.
        apply_schema_v24_sessions_drop(&conn).unwrap();

        // Row still present with every non-user_id field intact.
        let (id, agent, status): (String, String, String) = conn
            .query_row(
                "SELECT id, agent, status FROM sessions WHERE id = ?1",
                rusqlite::params!["sess-pre"],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(id, "sess-pre");
        assert_eq!(agent, "gir");
        assert_eq!(status, "running");

        // session_output row survived.
        let line: String = conn
            .query_row(
                "SELECT line FROM session_output WHERE session_id = ?1",
                rusqlite::params!["sess-pre"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(line, "first");

        // user_id column is gone.
        let col_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(col_count, 0, "user_id column must be absent after v24");
    }

    #[test]
    fn chiasm_tasks_usable_after_v4() {
        // v4 introduced the chiasm tables with a user_id shim; v25 drops
        // that shim. Cap the chain at v24 so this test still locks the
        // v4 shape.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        for m in TENANT_MIGRATIONS.iter() {
            if m.version >= 25 {
                break;
            }
            (m.up)(&conn).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
                rusqlite::params![m.version],
            )
            .unwrap();
        }

        // Both tables exist.
        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('chiasm_tasks', 'chiasm_task_updates')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            tables, 2,
            "chiasm_tasks and/or chiasm_task_updates missing after v4"
        );

        // Exercise the SQL shape kleos-lib chiasm.rs used pre-v25.
        conn.execute(
            "INSERT INTO chiasm_tasks (agent, project, title, status, summary, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                "claude-code",
                "engram-rust",
                "Phase 3.4",
                "active",
                None::<String>,
                4_i64
            ],
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

    /// v25: chiasm_tasks and chiasm_task_updates must NOT have a user_id
    /// column after the full migration chain completes.
    #[test]
    fn user_id_absent_from_chiasm_after_v25() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        for table in &["chiasm_tasks", "chiasm_task_updates"] {
            let count: i64 = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM pragma_table_info('{}') WHERE name='user_id'",
                        table
                    ),
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            assert_eq!(
                count, 0,
                "table '{}' still has user_id column after v25",
                table
            );
        }

        // idx_chiasm_tasks_user is gone.
        let idx: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_chiasm_tasks_user'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(idx, 0);
    }

    /// v25: the post-drop chiasm tables support the SQL shape kleos-lib
    /// services/chiasm.rs now uses (no user_id on INSERT, no user_id
    /// predicate on SELECT/UPDATE/DELETE). FK cascade from
    /// chiasm_tasks.id to chiasm_task_updates.task_id still works.
    #[test]
    fn chiasm_usable_after_v25() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

        conn.execute(
            "INSERT INTO chiasm_tasks (agent, project, title, status, summary) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params!["gir", "engram", "t1", "active", None::<String>],
        )
        .unwrap();
        let task_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO chiasm_task_updates (task_id, agent, status, summary) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![task_id, "gir", "active", "started"],
        )
        .unwrap();

        let (agent, project): (String, String) = conn
            .query_row(
                "SELECT agent, project FROM chiasm_tasks WHERE id = ?1",
                rusqlite::params![task_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(agent, "gir");
        assert_eq!(project, "engram");

        let update_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chiasm_task_updates WHERE task_id = ?1",
                rusqlite::params![task_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(update_count, 1);

        // FK cascade: delete the task, the update row goes with it.
        conn.execute(
            "DELETE FROM chiasm_tasks WHERE id = ?1",
            rusqlite::params![task_id],
        )
        .unwrap();
        let leftover: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chiasm_task_updates",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(leftover, 0, "FK cascade broken after v25");
    }

    /// v25: rows inserted under the v4 shim shape survive the drop with
    /// every non-user_id field intact on both chiasm_tasks and
    /// chiasm_task_updates.
    #[test]
    fn chiasm_rows_preserved_through_v25() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();

        for m in TENANT_MIGRATIONS.iter() {
            if m.version >= 25 {
                break;
            }
            (m.up)(&conn).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
                rusqlite::params![m.version],
            )
            .unwrap();
        }

        // Insert v4-shaped rows carrying user_id.
        conn.execute(
            "INSERT INTO chiasm_tasks (agent, project, title, status, summary, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params!["gir", "engram", "phase 5.4", "active", Some("shipping"), 1_i64],
        )
        .unwrap();
        let task_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO chiasm_task_updates (task_id, agent, status, summary, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![task_id, "gir", "active", "first update", 1_i64],
        )
        .unwrap();

        // Apply v25.
        apply_schema_v25_chiasm_drop(&conn).unwrap();

        let (agent, project, title, status, summary): (
            String,
            String,
            String,
            String,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT agent, project, title, status, summary FROM chiasm_tasks WHERE id = ?1",
                rusqlite::params![task_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(agent, "gir");
        assert_eq!(project, "engram");
        assert_eq!(title, "phase 5.4");
        assert_eq!(status, "active");
        assert_eq!(summary.as_deref(), Some("shipping"));

        let (upd_agent, upd_status, upd_summary): (String, String, Option<String>) = conn
            .query_row(
                "SELECT agent, status, summary FROM chiasm_task_updates WHERE task_id = ?1",
                rusqlite::params![task_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(upd_agent, "gir");
        assert_eq!(upd_status, "active");
        assert_eq!(upd_summary.as_deref(), Some("first update"));

        for table in &["chiasm_tasks", "chiasm_task_updates"] {
            let col_count: i64 = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM pragma_table_info('{}') WHERE name='user_id'",
                        table
                    ),
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(col_count, 0, "{} still has user_id after v25", table);
        }
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
        // v5 introduced approvals with a user_id shim; v26 drops it.
        // Cap the chain at v25 so this test still locks the v5 shape.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        for m in TENANT_MIGRATIONS.iter() {
            if m.version >= 26 {
                break;
            }
            (m.up)(&conn).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
                rusqlite::params![m.version],
            )
            .unwrap();
        }

        let table: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='approvals'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(table, 1, "approvals table missing after v5");

        // Exercise the SQL shape kleos-lib approvals/mod.rs used pre-v26.
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

    /// v26: approvals must NOT have a user_id column after the full chain.
    #[test]
    fn user_id_absent_from_approvals_after_v26() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('approvals') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        assert_eq!(count, 0, "approvals still has user_id column after v26");

        // Both shim indexes are gone.
        for idx in &["idx_approvals_user", "idx_approvals_user_status"] {
            let count: i64 = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='{}'", idx),
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 0, "index '{}' still present after v26", idx);
        }
    }

    /// v26: the post-drop approvals table supports the SQL shape kleos-lib
    /// approvals/mod.rs now uses (no user_id on INSERT, no user_id
    /// predicate on SELECT/UPDATE).
    #[test]
    fn approvals_usable_after_v26() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        conn.execute(
            "INSERT INTO approvals (id, action, context, requester, status, created_at, expires_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                "appr-v26",
                "run tacos",
                None::<String>,
                "gir",
                "pending",
                "2026-04-22T00:00:00Z",
                "2026-04-22T00:02:00Z",
            ],
        )
        .unwrap();

        let (id, status): (String, String) = conn
            .query_row(
                "SELECT id, status FROM approvals WHERE id = ?1",
                rusqlite::params!["appr-v26"],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(id, "appr-v26");
        assert_eq!(status, "pending");

        // UPDATE without user_id predicate also works.
        conn.execute(
            "UPDATE approvals SET status = 'approved' WHERE id = ?1",
            rusqlite::params!["appr-v26"],
        )
        .unwrap();
        let status: String = conn
            .query_row(
                "SELECT status FROM approvals WHERE id = ?1",
                rusqlite::params!["appr-v26"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "approved");
    }

    /// v26: rows inserted under the v5 shim shape survive the drop with
    /// every non-user_id field intact.
    #[test]
    fn approvals_rows_preserved_through_v26() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        for m in TENANT_MIGRATIONS.iter() {
            if m.version >= 26 {
                break;
            }
            (m.up)(&conn).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
                rusqlite::params![m.version],
            )
            .unwrap();
        }

        conn.execute(
            "INSERT INTO approvals (id, action, context, requester, status, created_at, expires_at, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                "appr-pre",
                "ship 5.5",
                Some("{\"ctx\": true}"),
                "gir",
                "pending",
                "2026-04-22T00:00:00Z",
                "2026-04-22T00:05:00Z",
                1_i64,
            ],
        )
        .unwrap();

        apply_schema_v26_approvals_drop(&conn).unwrap();

        let (id, action, context, requester, status): (
            String,
            String,
            Option<String>,
            String,
            String,
        ) = conn
            .query_row(
                "SELECT id, action, context, requester, status FROM approvals WHERE id = ?1",
                rusqlite::params!["appr-pre"],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(id, "appr-pre");
        assert_eq!(action, "ship 5.5");
        assert_eq!(context.as_deref(), Some("{\"ctx\": true}"));
        assert_eq!(requester, "gir");
        assert_eq!(status, "pending");

        let col_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('approvals') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(col_count, 0);
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
        // v6 introduced broca_actions with a user_id shim; v27 drops it.
        // Cap the chain at v26 so this test still locks the v6 shape.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        for m in TENANT_MIGRATIONS.iter() {
            if m.version >= 27 {
                break;
            }
            (m.up)(&conn).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
                rusqlite::params![m.version],
            )
            .unwrap();
        }

        let table: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='broca_actions'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(table, 1, "broca_actions table missing after v6");

        // Exercise the INSERT shape kleos-lib services/broca.rs used pre-v27.
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

    /// v27: broca_actions must NOT have a user_id column after the chain.
    #[test]
    fn user_id_absent_from_broca_after_v27() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('broca_actions') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        assert_eq!(count, 0, "broca_actions still has user_id column after v27");

        let idx: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_broca_actions_user'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(idx, 0);
    }

    /// v27: broca_actions supports the SQL shape kleos-lib services/broca.rs
    /// now uses (no user_id on INSERT, no user_id predicate on SELECT).
    #[test]
    fn broca_actions_usable_after_v27() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        conn.execute(
            "INSERT INTO broca_actions (agent, service, action, payload, narrative, axon_event_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                "gir",
                "tacos",
                "bake",
                r#"{"temp":"molten"}"#,
                None::<String>,
                None::<i64>,
            ],
        )
        .unwrap();

        let (agent, service): (String, String) = conn
            .query_row(
                "SELECT agent, service FROM broca_actions ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(agent, "gir");
        assert_eq!(service, "tacos");

        // Per-agent index still covers the ordered query.
        let agent_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM broca_actions WHERE agent = ?1",
                rusqlite::params!["gir"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(agent_count, 1);
    }

    /// v27: rows inserted under the v6 shim shape survive the drop with
    /// every non-user_id field intact.
    #[test]
    fn broca_actions_rows_preserved_through_v27() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        for m in TENANT_MIGRATIONS.iter() {
            if m.version >= 27 {
                break;
            }
            (m.up)(&conn).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
                rusqlite::params![m.version],
            )
            .unwrap();
        }

        conn.execute(
            "INSERT INTO broca_actions (agent, service, action, payload, narrative, axon_event_id, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                "gir",
                "engram",
                "ship-5.6",
                "{\"batch\":\"5.3-5.6\"}",
                Some("tacos"),
                None::<i64>,
                1_i64,
            ],
        )
        .unwrap();
        let pre_id = conn.last_insert_rowid();

        apply_schema_v27_broca_drop(&conn).unwrap();

        let (agent, service, action, payload, narrative): (
            String,
            String,
            String,
            String,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT agent, service, action, payload, narrative FROM broca_actions WHERE id = ?1",
                rusqlite::params![pre_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(agent, "gir");
        assert_eq!(service, "engram");
        assert_eq!(action, "ship-5.6");
        assert_eq!(payload, "{\"batch\":\"5.3-5.6\"}");
        assert_eq!(narrative.as_deref(), Some("tacos"));

        let col_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('broca_actions') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(col_count, 0);
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
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        // Cap the chain before v28 so the shim shape (with user_id) is the
        // one under test. After v28 lands, projects.user_id is gone and the
        // INSERT + SELECT shape below no longer applies.
        for m in TENANT_MIGRATIONS.iter() {
            if m.version >= 28 {
                break;
            }
            (m.up)(&conn).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
                rusqlite::params![m.version],
            )
            .unwrap();
        }

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
            "INSERT INTO memories (content, category, source) VALUES (?1, ?2, ?3)",
            rusqlite::params!["seed", "general", "test"],
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

    /// v28: user_id column and idx_projects_user are gone after the drop.
    #[test]
    fn user_id_absent_from_projects_after_v28() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        let col_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('projects') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(col_count, 0, "projects.user_id still present after v28");

        let idx_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_projects_user'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(idx_count, 0, "idx_projects_user still present after v28");

        // memory_projects survives the rebuild and its FK to projects(id)
        // still resolves.
        let mp: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='memory_projects'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(mp, 1);
    }

    /// v28: INSERT + SELECT without user_id works, and the memory_projects
    /// FK cascade on project deletion still fires (FK was preserved across
    /// the rebuild via legacy_alter_table=1).
    #[test]
    fn projects_usable_after_v28() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();

        conn.execute(
            "INSERT INTO memories (content, category, source) VALUES (?1, ?2, ?3)",
            rusqlite::params!["seed", "general", "test"],
        )
        .unwrap();
        let memory_id = conn.last_insert_rowid();

        let (project_id, _created_at): (i64, String) = conn
            .query_row(
                "INSERT INTO projects (name, description, status, metadata) \
                 VALUES (?1, ?2, ?3, ?4) RETURNING id, created_at",
                rusqlite::params!["p1", None::<String>, "active", None::<String>],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();

        conn.execute(
            "INSERT OR IGNORE INTO memory_projects (memory_id, project_id) VALUES (?1, ?2)",
            rusqlite::params![memory_id, project_id],
        )
        .unwrap();

        // UNIQUE(name) enforced: second insert with same name fails.
        let dup = conn.execute(
            "INSERT INTO projects (name, description, status, metadata) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["p1", None::<String>, "active", None::<String>],
        );
        assert!(dup.is_err(), "UNIQUE(name) should reject duplicate names");

        // FK cascade: deleting the project removes the memory_projects row.
        conn.execute(
            "DELETE FROM projects WHERE id = ?1",
            rusqlite::params![project_id],
        )
        .unwrap();
        let linked: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memory_projects WHERE project_id = ?1",
                rusqlite::params![project_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(linked, 0, "memory_projects FK cascade did not fire");
    }

    /// v28: rows inserted under the v7 shim shape survive the rebuild with
    /// every non-user_id field intact.
    #[test]
    fn projects_rows_preserved_through_v28() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        for m in TENANT_MIGRATIONS.iter() {
            if m.version >= 28 {
                break;
            }
            (m.up)(&conn).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
                rusqlite::params![m.version],
            )
            .unwrap();
        }

        conn.execute(
            "INSERT INTO projects (name, description, status, metadata, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                "alpha",
                Some("the first"),
                "active",
                Some("{\"k\":1}"),
                1_i64,
            ],
        )
        .unwrap();
        let pre_id = conn.last_insert_rowid();

        apply_schema_v28_projects_drop(&conn).unwrap();

        let (name, description, status, metadata): (
            String,
            Option<String>,
            String,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT name, description, status, metadata FROM projects WHERE id = ?1",
                rusqlite::params![pre_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(name, "alpha");
        assert_eq!(description.as_deref(), Some("the first"));
        assert_eq!(status, "active");
        assert_eq!(metadata.as_deref(), Some("{\"k\":1}"));

        let col_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('projects') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(col_count, 0);
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
        // v8 introduced axon_events and soma_agents with user_id shims; v29
        // drops them. Cap the chain at v28 so this test still locks the v8
        // shape (with user_id still present).
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        for m in TENANT_MIGRATIONS.iter() {
            if m.version >= 29 {
                break;
            }
            (m.up)(&conn).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
                rusqlite::params![m.version],
            )
            .unwrap();
        }

        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('axon_events', 'soma_agents')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 2, "axon_events or soma_agents missing after v8");

        // Exercise the INSERT shape services/axon.rs and soma.rs used pre-v29.
        conn.execute(
            "INSERT INTO axon_events (channel, source, type, payload, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params!["agent.reports", "activity", "task.completed", "{}", 4_i64],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO soma_agents (name, type, description, capabilities, config, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params!["claude-code", "cli", None::<String>, "[]", "{}", 4_i64,],
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
        // v9 introduced webhooks with a user_id shim; v30 drops it.
        // Cap the chain at v29 so this test still locks the v9 shape.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        for m in TENANT_MIGRATIONS.iter() {
            if m.version >= 30 {
                break;
            }
            (m.up)(&conn).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
                rusqlite::params![m.version],
            )
            .unwrap();
        }

        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('webhooks', 'webhook_dead_letters')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 2, "webhooks/webhook_dead_letters missing after v9");

        // Exercise the INSERT shape webhooks.rs used pre-v30.
        conn.execute(
            "INSERT INTO webhooks (user_id, url, events, secret) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                4_i64,
                "https://example.test/hook",
                "memory.created",
                None::<String>
            ],
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
        // v10 introduced upload_sessions, upload_chunks, and ingestion_hashes
        // with user_id shims; v33 drops user_id from ingestion_hashes. Cap the
        // chain at v32 so this test still locks the v10 shape (user_id present).
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        for m in TENANT_MIGRATIONS.iter() {
            if m.version >= 33 {
                break;
            }
            (m.up)(&conn).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
                rusqlite::params![m.version],
            )
            .unwrap();
        }

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
        // v11 introduced axon_channels, axon_subscriptions, axon_cursors with
        // user_id shims; v31 drops them. Cap the chain at v30 so this test
        // still locks the v11 shape (with user_id still present).
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        for m in TENANT_MIGRATIONS.iter() {
            if m.version >= 31 {
                break;
            }
            (m.up)(&conn).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
                rusqlite::params![m.version],
            )
            .unwrap();
        }

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

        // Exercise the INSERT shape services/axon.rs used pre-v31.
        conn.execute(
            "INSERT INTO axon_subscriptions (agent, channel, filter_type, webhook_url, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                "claude-code",
                "system",
                None::<String>,
                None::<String>,
                4_i64
            ],
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
        // v12 added soma_groups/soma_agent_groups/soma_agent_logs. v29 drops
        // user_id from soma_agents; after the full chain user_id is absent from
        // soma_agents but soma_groups / soma_agent_logs retain their own columns.
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

        // Seed a soma_agents row (no user_id after v29).
        conn.execute(
            "INSERT INTO soma_agents (name, type, description, capabilities, config) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params!["claude-code", "cli", None::<String>, "[]", "{}"],
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
        // v34 drops user_id from loom_workflows and loom_runs. Cap this test
        // at v33 so it exercises the v13 shim shape before the column drop.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        for m in TENANT_MIGRATIONS.iter() {
            if m.version >= 34 {
                break;
            }
            (m.up)(&conn).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
                rusqlite::params![m.version],
            )
            .unwrap();
        }

        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' \
                 AND name IN ('loom_workflows', 'loom_runs', 'loom_steps', 'loom_run_logs')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 4, "loom family tables missing after v13");

        // Exercise INSERT shapes with user_id (v13 shim shape, before v34 drop).
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
            "INSERT INTO memories (content, category, source) VALUES (?1, ?2, ?3)",
            rusqlite::params!["seed memory", "general", "test"],
        )
        .unwrap();
        let memory_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO entities (name, entity_type, description, aliases, user_id, space_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                "alpha",
                "concept",
                None::<String>,
                None::<String>,
                4_i64,
                None::<i64>
            ],
        )
        .unwrap();
        let a_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO entities (name, entity_type, description, aliases, user_id, space_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                "beta",
                "concept",
                None::<String>,
                None::<String>,
                4_i64,
                None::<i64>
            ],
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
        assert_eq!(
            has_key, 1,
            "user_preferences missing 'key' column after v16"
        );

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
    fn reflections_usable_after_v17() {
        // v17 introduced reflections with a user_id shim; v32 drops it.
        // Cap the chain at v31 so this test still locks the v17 shape.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        for m in TENANT_MIGRATIONS.iter() {
            if m.version >= 32 {
                break;
            }
            (m.up)(&conn).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
                rusqlite::params![m.version],
            )
            .unwrap();
        }

        let table: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='reflections'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(table, 1, "reflections table missing after v17");

        // Exercise the INSERT shape intelligence/reflections.rs used pre-v32.
        conn.execute(
            "INSERT INTO reflections \
             (content, reflection_type, themes, source_memory_ids, confidence, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                "growth observation content",
                "pattern",
                Some("[\"repetition\"]"),
                Some("[42, 43]"),
                0.75_f64,
                4_i64,
            ],
        )
        .unwrap();

        let (content, uid): (String, i64) = conn
            .query_row(
                "SELECT content, user_id FROM reflections WHERE user_id = ?1",
                rusqlite::params![4_i64],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(content, "growth observation content");
        assert_eq!(uid, 4);
    }

    #[test]
    fn intelligence_family_usable_after_v18() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' \
                 AND name IN ('consolidations', 'current_state', 'causal_chains', \
                              'causal_links', 'reconsolidations', 'temporal_patterns', \
                              'digests', 'memory_feedback')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 8, "intelligence family tables missing after v18");

        conn.execute(
            "INSERT INTO memories (content, category, source) VALUES (?1, ?2, ?3)",
            rusqlite::params!["seed", "general", "test"],
        )
        .unwrap();
        let mid = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO consolidations (source_ids, strategy, confidence, user_id) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["[1,2,3]", "merge", 0.9_f64, 4_i64],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO current_state (agent, key, value, user_id) VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(agent, key, user_id) DO UPDATE SET value = excluded.value",
            rusqlite::params!["claude", "location", "home", 4_i64],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO causal_chains (root_memory_id, description, user_id) VALUES (?1, ?2, ?3)",
            rusqlite::params![mid, "chain", 4_i64],
        )
        .unwrap();
        let chain_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO causal_links (chain_id, cause_memory_id, effect_memory_id) \
             VALUES (?1, ?2, ?2)",
            rusqlite::params![chain_id, mid],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO reconsolidations (memory_id, old_content, new_content, user_id) \
             VALUES (?1, 'old', 'new', ?2)",
            rusqlite::params![mid, 4_i64],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO temporal_patterns (pattern_type, description, user_id) VALUES (?1, ?2, ?3)",
            rusqlite::params!["daily", "morning routine", 4_i64],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO digests (period, content, memory_count, user_id) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["daily", "digest body", 10_i64, 4_i64],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO memory_feedback (memory_id, user_id, rating) VALUES (?1, ?2, ?3)",
            rusqlite::params![mid, 4_i64, "up"],
        )
        .unwrap();

        let (c, s, cc, cl, r, tp, d, f): (i64, i64, i64, i64, i64, i64, i64, i64) = conn
            .query_row(
                "SELECT \
                   (SELECT COUNT(*) FROM consolidations WHERE user_id = ?1), \
                   (SELECT COUNT(*) FROM current_state WHERE user_id = ?1), \
                   (SELECT COUNT(*) FROM causal_chains WHERE user_id = ?1), \
                   (SELECT COUNT(*) FROM causal_links WHERE chain_id = ?2), \
                   (SELECT COUNT(*) FROM reconsolidations WHERE user_id = ?1), \
                   (SELECT COUNT(*) FROM temporal_patterns WHERE user_id = ?1), \
                   (SELECT COUNT(*) FROM digests WHERE user_id = ?1), \
                   (SELECT COUNT(*) FROM memory_feedback WHERE user_id = ?1)",
                rusqlite::params![4_i64, chain_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!((c, s, cc, cl, r, tp, d, f), (1, 1, 1, 1, 1, 1, 1, 1));
    }

    #[test]
    fn skills_family_usable_after_v19() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' \
                 AND name IN ('skill_records', 'skill_lineage_parents', 'skill_tags', \
                              'execution_analyses', 'skill_judgments', 'skill_tool_deps', \
                              'tool_quality_records')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 7, "skills family tables missing after v19");

        let fts_present: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='skills_fts'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(fts_present >= 1, "skills_fts FTS5 virtual table missing");

        conn.execute(
            "INSERT INTO skill_records (name, agent, code, user_id) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["brew-coffee", "claude", "# coffee recipe", 4_i64],
        )
        .unwrap();
        let sid = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO skill_tags (skill_id, tag) VALUES (?1, ?2)",
            rusqlite::params![sid, "food"],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO skill_tool_deps (skill_id, tool_name, is_optional) VALUES (?1, ?2, 0)",
            rusqlite::params![sid, "kettle"],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO execution_analyses (skill_id, success, duration_ms) VALUES (?1, 1, 42.0)",
            rusqlite::params![sid],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO skill_judgments (skill_id, judge_agent, score) VALUES (?1, ?2, ?3)",
            rusqlite::params![sid, "gir", 0.8_f64],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO tool_quality_records (tool_name, agent, success) VALUES (?1, ?2, ?3)",
            rusqlite::params!["kettle", "claude", 1_i64],
        )
        .unwrap();

        // FTS trigger should have populated skills_fts with the new row.
        let fts_hits: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM skills_fts WHERE skills_fts MATCH 'coffee'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fts_hits, 1, "skills_fts insert trigger did not fire");

        let (s, t, d, e, j, tq): (i64, i64, i64, i64, i64, i64) = conn
            .query_row(
                "SELECT \
                   (SELECT COUNT(*) FROM skill_records WHERE user_id = ?1), \
                   (SELECT COUNT(*) FROM skill_tags WHERE skill_id = ?2), \
                   (SELECT COUNT(*) FROM skill_tool_deps WHERE skill_id = ?2), \
                   (SELECT COUNT(*) FROM execution_analyses WHERE skill_id = ?2), \
                   (SELECT COUNT(*) FROM skill_judgments WHERE skill_id = ?2), \
                   (SELECT COUNT(*) FROM tool_quality_records WHERE tool_name = 'kettle')",
                rusqlite::params![4_i64, sid],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!((s, t, d, e, j, tq), (1, 1, 1, 1, 1, 1));
    }

    #[test]
    fn episodes_user_id_and_fts_after_v20() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        // episodes now carries user_id.
        let has_user_id: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('episodes') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has_user_id, 1);

        // episodes_fts FTS5 virtual table is present.
        let fts_present: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='episodes_fts'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(fts_present >= 1);

        // Insert exercises kleos_lib::episodes create path shape.
        conn.execute(
            "INSERT INTO episodes (title, session_id, agent, summary, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params!["morning", "sess-1", "claude", "coffee routine", 4_i64],
        )
        .unwrap();

        // Trigger should have synced the FTS index.
        let fts_hits: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM episodes_fts WHERE episodes_fts MATCH 'coffee'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fts_hits, 1, "episodes_fts insert trigger did not fire");
    }

    #[test]
    fn messages_and_fts_usable_after_v21() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        let has_messages: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='messages'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has_messages, 1);

        let has_fts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='messages_fts'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(has_fts >= 1);

        // Need a parent conversation row (added in v16).
        conn.execute(
            "INSERT INTO conversations (agent, session_id, title, metadata, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params!["claude", "sess-1", None::<String>, None::<String>, 4_i64,],
        )
        .unwrap();
        let conv_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO messages (conversation_id, role, content, metadata) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![conv_id, "user", "hello world", None::<String>],
        )
        .unwrap();

        let fts_hits: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages_fts WHERE messages_fts MATCH 'hello'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fts_hits, 1, "messages_fts insert trigger did not fire");
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

        // Run chain; v3 adds the shim, v24 later drops it. End state: absent.
        run_tenant_migrations(&conn).unwrap();

        let post_user: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(post_user, 0);

        let post_output: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='session_output'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(post_output, 1);
    }

    /// v22: memories, artifacts, and vector_sync_pending must NOT have a
    /// user_id column after the full migration chain completes.
    #[test]
    fn user_id_absent_from_memories_after_v22() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        for table in &["memories", "artifacts", "vector_sync_pending"] {
            let count: i64 = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM pragma_table_info('{}') WHERE name='user_id'",
                        table
                    ),
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            assert_eq!(
                count, 0,
                "table '{}' still has user_id column after v22",
                table
            );
        }
    }

    /// v22: insert a memory without user_id and verify it can be FTS-matched.
    #[test]
    fn memories_constraint_reshaped_after_v22() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        conn.execute(
            "INSERT INTO memories (content, category, source, importance, confidence, \
             created_at, updated_at, is_latest, is_forgotten, is_archived) \
             VALUES ('thetestword unique phrase', 'general', 'test', 5, 1.0, \
             datetime('now'), datetime('now'), 1, 0, 0)",
            [],
        )
        .unwrap();

        let hit: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memories_fts WHERE memories_fts MATCH 'thetestword'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hit, 1, "FTS trigger must fire and index the new memory");
    }

    /// v29: axon_events and soma_agents must NOT have a user_id column after
    /// the full migration chain completes, and their user-indexes must be gone.
    #[test]
    fn user_id_absent_from_activity_after_v29() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        for table in &["axon_events", "soma_agents"] {
            let count: i64 = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM pragma_table_info('{}') WHERE name='user_id'",
                        table
                    ),
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            assert_eq!(
                count, 0,
                "table '{}' still has user_id column after v29",
                table
            );
        }

        for idx_name in &["idx_axon_events_user", "idx_soma_agents_user"] {
            let idx: i64 = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='{}'",
                        idx_name
                    ),
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(idx, 0, "index '{}' still exists after v29", idx_name);
        }
    }

    /// v29: axon_events and soma_agents support the SQL shape services/axon.rs
    /// and services/soma.rs now use (no user_id on INSERT or SELECT).
    #[test]
    fn activity_tables_usable_after_v29() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        conn.execute(
            "INSERT INTO axon_events (channel, source, type, payload) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["agent.reports", "activity", "task.completed", "{}"],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO soma_agents (name, type, capabilities, config) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["claude-code", "cli", "[]", "{}"],
        )
        .unwrap();

        let (event_count, agent_count): (i64, i64) = conn
            .query_row(
                "SELECT \
                   (SELECT COUNT(*) FROM axon_events WHERE channel = 'agent.reports'), \
                   (SELECT COUNT(*) FROM soma_agents WHERE name = 'claude-code')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(event_count, 1);
        assert_eq!(agent_count, 1);
    }

    /// v29: rows inserted under the v8 shim shape survive the drop with
    /// every non-user_id field intact on both tables.
    #[test]
    fn activity_rows_preserved_through_v29() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        for m in TENANT_MIGRATIONS.iter() {
            if m.version >= 29 {
                break;
            }
            (m.up)(&conn).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
                rusqlite::params![m.version],
            )
            .unwrap();
        }

        conn.execute(
            "INSERT INTO axon_events (channel, source, type, payload, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params!["sys", "gir", "taco.baked", "{}", 1_i64],
        )
        .unwrap();
        let event_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO soma_agents (name, type, capabilities, config, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params!["gir-unit", "sir", "[]", "{}", 1_i64],
        )
        .unwrap();
        let agent_id = conn.last_insert_rowid();

        apply_schema_v29_activity_drop(&conn).unwrap();

        let (channel, type_): (String, String) = conn
            .query_row(
                "SELECT channel, type FROM axon_events WHERE id = ?1",
                rusqlite::params![event_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(channel, "sys");
        assert_eq!(type_, "taco.baked");

        let (name, type2): (String, String) = conn
            .query_row(
                "SELECT name, type FROM soma_agents WHERE id = ?1",
                rusqlite::params![agent_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(name, "gir-unit");
        assert_eq!(type2, "sir");

        for table in &["axon_events", "soma_agents"] {
            let col: i64 = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM pragma_table_info('{}') WHERE name='user_id'",
                        table
                    ),
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(col, 0, "'{}' still has user_id after v29", table);
        }
    }

    /// v22: rows inserted before v22 survive the DROP COLUMN migration intact.
    #[test]
    fn memories_rows_preserved_through_v22() {
        let conn = Connection::open_in_memory().unwrap();

        // Bootstrap the schema_migrations table manually so we can stop at v21.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();

        // Apply migrations v1..v21 (stop before v22).
        for m in TENANT_MIGRATIONS.iter() {
            if m.version >= 22 {
                break;
            }
            (m.up)(&conn).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
                rusqlite::params![m.version],
            )
            .unwrap();
        }

        // Insert a memory row with user_id while the column still exists.
        conn.execute(
            "INSERT INTO memories (content, category, source, importance, confidence, \
             user_id, created_at, updated_at, is_latest, is_forgotten, is_archived) \
             VALUES ('pre-migration content', 'general', 'test', 5, 1.0, \
             1, datetime('now'), datetime('now'), 1, 0, 0)",
            [],
        )
        .unwrap();

        let pre_id: i64 = conn
            .query_row("SELECT last_insert_rowid()", [], |r| r.get(0))
            .unwrap();
        assert!(pre_id > 0);

        // Now apply v22.
        apply_schema_v22_memories_drop(&conn).unwrap();

        // Row must still exist.
        let content: String = conn
            .query_row(
                "SELECT content FROM memories WHERE id = ?1",
                rusqlite::params![pre_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(content, "pre-migration content");

        // user_id column must be gone.
        let col_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('memories') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(col_count, 0, "user_id column must be absent after v22");
    }

    /// v30: webhooks must NOT have a user_id column after the full migration
    /// chain, and idx_webhooks_user must be gone.
    #[test]
    fn user_id_absent_from_webhooks_after_v30() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('webhooks') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        assert_eq!(count, 0, "webhooks still has user_id column after v30");

        let idx: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_webhooks_user'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(idx, 0, "idx_webhooks_user still exists after v30");
    }

    /// v30: webhooks supports the SQL shape kleos-lib/src/webhooks.rs now
    /// uses (no user_id on INSERT or SELECT).
    #[test]
    fn webhooks_usable_after_v30() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        conn.execute(
            "INSERT INTO webhooks (url, events) VALUES (?1, ?2)",
            rusqlite::params!["https://hooks.test/v30", "[\"memory.created\"]"],
        )
        .unwrap();
        let wid = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO webhook_dead_letters (webhook_id, event, payload, attempts) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![wid, "memory.created", "{}", 1_i64],
        )
        .unwrap();

        let url: String = conn
            .query_row(
                "SELECT url FROM webhooks WHERE id = ?1",
                rusqlite::params![wid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(url, "https://hooks.test/v30");

        let dl: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM webhook_dead_letters WHERE webhook_id = ?1",
                rusqlite::params![wid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dl, 1);

        // idx_webhooks_active survives the drop.
        let idx: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_webhooks_active'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(idx, 1, "idx_webhooks_active must survive v30");
    }

    /// v30: rows inserted under the v9 shim shape survive the drop with
    /// every non-user_id field intact.
    #[test]
    fn webhooks_rows_preserved_through_v30() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        for m in TENANT_MIGRATIONS.iter() {
            if m.version >= 30 {
                break;
            }
            (m.up)(&conn).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
                rusqlite::params![m.version],
            )
            .unwrap();
        }

        conn.execute(
            "INSERT INTO webhooks (url, events, secret, user_id) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                "https://preserve.test/hook",
                "[\"memory.created\"]",
                None::<String>,
                1_i64,
            ],
        )
        .unwrap();
        let pre_id = conn.last_insert_rowid();

        apply_schema_v30_webhooks_drop(&conn).unwrap();

        let (url, events): (String, String) = conn
            .query_row(
                "SELECT url, events FROM webhooks WHERE id = ?1",
                rusqlite::params![pre_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(url, "https://preserve.test/hook");
        assert_eq!(events, "[\"memory.created\"]");

        let col: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('webhooks') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(col, 0, "webhooks still has user_id after v30");
    }

    /// v31: axon_subscriptions and axon_cursors must NOT have a user_id column
    /// after the full migration chain.
    #[test]
    fn user_id_absent_from_axon_after_v31() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        for table in &["axon_subscriptions", "axon_cursors"] {
            let count: i64 = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM pragma_table_info('{}') WHERE name='user_id'",
                        table
                    ),
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            assert_eq!(
                count, 0,
                "table '{}' still has user_id column after v31",
                table
            );
        }

        // UNIQUE(agent, channel) and PRIMARY KEY(agent, channel) survive.
        let idx: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_axon_subs_channel'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(idx, 1, "idx_axon_subs_channel must survive v31");
    }

    /// v31: axon_subscriptions and axon_cursors support the SQL shape
    /// services/axon.rs now uses (no user_id on INSERT or SELECT).
    #[test]
    fn axon_tables_usable_after_v31() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        conn.execute(
            "INSERT INTO axon_subscriptions (agent, channel) VALUES (?1, ?2)",
            rusqlite::params!["gir", "taco.channel"],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO axon_cursors (agent, channel, last_event_id) VALUES (?1, ?2, ?3)",
            rusqlite::params!["gir", "taco.channel", 42_i64],
        )
        .unwrap();

        let (agent, channel): (String, String) = conn
            .query_row(
                "SELECT agent, channel FROM axon_subscriptions ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(agent, "gir");
        assert_eq!(channel, "taco.channel");

        let last_id: i64 = conn
            .query_row(
                "SELECT last_event_id FROM axon_cursors WHERE agent = ?1 AND channel = ?2",
                rusqlite::params!["gir", "taco.channel"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(last_id, 42);
    }

    /// v31: rows inserted under the v11 shim shape survive the drop with
    /// every non-user_id field intact.
    #[test]
    fn axon_rows_preserved_through_v31() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        for m in TENANT_MIGRATIONS.iter() {
            if m.version >= 31 {
                break;
            }
            (m.up)(&conn).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
                rusqlite::params![m.version],
            )
            .unwrap();
        }

        conn.execute(
            "INSERT INTO axon_subscriptions (agent, channel, filter_type, webhook_url, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params!["gir", "ship.channel", None::<String>, None::<String>, 1_i64],
        )
        .unwrap();
        let sub_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO axon_cursors (agent, channel, last_event_id, user_id) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["gir", "ship.channel", 99_i64, 1_i64],
        )
        .unwrap();

        apply_schema_v31_axon_drop(&conn).unwrap();

        let (agent, channel): (String, String) = conn
            .query_row(
                "SELECT agent, channel FROM axon_subscriptions WHERE id = ?1",
                rusqlite::params![sub_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(agent, "gir");
        assert_eq!(channel, "ship.channel");

        let last_id: i64 = conn
            .query_row(
                "SELECT last_event_id FROM axon_cursors WHERE agent = ?1 AND channel = ?2",
                rusqlite::params!["gir", "ship.channel"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(last_id, 99);

        for table in &["axon_subscriptions", "axon_cursors"] {
            let col: i64 = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM pragma_table_info('{}') WHERE name='user_id'",
                        table
                    ),
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(col, 0, "'{}' still has user_id after v31", table);
        }
    }

    /// v32: reflections must NOT have a user_id column after the full
    /// migration chain, and idx_reflections_user must be gone.
    #[test]
    fn user_id_absent_from_reflections_after_v32() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('reflections') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        assert_eq!(count, 0, "reflections still has user_id column after v32");

        let idx: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_reflections_user'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(idx, 0, "idx_reflections_user still exists after v32");

        // idx_reflections_type and idx_reflections_period survive.
        for surviving in &["idx_reflections_type", "idx_reflections_period"] {
            let n: i64 = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='{}'",
                        surviving
                    ),
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "'{}' must survive v32", surviving);
        }
    }

    /// v32: reflections supports the SQL shape intelligence/reflections.rs
    /// now uses (no user_id on INSERT or SELECT).
    #[test]
    fn reflections_usable_after_v32() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        conn.execute(
            "INSERT INTO reflections (content, reflection_type, source_memory_ids, confidence) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["taco observation", "insight", "[1,2]", 0.9_f64],
        )
        .unwrap();

        let (content, rtype): (String, String) = conn
            .query_row(
                "SELECT content, reflection_type FROM reflections ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(content, "taco observation");
        assert_eq!(rtype, "insight");
    }

    /// v32: rows inserted under the v17 shim shape survive the drop with
    /// every non-user_id field intact.
    #[test]
    fn reflections_rows_preserved_through_v32() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        for m in TENANT_MIGRATIONS.iter() {
            if m.version >= 32 {
                break;
            }
            (m.up)(&conn).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
                rusqlite::params![m.version],
            )
            .unwrap();
        }

        conn.execute(
            "INSERT INTO reflections (content, reflection_type, source_memory_ids, confidence, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params!["pre-drop content", "enrich", "[7,8]", 0.8_f64, 1_i64],
        )
        .unwrap();
        let pre_id = conn.last_insert_rowid();

        apply_schema_v32_growth_drop(&conn).unwrap();

        let (content, rtype, confidence): (String, String, f64) = conn
            .query_row(
                "SELECT content, reflection_type, confidence FROM reflections WHERE id = ?1",
                rusqlite::params![pre_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(content, "pre-drop content");
        assert_eq!(rtype, "enrich");
        assert!((confidence - 0.8).abs() < 1e-9);

        let col: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('reflections') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(col, 0, "reflections still has user_id after v32");
    }

    /// v33: ingestion_hashes must NOT have a user_id column after the full
    /// migration chain, and no idx_ingestion_hashes_user index should exist.
    #[test]
    fn user_id_absent_from_ingestion_hashes_after_v33() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).expect("migrations");

        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('ingestion_hashes')")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(
            !cols.iter().any(|c| c == "user_id"),
            "user_id still present in ingestion_hashes: {:?}",
            cols
        );

        let idx_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_ingestion_hashes_user'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx_count, 0, "idx_ingestion_hashes_user still present");
    }

    /// v33: ingestion_hashes supports the SQL shape ingestion/mod.rs now uses
    /// (no user_id on INSERT or SELECT).
    #[test]
    fn ingestion_hashes_usable_after_v33() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).expect("migrations");

        conn.execute(
            "INSERT OR IGNORE INTO ingestion_hashes (sha256, job_id) VALUES (?1, ?2)",
            rusqlite::params!["abc123def456", "job-test-1"],
        )
        .expect("insert");

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM ingestion_hashes", [], |row| {
                row.get(0)
            })
            .expect("count");
        assert_eq!(count, 1);

        // Second insert of same sha256 must be silently ignored (PK dedup).
        conn.execute(
            "INSERT OR IGNORE INTO ingestion_hashes (sha256, job_id) VALUES (?1, ?2)",
            rusqlite::params!["abc123def456", "job-test-2"],
        )
        .expect("second insert or ignore");

        let count2: i64 = conn
            .query_row("SELECT COUNT(*) FROM ingestion_hashes", [], |row| {
                row.get(0)
            })
            .expect("count after dedup");
        assert_eq!(count2, 1, "duplicate sha256 should be deduped by PK");
    }

    /// v33: rows inserted under the v10 shim shape (with user_id) survive the
    /// PK rebuild with every non-user_id field intact.
    #[test]
    fn ingestion_hashes_rows_preserved_through_v33() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        for m in TENANT_MIGRATIONS.iter() {
            if m.version >= 33 {
                break;
            }
            (m.up)(&conn).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
                rusqlite::params![m.version],
            )
            .unwrap();
        }

        // Insert a row in the old shape (sha256, user_id, job_id).
        conn.execute(
            "INSERT OR IGNORE INTO ingestion_hashes (sha256, user_id, job_id) VALUES (?1, ?2, ?3)",
            rusqlite::params!["cafebabe", 1_i64, "job-pre-v33"],
        )
        .expect("insert old shape");

        // Apply v33 -- PK rebuild drops user_id.
        apply_schema_v33_ingestion_hashes_drop(&conn).expect("apply v33");

        // Verify the row survived with sha256 and job_id intact.
        let (sha256, job_id): (String, Option<String>) = conn
            .query_row(
                "SELECT sha256, job_id FROM ingestion_hashes WHERE sha256 = ?1",
                rusqlite::params!["cafebabe"],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("select after migration");
        assert_eq!(sha256, "cafebabe");
        assert_eq!(job_id.as_deref(), Some("job-pre-v33"));

        let col: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('ingestion_hashes') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(col, 0, "ingestion_hashes still has user_id after v33");
    }

    /// v34: loom_workflows and loom_runs must NOT have a user_id column after
    /// the full migration chain. idx_loom_workflows_user and
    /// idx_loom_runs_user must not exist.
    #[test]
    fn user_id_absent_from_loom_after_v34() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).expect("migrations");

        let wf_cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('loom_workflows')")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(
            !wf_cols.iter().any(|c| c == "user_id"),
            "user_id still present in loom_workflows: {:?}",
            wf_cols
        );

        let runs_cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('loom_runs')")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(
            !runs_cols.iter().any(|c| c == "user_id"),
            "user_id still present in loom_runs: {:?}",
            runs_cols
        );

        let wf_idx: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_loom_workflows_user'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(wf_idx, 0, "idx_loom_workflows_user still present");

        let runs_idx: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_loom_runs_user'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(runs_idx, 0, "idx_loom_runs_user still present");
    }

    /// v34: loom_workflows and loom_runs support the SQL shape loom.rs now uses
    /// (no user_id on INSERT or SELECT). A workflow and run can be inserted and
    /// queried without user_id.
    #[test]
    fn loom_usable_after_v34() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).expect("migrations");

        // Insert a workflow without user_id.
        conn.execute(
            "INSERT INTO loom_workflows (name, description, steps) VALUES (?1, ?2, ?3)",
            rusqlite::params!["test-wf", "desc", "[]"],
        )
        .expect("insert workflow");

        let wf_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM loom_workflows", [], |row| row.get(0))
            .expect("count workflows");
        assert_eq!(wf_count, 1);

        let wf_id: i64 = conn
            .query_row("SELECT id FROM loom_workflows WHERE name = ?1", rusqlite::params!["test-wf"], |r| r.get(0))
            .expect("get workflow id");

        // Insert a run referencing the workflow without user_id.
        conn.execute(
            "INSERT INTO loom_runs (workflow_id, status, input, output) VALUES (?1, 'pending', '{}', '{}')",
            rusqlite::params![wf_id],
        )
        .expect("insert run");

        let run_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM loom_runs", [], |row| row.get(0))
            .expect("count runs");
        assert_eq!(run_count, 1);

        // UNIQUE(name) on loom_workflows should reject a duplicate name.
        let dup = conn.execute(
            "INSERT INTO loom_workflows (name, description, steps) VALUES (?1, ?2, ?3)",
            rusqlite::params!["test-wf", "other", "[]"],
        );
        assert!(dup.is_err(), "duplicate workflow name should be rejected by UNIQUE(name)");
    }

    /// v34: rows inserted under the v13 shim shape (with user_id) survive the
    /// rebuild with every non-user_id field intact. The FK from loom_runs to
    /// loom_workflows must remain intact after the rebuild.
    #[test]
    fn loom_rows_preserved_through_v34() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();

        // Apply migrations v1..v33 (stop before v34).
        for m in TENANT_MIGRATIONS.iter() {
            if m.version >= 34 {
                break;
            }
            (m.up)(&conn).unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
                rusqlite::params![m.version],
            )
            .unwrap();
        }

        // Insert a workflow in the old shape (with user_id).
        conn.execute(
            "INSERT INTO loom_workflows (name, description, steps, user_id) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["pre-wf", "pre-desc", "[]", 1_i64],
        )
        .expect("insert old workflow");
        let wf_id: i64 = conn
            .query_row("SELECT last_insert_rowid()", [], |r| r.get(0))
            .unwrap();

        // Insert a run in the old shape (with user_id).
        conn.execute(
            "INSERT INTO loom_runs (workflow_id, status, input, output, user_id) VALUES (?1, 'pending', '{}', '{}', ?2)",
            rusqlite::params![wf_id, 1_i64],
        )
        .expect("insert old run");
        let run_id: i64 = conn
            .query_row("SELECT last_insert_rowid()", [], |r| r.get(0))
            .unwrap();

        // Apply v34.
        apply_schema_v34_loom_drop(&conn).expect("apply v34");

        // Workflow row survived with name and description intact.
        let (wf_name, wf_desc): (String, Option<String>) = conn
            .query_row(
                "SELECT name, description FROM loom_workflows WHERE id = ?1",
                rusqlite::params![wf_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("select workflow after v34");
        assert_eq!(wf_name, "pre-wf");
        assert_eq!(wf_desc.as_deref(), Some("pre-desc"));

        // Run row survived with workflow_id FK intact.
        let (run_wf_id, run_status): (i64, String) = conn
            .query_row(
                "SELECT workflow_id, status FROM loom_runs WHERE id = ?1",
                rusqlite::params![run_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("select run after v34");
        assert_eq!(run_wf_id, wf_id, "loom_runs.workflow_id FK preserved");
        assert_eq!(run_status, "pending");

        // user_id column is gone from both tables.
        let wf_col: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('loom_workflows') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(wf_col, 0, "loom_workflows still has user_id after v34");

        let runs_col: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('loom_runs') WHERE name='user_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(runs_col, 0, "loom_runs still has user_id after v34");
    }
}
