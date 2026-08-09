//! SQLite forge database -- opens the on-disk DB, applies the initial schema,
//! and runs incremental migrations so older databases gain new columns without
//! data loss. All tools share one `Database` instance per process.

use rusqlite::{Connection, Result as SqliteResult};
use std::fs;
use std::path::Path;

/// Thin wrapper around a `rusqlite::Connection` that owns the forge DB file.
/// Callers borrow the inner connection via `conn()` to execute queries.
pub struct Database {
    conn: Connection,
}

/// Open, initialise, and migrate the forge database.
impl Database {
    /// Open (or create) the forge DB at `path`, create parent directories as
    /// needed, apply the full schema, and run any pending migrations.
    pub fn open(path: &Path) -> SqliteResult<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    /// Create all core tables (specs, hypotheses, checkpoints, session_learns,
    /// approaches, verifications) if they do not already exist, then run migrations.
    fn init_schema(&self) -> SqliteResult<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS specs (
                id TEXT PRIMARY KEY,
                created_at INTEGER NOT NULL,
                task_description TEXT NOT NULL,
                task_type TEXT NOT NULL,
                acceptance_criteria TEXT NOT NULL,
                interface_contract TEXT,
                edge_cases TEXT,
                files_to_touch TEXT,
                dependencies TEXT,
                unchanged_behaviors TEXT NOT NULL DEFAULT '[]',
                implementation_tasks TEXT NOT NULL DEFAULT '[]',
                test_properties TEXT NOT NULL DEFAULT '[]',
                status TEXT DEFAULT 'active',
                completed_at INTEGER,
                status_note TEXT
            );

            CREATE TABLE IF NOT EXISTS hypotheses (
                id TEXT PRIMARY KEY,
                created_at INTEGER NOT NULL,
                bug_description TEXT NOT NULL,
                hypothesis TEXT NOT NULL,
                confidence REAL NOT NULL,
                outcome TEXT,
                outcome_notes TEXT,
                verified_at INTEGER,
                spec_id TEXT REFERENCES specs(id)
            );

            CREATE TABLE IF NOT EXISTS checkpoints (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                git_ref TEXT,
                files_snapshot TEXT,
                description TEXT,
                spec_id TEXT REFERENCES specs(id),
                slice_index INTEGER,
                repo_root TEXT,
                UNIQUE(repo_root, name)
            );

            CREATE TABLE IF NOT EXISTS session_learns (
                id TEXT PRIMARY KEY,
                created_at INTEGER NOT NULL,
                discovery TEXT NOT NULL,
                context TEXT,
                tags TEXT,
                spec_id TEXT REFERENCES specs(id)
            );

            CREATE TABLE IF NOT EXISTS approaches (
                id TEXT PRIMARY KEY,
                spec_id TEXT REFERENCES specs(id),
                created_at INTEGER NOT NULL,
                name TEXT NOT NULL,
                description TEXT NOT NULL,
                pros TEXT,
                cons TEXT,
                score REAL,
                chosen INTEGER DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS verifications (
                id TEXT PRIMARY KEY,
                spec_id TEXT REFERENCES specs(id),
                created_at INTEGER NOT NULL,
                command TEXT NOT NULL,
                exit_code INTEGER NOT NULL,
                success INTEGER NOT NULL,
                duration_ms INTEGER,
                criteria_index INTEGER,
                stdout TEXT,
                stderr TEXT
            );
            "#,
        )?;

        // Migrations for existing databases
        self.migrate()
    }

    /// Apply incremental column additions to existing databases. Each migration
    /// is guarded by a probe query so it is safe to re-run on an up-to-date DB.
    fn migrate(&self) -> SqliteResult<()> {
        let has_column = |table: &str, col: &str| -> bool {
            self.conn
                .prepare(&format!("SELECT {} FROM {} LIMIT 0", col, table))
                .is_ok()
        };

        if !has_column("hypotheses", "spec_id") {
            self.conn.execute_batch(
                "ALTER TABLE hypotheses ADD COLUMN spec_id TEXT REFERENCES specs(id);",
            )?;
        }
        if !has_column("session_learns", "spec_id") {
            self.conn.execute_batch(
                "ALTER TABLE session_learns ADD COLUMN spec_id TEXT REFERENCES specs(id);",
            )?;
        }
        if !has_column("specs", "completed_at") {
            self.conn
                .execute_batch("ALTER TABLE specs ADD COLUMN completed_at INTEGER;")?;
        }
        if !has_column("specs", "status_note") {
            self.conn
                .execute_batch("ALTER TABLE specs ADD COLUMN status_note TEXT;")?;
        }
        if !has_column("specs", "unchanged_behaviors") {
            self.conn.execute_batch(
                "ALTER TABLE specs ADD COLUMN unchanged_behaviors TEXT NOT NULL DEFAULT '[]';",
            )?;
        }
        if !has_column("specs", "implementation_tasks") {
            self.conn.execute_batch(
                "ALTER TABLE specs ADD COLUMN implementation_tasks TEXT NOT NULL DEFAULT '[]';",
            )?;
        }
        if !has_column("specs", "test_properties") {
            self.conn.execute_batch(
                "ALTER TABLE specs ADD COLUMN test_properties TEXT NOT NULL DEFAULT '[]';",
            )?;
        }
        if !has_column("checkpoints", "spec_id") {
            self.conn.execute_batch(
                "ALTER TABLE checkpoints ADD COLUMN spec_id TEXT REFERENCES specs(id);",
            )?;
        }
        if !has_column("checkpoints", "slice_index") {
            self.conn
                .execute_batch("ALTER TABLE checkpoints ADD COLUMN slice_index INTEGER;")?;
        }
        if !has_column("checkpoints", "repo_root") {
            // SQLite cannot drop the legacy UNIQUE(name) constraint in place.
            // Rebuild atomically, retaining old rows without claiming they
            // belong to a repository that was never recorded.
            self.conn.execute_batch(
                r#"
                BEGIN IMMEDIATE;
                ALTER TABLE checkpoints RENAME TO checkpoints_legacy;
                CREATE TABLE checkpoints (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    git_ref TEXT,
                    files_snapshot TEXT,
                    description TEXT,
                    spec_id TEXT REFERENCES specs(id),
                    slice_index INTEGER,
                    repo_root TEXT,
                    UNIQUE(repo_root, name)
                );
                INSERT INTO checkpoints (
                    id, name, created_at, git_ref, files_snapshot, description,
                    spec_id, slice_index, repo_root
                )
                SELECT id, name, created_at, git_ref, files_snapshot, description,
                       spec_id, slice_index, NULL
                FROM checkpoints_legacy;
                DROP TABLE checkpoints_legacy;
                COMMIT;
                "#,
            )?;
        }
        Ok(())
    }

    /// Return a shared reference to the underlying `rusqlite::Connection` for
    /// direct query execution by callers.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }
}

