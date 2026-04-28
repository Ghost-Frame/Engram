//! Round-trip smoke test: encrypt a source DB with SQLCipher, run the
//! kleos-migrate binary against it with the key env var set, assert the
//! target tenant shard ends up with the expected row counts.
//!
//! This is the only test that exercises the PRAGMA key branch of
//! source::open, so regressions in the SQLCipher path are caught here.

use rusqlite::Connection;
use std::process::Command;

const TEST_KEY_HEX: &str = "deadbeefcafe0000deadbeefcafe0000deadbeefcafe0000deadbeefcafe0000";

fn write_encrypted_source(path: &std::path::Path) {
    let conn = Connection::open(path).expect("open fresh source db");
    conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", TEST_KEY_HEX))
        .expect("apply pragma key");
    conn.execute_batch(
        "CREATE TABLE memories (
            id INTEGER PRIMARY KEY,
            user_id INTEGER NOT NULL,
            content TEXT NOT NULL,
            created_at INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL DEFAULT 0
        );
        INSERT INTO memories (id, user_id, content) VALUES
            (1, 1, 'master first'),
            (2, 1, 'master second'),
            (3, 2, 'bot row should be excluded'),
            (4, 1, 'master third');
        CREATE TABLE spaces (id INTEGER PRIMARY KEY, name TEXT);
        INSERT INTO spaces VALUES (1, 'master-space');",
    )
    .expect("seed rows");
}

#[test]
fn migrates_encrypted_monolith_into_tenant_shard() {
    let tmp = tempfile::tempdir().expect("mktemp");
    let source_path = tmp.path().join("source.db");
    let target_dir = tmp.path().join("tenant1");

    write_encrypted_source(&source_path);

    // Sanity: plaintext sqlite cannot read the source.
    assert!(
        Connection::open(&source_path)
            .and_then(|c| c.query_row("SELECT COUNT(*) FROM memories", [], |r| r.get::<_, i64>(0)))
            .is_err(),
        "source must be encrypted: unkeyed read should fail"
    );

    let bin = env!("CARGO_BIN_EXE_kleos-migrate");
    let output = Command::new(bin)
        .env("KLEOS_MIGRATE_TEST_KEY", TEST_KEY_HEX)
        .args([
            "--source",
            source_path.to_str().unwrap(),
            "--source-key-env",
            "KLEOS_MIGRATE_TEST_KEY",
            "--target",
            target_dir.to_str().unwrap(),
            "--filter-user-id",
            "1",
        ])
        .output()
        .expect("spawn kleos-migrate");

    assert!(
        output.status.success(),
        "kleos-migrate failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // Target tenant shard is a plaintext kleos.db after migration.
    let target_db = target_dir.join("kleos.db");
    assert!(target_db.exists(), "target kleos.db must exist");

    let target_conn = Connection::open(&target_db).expect("open target");
    let memories_count: i64 = target_conn
        .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))
        .expect("count memories");
    assert_eq!(memories_count, 3, "only user_id=1 rows should be copied");

    // spaces is now a valid tenant table (v44 parity). The source
    // spaces table lacks a user_id column so all rows are copied
    // unfiltered via column intersection (id + name).
    let spaces_count: i64 = target_conn
        .query_row("SELECT COUNT(*) FROM spaces", [], |r| r.get(0))
        .expect("count spaces");
    assert_eq!(
        spaces_count, 1,
        "source spaces row should be copied (no user_id filter)"
    );

    // Contents round-trip.
    let first: String = target_conn
        .query_row("SELECT content FROM memories WHERE id = 1", [], |r| {
            r.get(0)
        })
        .expect("read memory 1");
    assert_eq!(first, "master first");
}

fn write_compat3_source(path: &std::path::Path) {
    let conn = Connection::open(path).expect("open fresh source db");
    // compat PRAGMA MUST precede key pragma.
    conn.execute_batch("PRAGMA cipher_compatibility = 3;")
        .expect("set compat 3");
    conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", TEST_KEY_HEX))
        .expect("apply key");
    conn.execute_batch(
        "CREATE TABLE memories (id INTEGER PRIMARY KEY, user_id INTEGER, content TEXT);
         INSERT INTO memories VALUES (1, 1, 'compat3 row');",
    )
    .expect("seed");
}

/// Covers the fallback path in source::open when the source was created
/// with SQLCipher 3 instead of 4. Without the fallback, tool would error
/// "file is not a database".
#[test]
fn opens_cipher_compatibility_3_source() {
    let tmp = tempfile::tempdir().expect("mktemp");
    let source_path = tmp.path().join("source_compat3.db");
    let target_dir = tmp.path().join("tenant_compat3");

    write_compat3_source(&source_path);

    let bin = env!("CARGO_BIN_EXE_kleos-migrate");
    let output = Command::new(bin)
        .env("KLEOS_MIGRATE_TEST_KEY", TEST_KEY_HEX)
        .args([
            "--source",
            source_path.to_str().unwrap(),
            "--source-key-env",
            "KLEOS_MIGRATE_TEST_KEY",
            "--target",
            target_dir.to_str().unwrap(),
            "--filter-user-id",
            "1",
        ])
        .output()
        .expect("spawn");

    assert!(
        output.status.success(),
        "compat=3 fallback should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let conn = Connection::open(target_dir.join("kleos.db")).expect("open target");
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))
        .expect("count");
    assert_eq!(count, 1);
}

