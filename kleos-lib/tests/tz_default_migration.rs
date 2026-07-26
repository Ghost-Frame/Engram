//! Migration tests for the datetime('now', 'utc') double-UTC-conversion fix
//! (global migration 99 / tenant migration 82).
//!
//! SQLite's 'utc' modifier treats its input as localtime and converts it to
//! UTC. Applying it to the already-UTC 'now' therefore double-converts,
//! skewing the stored value by the host's UTC offset (-2h under CEST, -1h
//! under CET). The fix rebuilds the affected tables with a plain
//! `datetime('now')` default and backfills skewed rows through
//! `datetime(col, 'localtime')` -- the exact per-row, DST-aware inverse of the
//! buggy write under an unchanged host timezone.
//!
//! Because the historical creation migrations now carry the corrected
//! default, a database freshly migrated to the pre-fix head no longer exhibits
//! the bug. Each test therefore "de-fixes" the tables under test back to the
//! exact buggy shape an old deployment has on disk, seeds skewed rows, and
//! then migrates forward -- the same shape production databases hit.
//!
//! The whole file pins TZ=Europe/Berlin so the expected offsets are
//! deterministic regardless of host timezone (CI runs on UTC, where the
//! buggy write and its inverse would both be identity and prove nothing).
//! Unix-only: the TZ/tzset mechanism does not control SQLite's localtime
//! conversion on Windows.
#![cfg(unix)]

use std::process::Command;

use rusqlite::Connection;

/// Re-runs one test in a child whose timezone is fixed before process startup.
///
/// SQLite and libc may cache timezone state before a test mutates `TZ`, which
/// made in-process environment mutation nondeterministic on non-UTC hosts.
/// The marker prevents recursion: the parent waits for the exact child test,
/// while the child returns `false` and executes the test body.
fn rerun_under_berlin(test_name: &str) -> bool {
    const CHILD_MARKER: &str = "KLEOS_TZ_TEST_CHILD";
    if std::env::var(CHILD_MARKER).as_deref() == Ok(test_name) {
        return false;
    }

    let status = Command::new(std::env::current_exe().expect("resolve integration-test binary"))
        .env("TZ", "Europe/Berlin")
        .env(CHILD_MARKER, test_name)
        .args(["--exact", test_name, "--nocapture"])
        .status()
        .expect("run timezone-pinned child test");
    assert!(
        status.success(),
        "timezone-pinned child test {test_name} failed with {status}"
    );
    true
}

/// Restores the buggy pre-fix global `handoffs` table (and its FTS triggers)
/// exactly as migration 55 originally created it, so migration 99 sees the
/// `datetime('now', 'utc')` default that real old deployments have on disk.
/// Dropping the table also drops its indexes and triggers; migration 99
/// recreates the indexes, and the triggers are recreated here because the
/// seeded rows must flow into `handoffs_fts` the same way production rows did.
fn defix_global_handoffs(conn: &Connection) {
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         DROP TABLE IF EXISTS handoffs;
         CREATE TABLE handoffs (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             user_id INTEGER NOT NULL DEFAULT 1,
             created_at TEXT NOT NULL DEFAULT (datetime('now', 'utc')),
             project TEXT NOT NULL,
             branch TEXT,
             directory TEXT,
             agent TEXT DEFAULT 'unknown',
             [type] TEXT DEFAULT 'manual',
             content TEXT NOT NULL,
             metadata TEXT,
             session_id TEXT,
             model TEXT,
             host TEXT,
             content_hash TEXT
         );
         CREATE TRIGGER handoffs_fts_ai AFTER INSERT ON handoffs BEGIN
             INSERT INTO handoffs_fts(rowid, content) VALUES (new.id, new.content);
         END;
         CREATE TRIGGER handoffs_fts_ad AFTER DELETE ON handoffs BEGIN
             INSERT INTO handoffs_fts(handoffs_fts, rowid, content) VALUES('delete', old.id, old.content);
         END;
         CREATE TRIGGER handoffs_fts_au AFTER UPDATE OF content ON handoffs BEGIN
             INSERT INTO handoffs_fts(handoffs_fts, rowid, content) VALUES('delete', old.id, old.content);
             INSERT INTO handoffs_fts(rowid, content) VALUES (new.id, new.content);
         END;",
    )
    .expect("de-fix global handoffs");
}

/// Restores the buggy pre-fix global `handoff_atoms` table as migration 63
/// originally created it (both timestamp defaults skewed).
fn defix_global_atoms(conn: &Connection) {
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         DROP TABLE IF EXISTS handoff_atoms;
         CREATE TABLE handoff_atoms (
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
         );",
    )
    .expect("de-fix global handoff_atoms");
}