#[cfg(test)]
/// Schema and migration tests for the forge database.
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// A fresh database scopes duplicate checkpoint names by repository root.
    #[test]
    fn fresh_db_has_repository_scoped_checkpoints() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("forge.db")).unwrap();
        assert!(db
            .conn()
            .prepare("SELECT spec_id, slice_index, repo_root FROM checkpoints LIMIT 0")
            .is_ok());
        db.conn()
            .execute_batch(
                "INSERT INTO checkpoints (id, name, created_at, repo_root) \
                 VALUES ('one', 'shared', 1, '/repo/one'); \
                 INSERT INTO checkpoints (id, name, created_at, repo_root) \
                 VALUES ('two', 'shared', 2, '/repo/two');",
            )
            .unwrap();
    }

    /// A legacy database is rebuilt losslessly while its rows remain unscoped.
    #[test]
    fn legacy_db_gains_repository_scope_without_claiming_rows() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("forge.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE checkpoints (
                     id TEXT PRIMARY KEY,
                     name TEXT NOT NULL UNIQUE,
                     created_at INTEGER NOT NULL,
                     git_ref TEXT,
                     files_snapshot TEXT,
                     description TEXT
                 );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO checkpoints (id, name, created_at, git_ref, description) \
                 VALUES ('legacy', 'shared', 1, 'abc123', 'preserve me')",
                [],
            )
            .unwrap();
        }
        let db = Database::open(&path).unwrap();
        assert!(db
            .conn()
            .prepare("SELECT spec_id, slice_index, repo_root FROM checkpoints LIMIT 0")
            .is_ok());
        let legacy: (String, Option<String>) = db
            .conn()
            .query_row(
                "SELECT description, repo_root FROM checkpoints WHERE id = 'legacy'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(legacy, ("preserve me".into(), None));
        db.conn()
            .execute_batch(
                "INSERT INTO checkpoints (id, name, created_at, repo_root) \
                 VALUES ('one', 'shared', 2, '/repo/one'); \
                 INSERT INTO checkpoints (id, name, created_at, repo_root) \
                 VALUES ('two', 'shared', 3, '/repo/two');",
            )
            .unwrap();
        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM checkpoints WHERE name = 'shared'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 3);
    }

    /// A database created before spec artifacts gains empty structured fields
    /// without changing the existing spec row.
    #[test]
    fn legacy_specs_gain_artifact_fields_without_data_loss() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("forge.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE specs (
                     id TEXT PRIMARY KEY,
                     created_at INTEGER NOT NULL,
                     task_description TEXT NOT NULL,
                     task_type TEXT NOT NULL,
                     acceptance_criteria TEXT NOT NULL,
                     interface_contract TEXT,
                     edge_cases TEXT,
                     files_to_touch TEXT,
                     dependencies TEXT,
                     status TEXT DEFAULT 'active'
                 );
                 INSERT INTO specs (
                     id, created_at, task_description, task_type,
                     acceptance_criteria, status
                 ) VALUES (
                     'legacy', 1, 'Preserve this spec', 'feature',
                     '[\"still here\"]', 'active'
                 );",
            )
            .unwrap();
        }

        let db = Database::open(&path).unwrap();
        let row: (String, String, String, String) = db
            .conn()
            .query_row(
                "SELECT task_description, unchanged_behaviors,
                        implementation_tasks, test_properties
                 FROM specs WHERE id = 'legacy'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();

        assert_eq!(
            row,
            (
                "Preserve this spec".into(),
                "[]".into(),
                "[]".into(),
                "[]".into()
            )
        );
    }
}