/// --dry-run reports counts without creating the target directory.
#[test]
fn dry_run_does_not_write_target() {
    let tmp = tempfile::tempdir().expect("mktemp");
    let source_path = tmp.path().join("source.db");
    let target_dir = tmp.path().join("ghost-target");

    write_encrypted_source(&source_path);

    let bin = env!("CARGO_BIN_EXE_kleos-migrate");
    let output = Command::new(bin)
        .env("KLEOS_MIGRATE_TEST_KEY", TEST_KEY_HEX)
        .args([
            "--source",
            source_path.to_str().unwrap(),
            "--source-key-env",
            "KLEOS_MIGRATE_TEST_KEY",
            "--target",
            target_dir.to_str().unwrap(),
            "--filter-user-id",
            "1",
            "--dry-run",
        ])
        .output()
        .expect("spawn");

    assert!(output.status.success(), "dry run should succeed");
    assert!(
        !target_dir.exists(),
        "dry run must not create the target directory"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Dry run: monolith source-filtered counts"),
        "stdout should contain dry-run header; got: {}",
        stdout
    );
    assert!(
        stdout.contains("memories"),
        "stdout should list memories table"
    );
}

/// Build a legacy plaintext `handoffs.db` matching the schema that
/// `kleos_lib::handoffs::HandoffsDb::open` produced before the v43
/// refactor (table + user_id column).
fn write_legacy_handoffs(path: &std::path::Path) {
    let conn = Connection::open(path).expect("open handoffs db");
    conn.execute_batch(
        "CREATE TABLE handoffs (
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
        INSERT INTO handoffs (user_id, project, content) VALUES
            (1, 'Kleos', 'master handoff one'),
            (1, 'Kleos', 'master handoff two'),
            (2, 'Bot',   'bot handoff one'),
            (1, 'Misc',  'master handoff three');",
    )
    .expect("seed legacy handoffs");
}

/// `--handoffs-source` alone (no `--source`) targets a fresh tenant shard
/// and copies only the rows for `--filter-user-id`. Validates the
/// reserved 'handoffs' tenant code path.
#[test]
fn handoffs_only_etl_into_handoffs_shard() {
    let tmp = tempfile::tempdir().expect("mktemp");
    let handoffs_src = tmp.path().join("legacy-handoffs.db");
    let target_dir = tmp.path().join("tenants").join("handoffs");

    write_legacy_handoffs(&handoffs_src);

    let bin = env!("CARGO_BIN_EXE_kleos-migrate");
    let output = Command::new(bin)
        .args([
            "--handoffs-source",
            handoffs_src.to_str().unwrap(),
            "--target",
            target_dir.to_str().unwrap(),
            "--filter-user-id",
            "1",
        ])
        .output()
        .expect("spawn");

    assert!(
        output.status.success(),
        "handoffs-only run should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let target_db = target_dir.join("kleos.db");
    assert!(target_db.exists(), "target shard kleos.db must exist");

    let target = Connection::open(&target_db).expect("open target");
    let count: i64 = target
        .query_row("SELECT COUNT(*) FROM handoffs", [], |r| r.get(0))
        .expect("count handoffs");
    assert_eq!(count, 3, "user_id=1 has 3 rows in legacy seed");

    // FTS5 must be wired by schema_v43 trigger.
    let fts_hits: i64 = target
        .query_row(
            "SELECT COUNT(*) FROM handoffs_fts WHERE handoffs_fts MATCH 'master'",
            [],
            |r| r.get(0),
        )
        .expect("fts count");
    assert!(
        fts_hits >= 3,
        "FTS5 must have indexed all copied rows, got {fts_hits}"
    );

    // A second pass for user_id=2 appends without clobbering the originals.
    let output = Command::new(bin)
        .args([
            "--handoffs-source",
            handoffs_src.to_str().unwrap(),
            "--target",
            target_dir.to_str().unwrap(),
            "--filter-user-id",
            "2",
            "--force",
        ])
        .output()
        .expect("spawn second pass");
    assert!(
        output.status.success(),
        "second pass should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let master_rows: i64 = target
        .query_row("SELECT COUNT(*) FROM handoffs WHERE user_id = 1", [], |r| {
            r.get(0)
        })
        .expect("count master");
    let bot_rows: i64 = target
        .query_row("SELECT COUNT(*) FROM handoffs WHERE user_id = 2", [], |r| {
            r.get(0)
        })
        .expect("count bot");
    assert_eq!(master_rows, 3);
    assert_eq!(bot_rows, 1);
}

/// Dry-run with --handoffs-source reports the filtered count without
/// touching the target directory.
#[test]
fn handoffs_dry_run_reports_count() {
    let tmp = tempfile::tempdir().expect("mktemp");
    let handoffs_src = tmp.path().join("legacy-handoffs.db");
    let target_dir = tmp.path().join("ghost-shard");

    write_legacy_handoffs(&handoffs_src);

    let bin = env!("CARGO_BIN_EXE_kleos-migrate");
    let output = Command::new(bin)
        .args([
            "--handoffs-source",
            handoffs_src.to_str().unwrap(),
            "--target",
            target_dir.to_str().unwrap(),
            "--filter-user-id",
            "1",
            "--dry-run",
        ])
        .output()
        .expect("spawn dry run");

    assert!(output.status.success(), "dry run should succeed");
    assert!(!target_dir.exists(), "dry run must not write target");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Handoffs (filtered)"),
        "dry-run output should include handoffs row, got: {stdout}"
    );
    assert!(
        stdout.contains("3"),
        "dry-run should report 3 rows for user_id=1, got: {stdout}"
    );
}
