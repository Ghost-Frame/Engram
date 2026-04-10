use rusqlite::{Connection, Result as SqliteResult};
use std::path::Path;
use std::fs;

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &Path) -> SqliteResult<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

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
                status TEXT DEFAULT 'active'
            );

            CREATE TABLE IF NOT EXISTS hypotheses (
                id TEXT PRIMARY KEY,
                created_at INTEGER NOT NULL,
                bug_description TEXT NOT NULL,
                hypothesis TEXT NOT NULL,
                confidence REAL NOT NULL,
                outcome TEXT,
                outcome_notes TEXT,
                verified_at INTEGER
            );

            CREATE TABLE IF NOT EXISTS checkpoints (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                created_at INTEGER NOT NULL,
                git_ref TEXT,
                files_snapshot TEXT,
                description TEXT
            );

            CREATE TABLE IF NOT EXISTS session_learns (
                id TEXT PRIMARY KEY,
                created_at INTEGER NOT NULL,
                discovery TEXT NOT NULL,
                context TEXT,
                tags TEXT
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
            "#,
        )
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }
}
