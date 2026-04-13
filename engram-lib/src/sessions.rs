pub mod scrub;

use crate::db::Database;
use crate::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use tokio::sync::broadcast;
use uuid::Uuid;

const MAX_OUTPUT_LINES: usize = 10_000;

// ---------------------------------------------------------------------------
// SessionStatus -- lifecycle status for agent sessions.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Killed,
    TimedOut,
}

impl std::fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionStatus::Pending => write!(f, "pending"),
            SessionStatus::Running => write!(f, "running"),
            SessionStatus::Completed => write!(f, "completed"),
            SessionStatus::Failed => write!(f, "failed"),
            SessionStatus::Killed => write!(f, "killed"),
            SessionStatus::TimedOut => write!(f, "timed_out"),
        }
    }
}

impl std::str::FromStr for SessionStatus {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "pending" => Ok(SessionStatus::Pending),
            "running" => Ok(SessionStatus::Running),
            "completed" => Ok(SessionStatus::Completed),
            "failed" => Ok(SessionStatus::Failed),
            "killed" => Ok(SessionStatus::Killed),
            "timed_out" => Ok(SessionStatus::TimedOut),
            _ => Err(()),
        }
    }
}

impl SessionStatus {
    pub fn from_str_lossy(s: &str) -> Self {
        s.parse().unwrap_or(SessionStatus::Failed)
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            SessionStatus::Completed
                | SessionStatus::Failed
                | SessionStatus::Killed
                | SessionStatus::TimedOut
        )
    }
}

// ---------------------------------------------------------------------------
// ManagedSession -- in-memory session with output buffering + counters.
// ---------------------------------------------------------------------------

pub struct ManagedSession {
    pub id: String,
    pub task: String,
    pub agent: String,
    pub model: String,
    pub user: String,
    pub status: SessionStatus,
    pub created_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub output_buffer: VecDeque<String>,
    pub output_tx: broadcast::Sender<String>,
    pub exit_code: Option<i32>,
    pub pid: Option<u32>,
    /// Number of times the agent stored to engram during this session.
    pub engram_stores: usize,
    /// Number of user corrections issued during this session.
    pub corrections: usize,
    pub error: Option<String>,
}

impl ManagedSession {
    pub fn new(task: String, agent: String, model: String, user: String) -> Self {
        let (tx, _) = broadcast::channel(1024);
        ManagedSession {
            id: Uuid::new_v4().to_string(),
            task,
            agent,
            model,
            user,
            status: SessionStatus::Pending,
            created_at: Utc::now(),
            ended_at: None,
            output_buffer: VecDeque::new(),
            output_tx: tx,
            exit_code: None,
            pid: None,
            engram_stores: 0,
            corrections: 0,
            error: None,
        }
    }

    pub fn append_output(&mut self, line: String) {
        self.output_buffer.push_back(line.clone());
        if self.output_buffer.len() > MAX_OUTPUT_LINES {
            self.output_buffer.pop_front();
        }
        let _ = self.output_tx.send(line);
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "task": self.task,
            "agent": self.agent,
            "model": self.model,
            "user": self.user,
            "status": self.status,
            "created_at": self.created_at.to_rfc3339(),
            "ended_at": self.ended_at.map(|t| t.to_rfc3339()),
            "exit_code": self.exit_code,
            "pid": self.pid,
            "corrections": self.corrections,
            "engram_stores": self.engram_stores,
            "error": self.error,
            "output_lines": self.output_buffer.len(),
        })
    }

    pub fn short_id(&self) -> &str {
        if self.id.len() >= 8 { &self.id[..8] } else { &self.id }
    }
}

// ---------------------------------------------------------------------------
// Platform-specific process kill helper.
// ---------------------------------------------------------------------------

fn kill_process(pid: u32) {
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .output();
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .output();
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
    }
}

// ---------------------------------------------------------------------------
// SessionManager -- in-memory registry with optional SQLite persistence.
// ---------------------------------------------------------------------------

pub struct SessionManager {
    sessions: HashMap<String, ManagedSession>,
}

impl SessionManager {
    pub fn new() -> Self {
        SessionManager {
            sessions: HashMap::new(),
        }
    }

    pub fn create_session(
        &mut self,
        task: String,
        agent: String,
        model: String,
        user: String,
    ) -> String {
        let session = ManagedSession::new(task, agent, model, user);
        let id = session.id.clone();
        self.sessions.insert(id.clone(), session);
        id
    }

