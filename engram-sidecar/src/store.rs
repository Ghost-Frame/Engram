//! Persistent SQLite-backed checkpoint for sidecar sessions.
//!
//! The sidecar keeps active sessions in an in-memory `SessionManager` for
//! speed, but losing them to a crash or restart breaks long-running agent
//! workflows. This module offers an opt-in SQLite store that:
//!   - snapshots every session on a 60s interval,
//!   - writes on demand (session end / explicit resume),
//!   - and exposes `load_all` / `load_one` for startup and resume routes.
//!
//! The store deliberately uses a plain `rusqlite::Connection` inside a
//! `tokio::task::spawn_blocking` wrapper rather than pulling in the engram-lib
//! connection pool. The sidecar only needs a single-writer, low-traffic
//! checkpoint file -- pooling would be overkill.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::{params, Connection};
use tokio::sync::Mutex;

use crate::session::SessionSnapshot;

/// Thread-safe handle to a sidecar session store.
#[derive(Clone)]
pub struct SessionStore {
    conn: Arc<Mutex<Connection>>,
    path: PathBuf,
}

impl SessionStore {
    /// Open (or create) the session store at `path`. Creates any required
    /// tables on first run.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("failed to create store dir: {e}"))?;
            }
        }
        let open_path = path.clone();
        let conn = tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&open_path).map_err(|e| e.to_string())?;
            // WAL gives us a small perf boost for the regular checkpoint
            // flushes without blocking concurrent reads from /resume.
            conn.pragma_update(None, "journal_mode", "WAL")
                .map_err(|e| e.to_string())?;
            conn.pragma_update(None, "synchronous", "NORMAL")
                .map_err(|e| e.to_string())?;
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS sidecar_sessions (
                     id TEXT PRIMARY KEY,
                     started_at TEXT NOT NULL,
                     observation_count INTEGER NOT NULL,
                     stored_count INTEGER NOT NULL,
                     ended INTEGER NOT NULL,
                     pending_json TEXT NOT NULL DEFAULT '[]',
                     updated_at TEXT NOT NULL DEFAULT (datetime('now'))
                 );
                 CREATE INDEX IF NOT EXISTS idx_sidecar_sessions_ended
                     ON sidecar_sessions(ended);",
            )
            .map_err(|e| e.to_string())?;
            Ok::<_, String>(conn)
        })
        .await
        .map_err(|e| format!("store init task panicked: {e}"))??;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Persist a batch of snapshots. Uses `INSERT ... ON CONFLICT DO UPDATE`
    /// so a snapshot for an existing id refreshes its row in place.
    pub async fn save_batch(&self, snapshots: Vec<SessionSnapshot>) -> Result<(), String> {
        if snapshots.is_empty() {
            return Ok(());
        }
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let mut guard = conn.blocking_lock();
            let tx = guard.transaction().map_err(|e| e.to_string())?;
            for snap in snapshots {
                let pending_json =
                    serde_json::to_string(&snap.pending).map_err(|e| e.to_string())?;
                tx.execute(
                    "INSERT INTO sidecar_sessions
                        (id, started_at, observation_count, stored_count, ended, pending_json, updated_at)
                        VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))
                     ON CONFLICT(id) DO UPDATE SET
                        started_at = excluded.started_at,
                        observation_count = excluded.observation_count,
                        stored_count = excluded.stored_count,
                        ended = excluded.ended,
                        pending_json = excluded.pending_json,
                        updated_at = datetime('now')",
                    params![
                        snap.id,
                        snap.started_at.to_rfc3339(),
                        snap.observation_count as i64,
                        snap.stored_count as i64,
                        if snap.ended { 1i64 } else { 0i64 },
                        pending_json,
                    ],
                )
                .map_err(|e| e.to_string())?;
            }
            tx.commit().map_err(|e| e.to_string())?;
            Ok(())
        })
        .await
        .map_err(|e| format!("save_batch join: {e}"))?
    }

    /// Load every persisted session. Used on startup to rebuild the manager.
    pub async fn load_all(&self) -> Result<Vec<SessionSnapshot>, String> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || -> Result<Vec<SessionSnapshot>, String> {
            let guard = conn.blocking_lock();
            Self::load_all_inner(&guard)
        })
        .await
        .map_err(|e| format!("load_all join: {e}"))?
    }

    fn load_all_inner(conn: &Connection) -> Result<Vec<SessionSnapshot>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT id, started_at, observation_count, stored_count, ended, pending_json
                     FROM sidecar_sessions",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            let (id, started_at, obs_count, stored_count, ended, pending_json) =
                row.map_err(|e| e.to_string())?;
            let started_at = chrono::DateTime::parse_from_rfc3339(&started_at)
                .map_err(|e| e.to_string())?
                .with_timezone(&chrono::Utc);
            let pending = serde_json::from_str(&pending_json).map_err(|e| e.to_string())?;
            out.push(SessionSnapshot {
                id,
                started_at,
                observation_count: obs_count as usize,
                stored_count: stored_count as usize,
                pending,
                ended: ended != 0,
            });
        }
        Ok(out)
    }

    /// Load one session by id, or None if it was never persisted.
    pub async fn load_one(&self, id: &str) -> Result<Option<SessionSnapshot>, String> {
        let id = id.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || -> Result<Option<SessionSnapshot>, String> {
            let guard = conn.blocking_lock();
            let row: Option<(String, String, i64, i64, i64, String)> = guard
                .query_row(
                    "SELECT id, started_at, observation_count, stored_count, ended, pending_json
                         FROM sidecar_sessions WHERE id = ?1",
                    params![id],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                        ))
                    },
                )
                .ok();
            match row {
                Some((id, started_at, obs_count, stored_count, ended, pending_json)) => {
                    let started_at = chrono::DateTime::parse_from_rfc3339(&started_at)
                        .map_err(|e| e.to_string())?
                        .with_timezone(&chrono::Utc);
                    let pending = serde_json::from_str(&pending_json).map_err(|e| e.to_string())?;
                    Ok(Some(SessionSnapshot {
                        id,
                        started_at,
                        observation_count: obs_count as usize,
                        stored_count: stored_count as usize,
                        pending,
                        ended: ended != 0,
                    }))
                }
                None => Ok(None),
            }
        })
        .await
        .map_err(|e| format!("load_one join: {e}"))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!("{prefix}-{}.db", uuid::Uuid::new_v4()))
    }

    #[tokio::test]
    async fn round_trip_snapshot() {
        let path = tmp_path("sidecar-store");
        let store = SessionStore::open(&path).await.expect("open");

        let snap = SessionSnapshot {
            id: "abc-123".into(),
            started_at: chrono::Utc::now(),
            observation_count: 4,
            stored_count: 2,
            pending: vec![],
            ended: false,
        };
        store.save_batch(vec![snap.clone()]).await.expect("save");

        let loaded = store.load_all().await.expect("load_all");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "abc-123");
        assert_eq!(loaded[0].observation_count, 4);
        assert_eq!(loaded[0].stored_count, 2);
        assert!(!loaded[0].ended);

        let one = store.load_one("abc-123").await.expect("load_one");
        assert!(one.is_some());

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn save_batch_updates_existing_row() {
        let path = tmp_path("sidecar-store");
        let store = SessionStore::open(&path).await.expect("open");

        let mut snap = SessionSnapshot {
            id: "s1".into(),
            started_at: chrono::Utc::now(),
            observation_count: 1,
            stored_count: 0,
            pending: vec![],
            ended: false,
        };
        store.save_batch(vec![snap.clone()]).await.expect("save 1");
        snap.observation_count = 42;
        snap.ended = true;
        store.save_batch(vec![snap]).await.expect("save 2");

        let loaded = store.load_all().await.expect("load");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].observation_count, 42);
        assert!(loaded[0].ended);

        let _ = std::fs::remove_file(&path);
    }
}
