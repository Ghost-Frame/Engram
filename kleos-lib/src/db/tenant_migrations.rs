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