    pub fn append_output(&mut self, session_id: &str, line: String) {
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.append_output(line);
        }
    }

    /// Get session by id. If user is Some, returns None when user does not match (404 not 403).
    pub fn get_session(&self, id: &str, user: Option<&str>) -> Option<&ManagedSession> {
        let s = self.sessions.get(id)?;
        if let Some(u) = user {
            if s.user != u {
                return None;
            }
        }
        Some(s)
    }

    /// Get mutable session by id. If user is Some, returns None when user does not match.
    pub fn get_session_mut(&mut self, id: &str, user: Option<&str>) -> Option<&mut ManagedSession> {
        let s = self.sessions.get_mut(id)?;
        if let Some(u) = user {
            if s.user != u {
                return None;
            }
        }
        Some(s)
    }

    /// Send SIGTERM/taskkill to the session process and mark it Killed.
    pub fn kill_session(&mut self, id: &str, user: Option<&str>) -> std::result::Result<(), String> {
        let session = self
            .sessions
            .get_mut(id)
            .ok_or_else(|| format!("session {} not found", id))?;

        if let Some(u) = user {
            if session.user != u {
                return Err(format!("session {} not found", id));
            }
        }

        if session.status != SessionStatus::Running && session.status != SessionStatus::Pending {
            return Err(format!(
                "session {} is not running (status: {})",
                id, session.status
            ));
        }

        if let Some(pid) = session.pid {
            kill_process(pid);
        }

        session.status = SessionStatus::Killed;
        session.ended_at = Some(Utc::now());
        Ok(())
    }

    pub fn list_active(&self, user: &str) -> Vec<serde_json::Value> {
        self.sessions
            .values()
            .filter(|s| s.user == user)
            .filter(|s| {
                s.status == SessionStatus::Running || s.status == SessionStatus::Pending
            })
            .map(|s| s.to_json())
            .collect()
    }

    pub fn count_active_global(&self) -> usize {
        self.sessions
            .values()
            .filter(|s| {
                s.status == SessionStatus::Running || s.status == SessionStatus::Pending
            })
            .count()
    }

    pub fn list_all(&self, user: &str) -> Vec<serde_json::Value> {
        let mut all: Vec<_> = self
            .sessions
            .values()
            .filter(|s| s.user == user)
            .map(|s| s.to_json())
            .collect();

        all.sort_by(|a, b| {
            let a_ts = a["created_at"].as_str().unwrap_or("");
            let b_ts = b["created_at"].as_str().unwrap_or("");
            b_ts.cmp(a_ts)
        });
        all
    }

    /// Evict sessions that have reached a terminal status and ended longer ago than max_age.
    pub fn evict_completed(&mut self, max_age: std::time::Duration) {
        let now = Utc::now();
        self.sessions.retain(|_, s| {
            if !s.status.is_terminal() {
                return true;
            }
            match s.ended_at {
                Some(ended) => {
                    let age = now.signed_duration_since(ended);
                    let max_age_chrono = chrono::Duration::from_std(max_age)
                        .unwrap_or_else(|_| chrono::Duration::seconds(3600));
                    age < max_age_chrono
                }
                None => true,
            }
        });
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub agent: String,
    pub user_id: i64,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCreateRequest {
    pub agent: String,
}

pub async fn create_session(
    db: &Database,
    req: &SessionCreateRequest,
    user_id: i64,
) -> Result<SessionInfo> {
    let id = Uuid::new_v4().to_string();
    let agent = req.agent.clone();
    let id_for_insert = id.clone();
    db.write(move |conn| {
        conn.execute(
            "INSERT INTO sessions (id, agent, user_id) VALUES (?1, ?2, ?3)",
            params![id_for_insert, agent, user_id],
        )
        .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        Ok(())
    })
    .await?;

    // Fetch back the row so timestamps come from the DB (not local clock)
    get_session(db, &id, user_id).await
}

pub async fn get_session(db: &Database, session_id: &str, user_id: i64) -> Result<SessionInfo> {
    let session_id = session_id.to_string();
    db.read(move |conn| {
        conn.query_row(
            "SELECT id, agent, user_id, status, created_at, updated_at FROM sessions WHERE id = ?1 AND user_id = ?2",
            params![session_id, user_id],
            |row| row_to_session(row),
        )
        .optional()
        .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?
        .ok_or_else(|| crate::EngError::NotFound("session not found".into()))
    })
    .await
}

/// DOS-L4: enforce per-request pagination -- default 50 rows, max 500.
pub async fn list_sessions(
    db: &Database,
    user_id: i64,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Vec<SessionInfo>> {
    let limit = limit.unwrap_or(50).min(500) as i64;
    let offset = offset.unwrap_or(0) as i64;
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, agent, user_id, status, created_at, updated_at FROM sessions \
                 WHERE user_id = ?1 ORDER BY created_at DESC LIMIT ?2 OFFSET ?3",
            )
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        let rows = stmt
            .query_map(params![user_id, limit, offset], |row| row_to_session(row))
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row.map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?);
        }
        Ok(sessions)
    })
    .await
}