/// Restores the buggy pre-fix global `enrollment_invites` table as migration
/// 56 originally created it.
fn defix_global_invites(conn: &Connection) {
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         DROP TABLE IF EXISTS enrollment_invites;
         CREATE TABLE enrollment_invites (
             id         INTEGER PRIMARY KEY AUTOINCREMENT,
             user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
             token_hash TEXT    NOT NULL UNIQUE,
             method     TEXT    NOT NULL DEFAULT 'fido2',
             created_at TEXT    NOT NULL DEFAULT (datetime('now', 'utc')),
             expires_at TEXT    NOT NULL,
             consumed_at TEXT
         );",
    )
    .expect("de-fix global enrollment_invites");
}

/// Restores the buggy pre-fix tenant `handoffs` table (schema_v43 shape,
/// identical to the global one) including the FTS triggers.
fn defix_tenant_handoffs(conn: &Connection) {
    // Same DDL on both sides of the split; the tenant FTS shadow also already
    // exists from the v43 migration, so the global helper is reusable as-is.
    defix_global_handoffs(conn);
}

/// Restores the buggy pre-fix tenant `handoff_atoms` table (schema v54 shape:
/// FK to handoffs with ON DELETE CASCADE, salience default 1.0, and a
/// user_id column without a default).
fn defix_tenant_atoms(conn: &Connection) {
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         DROP TABLE IF EXISTS handoff_atoms;
         CREATE TABLE handoff_atoms (
             id              INTEGER PRIMARY KEY AUTOINCREMENT,
             atom_id         TEXT NOT NULL,
             handoff_id      INTEGER NOT NULL REFERENCES handoffs(id) ON DELETE CASCADE,
             user_id         INTEGER NOT NULL,
             project         TEXT NOT NULL,
             atom_type       TEXT NOT NULL,
             content         TEXT NOT NULL,
             canonical_form  TEXT NOT NULL,
             salience        REAL NOT NULL DEFAULT 1.0,
             confidence      REAL NOT NULL DEFAULT 0.5,
             status          TEXT NOT NULL DEFAULT 'active',
             created_at      TEXT NOT NULL DEFAULT (datetime('now', 'utc')),
             last_seen_at    TEXT NOT NULL DEFAULT (datetime('now', 'utc')),
             seen_count      INTEGER NOT NULL DEFAULT 1,
             decay_immune    INTEGER NOT NULL DEFAULT 0,
             superseded_by   TEXT,
             metadata        TEXT
         );",
    )
    .expect("de-fix tenant handoff_atoms");
}

/// Reads a single TEXT column for a row selected by id.
fn text_at(conn: &Connection, sql: &str, id: i64) -> String {
    conn.query_row(sql, [id], |r| r.get(0))
        .expect("row present")
}

/// Absolute drift in seconds between a stored timestamp and datetime('now').
fn drift_seconds(conn: &Connection, table: &str, col: &str, id: i64) -> i64 {
    conn.query_row(
        &format!(
            "SELECT abs(strftime('%s','now') - strftime('%s', {col})) FROM {table} WHERE id = ?1"
        ),
        [id],
        |r| r.get(0),
    )
    .expect("drift query")
}

/// Counts live tables whose CREATE text still contains the buggy default.
fn buggy_default_count(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND sql LIKE '%datetime(''now'', ''utc'')%'",
        [],
        |r| r.get(0),
    )
    .expect("sqlite_master scan")
}

