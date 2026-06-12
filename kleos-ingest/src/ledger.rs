use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;

/// File-ingest progress ledger backed by a single SQLite connection.
///
/// The connection is guarded by a `std::sync::Mutex`, and every method runs a
/// single short synchronous query while holding the lock. The lock is never
/// held across an `.await`, so it cannot stall the async scheduler the way a
/// std mutex held across a suspension point would. Each operation is a
/// sub-millisecond keyed lookup/upsert at file-tail frequency (one batch per
/// file-change event), so running synchronous SQLite directly on the runtime
/// thread is an acceptable trust boundary here rather than a `spawn_blocking`
/// hop. Revisit if the ledger ever moves onto a hot, high-concurrency path.
pub struct Ledger {
    conn: Mutex<Connection>,
}

impl Ledger {
    pub fn open(path: &Path) -> Result<Self, String> {
        if path != Path::new(":memory:") {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("failed to create ledger dir: {}", e))?;
            }
        }
        let conn = Connection::open(path).map_err(|e| format!("failed to open ledger: {}", e))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS files (
                path TEXT PRIMARY KEY,
                project TEXT,
                session_id TEXT,
                last_offset INTEGER NOT NULL DEFAULT 0,
                last_seen_at INTEGER NOT NULL,
                summarized INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS session_stats (
                session_id TEXT PRIMARY KEY,
                started_at INTEGER NOT NULL,
                last_activity_at INTEGER NOT NULL,
                memories_stored INTEGER NOT NULL DEFAULT 0,
                decisions_extracted INTEGER NOT NULL DEFAULT 0
            );",
        )
        .map_err(|e| format!("failed to init ledger schema: {}", e))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn get_offset(&self, path: &str) -> i64 {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT last_offset FROM files WHERE path = ?1",
            params![path],
            |row| row.get(0),
        )
        .unwrap_or(0)
    }

    pub fn set_offset(&self, path: &str, offset: i64, project: &str, session_id: &str) {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp();
        // Reset `summarized` to 0 whenever the offset advances past the previously
        // recorded position. This ensures that a session file receiving new content
        // after an idle-triggered summary will be re-summarized on the next idle
        // window -- without this reset the `summarized` flag stays set permanently
        // and all subsequent activity goes unsummarized (INGEST-1).
        let _ = conn.execute(
            "INSERT INTO files (path, project, session_id, last_offset, last_seen_at, summarized)
             VALUES (?1, ?2, ?3, ?4, ?5, 0)
             ON CONFLICT(path) DO UPDATE SET
                 last_offset = ?4,
                 last_seen_at = ?5,
                 summarized = CASE WHEN ?4 > last_offset THEN 0 ELSE summarized END",
            params![path, project, session_id, offset, now],
        );
    }

    pub fn is_summarized(&self, path: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT summarized FROM files WHERE path = ?1",
            params![path],
            |row| row.get::<_, i32>(0),
        )
        .unwrap_or(0)
            != 0
    }

    pub fn mark_summarized(&self, path: &str) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "UPDATE files SET summarized = 1 WHERE path = ?1",
            params![path],
        );
    }

    pub fn increment_memories(&self, session_id: &str) {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp();
        let _ = conn.execute(
            "INSERT INTO session_stats (session_id, started_at, last_activity_at, memories_stored)
             VALUES (?1, ?2, ?2, 1)
             ON CONFLICT(session_id) DO UPDATE SET
                memories_stored = memories_stored + 1,
                last_activity_at = ?2",
            params![session_id, now],
        );
    }

    pub fn print_stats(&self) {
        let conn = self.conn.lock().unwrap();
        let file_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
            .unwrap_or(0);
        let total_memories: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(memories_stored), 0) FROM session_stats",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let active_files: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE summarized = 0",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        println!("Ledger stats:");
        println!("  Files tracked: {}", file_count);
        println!("  Active (unsummarized): {}", active_files);
        println!("  Total memories stored: {}", total_memories);
    }
}