pub async fn append_output(
    db: &Database,
    session_id: &str,
    line: &str,
    user_id: i64,
) -> Result<()> {
    let session_id_owned = session_id.to_string();
    let line_owned = line.to_string();

    // Verify session exists and belongs to user
    let sid_check = session_id_owned.clone();
    let exists = db
        .read(move |conn| {
            let result = conn
                .query_row(
                    "SELECT id FROM sessions WHERE id = ?1 AND user_id = ?2",
                    params![sid_check, user_id],
                    |_| Ok(()),
                )
                .optional()
                .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
            Ok(result.is_some())
        })
        .await?;

    if !exists {
        return Err(crate::EngError::NotFound(format!(
            "session {} not found",
            session_id_owned
        )));
    }

    let sid_insert = session_id_owned.clone();
    db.write(move |conn| {
        conn.execute(
            "INSERT INTO session_output (session_id, line) VALUES (?1, ?2)",
            params![sid_insert, line_owned],
        )
        .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        Ok(())
    })
    .await?;

    // Update session updated_at
    let sid_update = session_id_owned.clone();
    db.write(move |conn| {
        conn.execute(
            "UPDATE sessions SET updated_at = datetime('now') WHERE id = ?1",
            params![sid_update],
        )
        .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        Ok(())
    })
    .await?;

    Ok(())
}

pub async fn get_session_output(
    db: &Database,
    session_id: &str,
    user_id: i64,
) -> Result<Vec<String>> {
    let session_id_owned = session_id.to_string();

    // Verify ownership
    let sid_check = session_id_owned.clone();
    let exists = db
        .read(move |conn| {
            let result = conn
                .query_row(
                    "SELECT id FROM sessions WHERE id = ?1 AND user_id = ?2",
                    params![sid_check, user_id],
                    |_| Ok(()),
                )
                .optional()
                .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
            Ok(result.is_some())
        })
        .await?;

    if !exists {
        return Err(crate::EngError::NotFound(format!(
            "session {} not found",
            session_id_owned
        )));
    }

    let sid_query = session_id_owned.clone();
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT line FROM session_output WHERE session_id = ?1 ORDER BY id ASC LIMIT 10000",
            )
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        let rows = stmt
            .query_map(params![sid_query], |row| row.get::<_, String>(0))
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?;
        let mut lines = Vec::new();
        for row in rows {
            lines.push(row.map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))?);
        }
        Ok(lines)
    })
    .await
}

fn row_to_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionInfo> {
    Ok(SessionInfo {
        id: row.get(0)?,
        agent: row.get(1)?,
        user_id: row.get(2)?,
        status: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_and_get_session() {
        let db = crate::db::Database::connect_memory().await.expect("db");
        let req = SessionCreateRequest {
            agent: "claude-code".to_string(),
        };
        let session = create_session(&db, &req, 1).await.expect("create");
        assert!(!session.id.is_empty());
        assert_eq!(session.agent, "claude-code");

        let fetched = get_session(&db, &session.id, 1).await.expect("get");
        assert_eq!(fetched.id, session.id);
    }

    #[tokio::test]
    async fn test_list_sessions() {
        let db = crate::db::Database::connect_memory().await.expect("db");
        let req = SessionCreateRequest {
            agent: "test-agent".to_string(),
        };
        create_session(&db, &req, 1).await.expect("create");

        let sessions = list_sessions(&db, 1, None, None).await.expect("list");
        assert!(!sessions.is_empty());
    }

    #[tokio::test]
    async fn test_append_and_get_output() {
        let db = crate::db::Database::connect_memory().await.expect("db");
        let req = SessionCreateRequest {
            agent: "test-agent".to_string(),
        };
        let session = create_session(&db, &req, 1).await.expect("create");

        append_output(&db, &session.id, "line 1", 1)
            .await
            .expect("append");
        append_output(&db, &session.id, "line 2", 1)
            .await
            .expect("append");

        let output = get_session_output(&db, &session.id, 1)
            .await
            .expect("get output");
        assert_eq!(output.len(), 2);
        assert_eq!(output[0], "line 1");
    }

    #[tokio::test]
    async fn test_append_to_nonexistent_session() {
        let db = crate::db::Database::connect_memory().await.expect("db");
        let result = append_output(&db, "nonexistent-id", "line", 1).await;
        assert!(result.is_err());
    }
}