/// Old-DB path: rows written under the buggy default (summer, winter, and
/// live-default) come out of migration 99 as true UTC, with per-row DST-aware
/// offsets, the last_seen_at carve-out honored, and no buggy default left in
/// the live schema.
#[test]
fn global_v99_backfills_skewed_rows_dst_aware() {
    if rerun_under_berlin("global_v99_backfills_skewed_rows_dst_aware") {
        return;
    }
    let conn = Connection::open_in_memory().unwrap();
    kleos_lib::db::migrations::run_migrations_to(&conn, 98).unwrap();

    defix_global_handoffs(&conn);
    defix_global_atoms(&conn);
    defix_global_invites(&conn);

    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         INSERT INTO handoffs (id, project, content, created_at)
             VALUES (42, 'p', 'summer row', '2026-07-01 10:00:00');
         INSERT INTO handoffs (id, project, content, created_at)
             VALUES (43, 'p', 'winter row', '2026-01-15 10:00:00');
         -- SQLite 3.46+ recognizes 'now' as UTC and makes a following 'utc'
         -- modifier a no-op. Recreate the legacy engine's stored value
         -- explicitly: UTC minus the local offset at the current instant.
         INSERT INTO handoffs (id, project, content, created_at)
             VALUES (
                 44,
                 'p',
                 'live default row',
                 datetime(2 * julianday('now') - julianday('now', 'localtime'))
             );
         INSERT INTO handoff_atoms
             (id, atom_id, handoff_id, project, atom_type, content, canonical_form,
              created_at, last_seen_at, seen_count)
             VALUES (1, 'a-1', 42, 'p', 'fact', 'c1', 'c1',
                     '2026-07-01 10:00:00', '2026-07-01 10:00:00', 1);
         INSERT INTO handoff_atoms
             (id, atom_id, handoff_id, project, atom_type, content, canonical_form,
              created_at, last_seen_at, seen_count)
             VALUES (2, 'a-2', 42, 'p', 'fact', 'c2', 'c2',
                     '2026-07-01 10:00:00', '2026-07-10 09:30:00', 3);
         INSERT INTO enrollment_invites (id, user_id, token_hash, created_at, expires_at)
             VALUES (7, 1, 'th', '2026-07-01 10:00:00', '2026-07-02 10:00:00');",
    )
    .unwrap();

    kleos_lib::db::migrations::run_migrations_to(&conn, 99).unwrap();

    // Summer row shifts by the CEST offset, winter row by the CET offset:
    // the backfill resolves each row's offset through tzdata, not a constant.
    assert_eq!(
        text_at(&conn, "SELECT created_at FROM handoffs WHERE id = ?1", 42),
        "2026-07-01 12:00:00"
    );
    assert_eq!(
        text_at(&conn, "SELECT created_at FROM handoffs WHERE id = ?1", 43),
        "2026-01-15 11:00:00"
    );
    // A row written through the live buggy default lands back on true UTC.
    assert!(
        drift_seconds(&conn, "handoffs", "created_at", 44) < 10,
        "live-default row must backfill to within seconds of true UTC"
    );

    // Atom carve-out: created_at always shifts; last_seen_at shifts only for
    // rows still on their insert default (seen_count <= 1). The re-seen row's
    // last_seen_at was written correctly and must come through untouched.
    assert_eq!(
        text_at(
            &conn,
            "SELECT created_at FROM handoff_atoms WHERE id = ?1",
            1
        ),
        "2026-07-01 12:00:00"
    );
    assert_eq!(
        text_at(
            &conn,
            "SELECT last_seen_at FROM handoff_atoms WHERE id = ?1",
            1
        ),
        "2026-07-01 12:00:00"
    );
    assert_eq!(
        text_at(
            &conn,
            "SELECT created_at FROM handoff_atoms WHERE id = ?1",
            2
        ),
        "2026-07-01 12:00:00"
    );
    assert_eq!(
        text_at(
            &conn,
            "SELECT last_seen_at FROM handoff_atoms WHERE id = ?1",
            2
        ),
        "2026-07-10 09:30:00"
    );

    // Invites: both the creation stamp and the buggy-expression expiry shift.
    assert_eq!(
        text_at(
            &conn,
            "SELECT created_at FROM enrollment_invites WHERE id = ?1",
            7
        ),
        "2026-07-01 12:00:00"
    );
    assert_eq!(
        text_at(
            &conn,
            "SELECT expires_at FROM enrollment_invites WHERE id = ?1",
            7
        ),
        "2026-07-02 12:00:00"
    );

    // No live table carries the buggy default any more (the tables that were
    // not de-fixed entered v99 already-correct and were skipped by the guard).
    assert_eq!(buggy_default_count(&conn), 0);
}

/// The handoffs rebuild preserves row ids (the FTS external-content shadow is
/// keyed by rowid), keeps pre-migration content searchable, and leaves a
/// working AFTER INSERT trigger and a true-UTC default for new rows.
#[test]
fn global_v99_preserves_fts_ids_and_installs_correct_default() {
    if rerun_under_berlin("global_v99_preserves_fts_ids_and_installs_correct_default") {
        return;
    }
    let conn = Connection::open_in_memory().unwrap();
    kleos_lib::db::migrations::run_migrations_to(&conn, 98).unwrap();

    defix_global_handoffs(&conn);
    conn.execute(
        "INSERT INTO handoffs (id, project, content, created_at)
         VALUES (42, 'p', 'quantum flamingo memo', '2026-07-01 10:00:00')",
        [],
    )
    .unwrap();

    kleos_lib::db::migrations::run_migrations_to(&conn, 99).unwrap();

    // Pre-migration content is still reachable through FTS under its old id.
    let hit: i64 = conn
        .query_row(
            "SELECT rowid FROM handoffs_fts WHERE handoffs_fts MATCH 'flamingo'",
            [],
            |r| r.get(0),
        )
        .expect("pre-migration row must stay FTS-searchable");
    assert_eq!(hit, 42);

    // The recreated trigger indexes new rows, and the new default is true UTC.
    conn.execute(
        "INSERT INTO handoffs (id, project, content) VALUES (77, 'p', 'starlit walrus notes')",
        [],
    )
    .unwrap();
    let hit: i64 = conn
        .query_row(
            "SELECT rowid FROM handoffs_fts WHERE handoffs_fts MATCH 'walrus'",
            [],
            |r| r.get(0),
        )
        .expect("post-migration row must be FTS-searchable");
    assert_eq!(hit, 77);
    assert!(
        drift_seconds(&conn, "handoffs", "created_at", 77) < 10,
        "post-migration default must be true UTC"
    );
}

/// Fresh-DB path: a database created by the current binary has no buggy
/// default anywhere (the historical creation migrations were corrected), the
/// v99 rebuild is a harmless no-op on it, and new rows land on true UTC even
/// on a non-UTC host.
#[test]
fn fresh_db_has_correct_defaults_after_full_migration() {
    if rerun_under_berlin("fresh_db_has_correct_defaults_after_full_migration") {
        return;
    }
    let conn = Connection::open_in_memory().unwrap();
    kleos_lib::db::migrations::run_migrations(&conn).unwrap();

    assert_eq!(
        buggy_default_count(&conn),
        0,
        "no table in a fresh DB may default to datetime('now', 'utc')"
    );

    conn.execute(
        "INSERT INTO handoffs (id, project, content) VALUES (1, 'p', 'fresh row')",
        [],
    )
    .unwrap();
    assert!(
        drift_seconds(&conn, "handoffs", "created_at", 1) < 10,
        "fresh-DB default must be true UTC on a non-UTC host"
    );
}

/// Tenant mirror: v82 backfills skewed shard rows with the same DST-aware
/// inversion and carve-out, and the rebuilt handoffs/handoff_atoms pair keeps
/// its ON DELETE CASCADE relationship intact.
#[test]
fn tenant_v82_backfills_skewed_rows_and_preserves_cascade() {
    if rerun_under_berlin("tenant_v82_backfills_skewed_rows_and_preserves_cascade") {
        return;
    }
    let conn = Connection::open_in_memory().unwrap();
    kleos_lib::db::tenant_migrations::run_tenant_migrations_to(&conn, Some(1), 81).unwrap();

    defix_tenant_handoffs(&conn);
    defix_tenant_atoms(&conn);

    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         INSERT INTO handoffs (id, project, content, created_at)
             VALUES (42, 'p', 'summer row', '2026-07-01 10:00:00');
         INSERT INTO handoffs (id, project, content, created_at)
             VALUES (43, 'p', 'winter row', '2026-01-15 10:00:00');
         INSERT INTO handoff_atoms
             (id, atom_id, handoff_id, user_id, project, atom_type, content,
              canonical_form, created_at, last_seen_at, seen_count)
             VALUES (1, 'a-1', 42, 1, 'p', 'fact', 'c1', 'c1',
                     '2026-07-01 10:00:00', '2026-07-01 10:00:00', 1);
         INSERT INTO handoff_atoms
             (id, atom_id, handoff_id, user_id, project, atom_type, content,
              canonical_form, created_at, last_seen_at, seen_count)
             VALUES (2, 'a-2', 43, 1, 'p', 'fact', 'c2', 'c2',
                     '2026-07-01 10:00:00', '2026-07-10 09:30:00', 3);",
    )
    .unwrap();

    kleos_lib::db::tenant_migrations::run_tenant_migrations_to(&conn, Some(1), 82).unwrap();

    assert_eq!(
        text_at(&conn, "SELECT created_at FROM handoffs WHERE id = ?1", 42),
        "2026-07-01 12:00:00"
    );
    assert_eq!(
        text_at(&conn, "SELECT created_at FROM handoffs WHERE id = ?1", 43),
        "2026-01-15 11:00:00"
    );
    assert_eq!(
        text_at(
            &conn,
            "SELECT last_seen_at FROM handoff_atoms WHERE id = ?1",
            1
        ),
        "2026-07-01 12:00:00"
    );
    assert_eq!(
        text_at(
            &conn,
            "SELECT last_seen_at FROM handoff_atoms WHERE id = ?1",
            2
        ),
        "2026-07-10 09:30:00"
    );
    assert_eq!(buggy_default_count(&conn), 0);

    // The rebuild carried the FK through: deleting a handoff cascades into
    // its atoms exactly as the v54 schema promised.
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    conn.execute("DELETE FROM handoffs WHERE id = 42", [])
        .unwrap();
    let orphans: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM handoff_atoms WHERE handoff_id = 42",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(orphans, 0, "ON DELETE CASCADE must survive the rebuild");
}
