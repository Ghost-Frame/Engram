use std::collections::HashMap;
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub tool_name: String,
    pub content: String,
    pub importance: i32,
    pub category: String,
    pub timestamp: DateTime<Utc>,
}

/// Wire-format snapshot of a session. Used by the persistent store to
/// reassemble a `Session` after a sidecar restart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub id: String,
    pub started_at: DateTime<Utc>,
    pub observation_count: usize,
    pub stored_count: usize,
    pub pending: Vec<Observation>,
    pub ended: bool,
}

pub struct Session {
    pub id: String,
    pub started_at: DateTime<Utc>,
    pub observation_count: usize,
    pub stored_count: usize,
    pub pending: Vec<Observation>,
    pub ended: bool,
    /// When the oldest pending observation was enqueued. Drives the time-based
    /// flush trigger in the sidecar's background batcher. Not persisted --
    /// restored sessions reset the clock so their backlog flushes promptly.
    pub pending_since: Option<Instant>,
}

impl Session {
    pub fn new(id: String) -> Self {
        Self {
            id,
            started_at: Utc::now(),
            observation_count: 0,
            stored_count: 0,
            pending: Vec::new(),
            ended: false,
            pending_since: None,
        }
    }

    pub fn add_observation(&mut self, obs: Observation) -> usize {
        self.observation_count += 1;
        if self.pending.is_empty() {
            self.pending_since = Some(Instant::now());
        }
        self.pending.push(obs);
        self.pending.len()
    }

    pub fn drain_pending(&mut self) -> Vec<Observation> {
        let drained: Vec<Observation> = self.pending.drain(..).collect();
        self.stored_count += drained.len();
        self.pending_since = None;
        drained
    }

    pub fn end(&mut self) {
        self.ended = true;
    }

    pub fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            id: self.id.clone(),
            started_at: self.started_at,
            observation_count: self.observation_count,
            stored_count: self.stored_count,
            pending: self.pending.clone(),
            ended: self.ended,
        }
    }

    pub fn from_snapshot(s: SessionSnapshot) -> Self {
        let pending_since = if s.pending.is_empty() {
            None
        } else {
            // Backlog loaded from disk -- flush on the next time-based tick.
            Some(Instant::now())
        };
        Self {
            id: s.id,
            started_at: s.started_at,
            observation_count: s.observation_count,
            stored_count: s.stored_count,
            pending: s.pending,
            ended: s.ended,
            pending_since,
        }
    }
}

/// Summary info for a session, returned by listing endpoints.
#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub started_at: DateTime<Utc>,
    pub observation_count: usize,
    pub stored_count: usize,
    pub pending_count: usize,
    pub ended: bool,
}

impl From<&Session> for SessionInfo {
    fn from(s: &Session) -> Self {
        Self {
            id: s.id.clone(),
            started_at: s.started_at,
            observation_count: s.observation_count,
            stored_count: s.stored_count,
            pending_count: s.pending.len(),
            ended: s.ended,
        }
    }
}

/// Manages multiple concurrent sessions keyed by session ID.
pub struct SessionManager {
    sessions: HashMap<String, Session>,
    pub default_session_id: String,
}

impl SessionManager {
    pub fn new(default_session_id: String) -> Self {
        let mut sessions = HashMap::new();
        sessions.insert(
            default_session_id.clone(),
            Session::new(default_session_id.clone()),
        );
        Self {
            sessions,
            default_session_id,
        }
    }

    /// Resolve a session_id: use provided value or fall back to default.
    pub fn resolve_id<'a>(&'a self, session_id: Option<&'a str>) -> &'a str {
        session_id.unwrap_or(&self.default_session_id)
    }

    /// Get an existing session by ID. Returns None if not found.
    pub fn get(&self, session_id: &str) -> Option<&Session> {
        self.sessions.get(session_id)
    }

    /// Get a mutable reference to an existing session.
    pub fn get_mut(&mut self, session_id: &str) -> Option<&mut Session> {
        self.sessions.get_mut(session_id)
    }

    /// Get an existing session or create a new one. Returns mutable ref.
    pub fn get_or_create(&mut self, session_id: &str) -> &mut Session {
        self.sessions
            .entry(session_id.to_string())
            .or_insert_with(|| Session::new(session_id.to_string()))
    }

    /// Explicitly start a new session. Returns error if session already exists
    /// and is not ended. If the session existed but was ended, it is replaced
    /// with a fresh one.
    pub fn start_session(&mut self, id: String) -> Result<&Session, SessionError> {
        if let Some(existing) = self.sessions.get(&id) {
            if !existing.ended {
                return Err(SessionError::AlreadyExists(id));
            }
        }
        self.sessions.insert(id.clone(), Session::new(id.clone()));
        Ok(self.sessions.get(&id).unwrap())
    }

    /// End a session by ID. Returns session stats or error.
    pub fn end_session(&mut self, session_id: &str) -> Result<SessionInfo, SessionError> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| SessionError::NotFound(session_id.to_string()))?;

        if session.ended {
            return Err(SessionError::AlreadyEnded(session_id.to_string()));
        }

        session.end();
        Ok(SessionInfo::from(&*session))
    }

    /// List all sessions (active and ended).
    pub fn list(&self) -> Vec<SessionInfo> {
        self.sessions.values().map(SessionInfo::from).collect()
    }

    /// List only active (non-ended) sessions.
    pub fn list_active(&self) -> Vec<SessionInfo> {
        self.sessions
            .values()
            .filter(|s| !s.ended)
            .map(SessionInfo::from)
            .collect()
    }

    /// Count of active (non-ended) sessions.
    pub fn active_count(&self) -> usize {
        self.sessions.values().filter(|s| !s.ended).count()
    }

    /// Total session count (including ended).
    pub fn total_count(&self) -> usize {
        self.sessions.len()
    }

    /// Snapshot every session for persistent-store flushing.
    pub fn snapshot_all(&self) -> Vec<SessionSnapshot> {
        self.sessions.values().map(Session::snapshot).collect()
    }

    /// Restore a snapshot into the manager. If a session with the same id
    /// already exists, the in-memory copy wins (caller should prefer the
    /// live state over a stale on-disk copy).
    pub fn restore_snapshot(&mut self, snap: SessionSnapshot) -> bool {
        if self.sessions.contains_key(&snap.id) {
            return false;
        }
        self.sessions
            .insert(snap.id.clone(), Session::from_snapshot(snap));
        true
    }
}

#[derive(Debug)]
pub enum SessionError {
    NotFound(String),
    AlreadyExists(String),
    AlreadyEnded(String),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "session not found: {}", id),
            Self::AlreadyExists(id) => write!(f, "session already exists and is active: {}", id),
            Self::AlreadyEnded(id) => write!(f, "session already ended: {}", id),
        }
    }
}

impl std::error::Error for SessionError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_new() {
        let s = Session::new("test-123".to_string());
        assert_eq!(s.id, "test-123");
        assert_eq!(s.observation_count, 0);
        assert_eq!(s.stored_count, 0);
        assert!(s.pending.is_empty());
        assert!(!s.ended);
    }

    #[test]
    fn test_add_and_drain() {
        let mut s = Session::new("test".to_string());
        let obs = Observation {
            tool_name: "read_file".to_string(),
            content: "Found config at /etc/app.conf".to_string(),
            importance: 3,
            category: "discovery".to_string(),
            timestamp: Utc::now(),
        };

        assert_eq!(s.add_observation(obs.clone()), 1);
        assert_eq!(s.add_observation(obs), 2);
        assert_eq!(s.observation_count, 2);

        let drained = s.drain_pending();
        assert_eq!(drained.len(), 2);
        assert!(s.pending.is_empty());
        assert_eq!(s.stored_count, 2);
    }

    #[test]
    fn test_end_session() {
        let mut s = Session::new("test".to_string());
        assert!(!s.ended);
        s.end();
        assert!(s.ended);
    }

    #[test]
    fn test_session_manager_default() {
        let mgr = SessionManager::new("default-123".to_string());
        assert_eq!(mgr.default_session_id, "default-123");
        assert_eq!(mgr.active_count(), 1);
        assert!(mgr.get("default-123").is_some());
    }

    #[test]
    fn test_session_manager_get_or_create() {
        let mut mgr = SessionManager::new("default".to_string());
        assert_eq!(mgr.active_count(), 1);

        // Auto-create a new session
        let s = mgr.get_or_create("session-2");
        assert_eq!(s.id, "session-2");
        assert_eq!(mgr.active_count(), 2);

        // Getting it again returns the same session, not a new one
        let s = mgr.get_or_create("session-2");
        assert_eq!(s.id, "session-2");
        assert_eq!(mgr.active_count(), 2);
    }

    #[test]
    fn test_session_manager_start_session() {
        let mut mgr = SessionManager::new("default".to_string());

        // Start a new session
        let result = mgr.start_session("new-session".to_string());
        assert!(result.is_ok());
        assert_eq!(mgr.active_count(), 2);

        // Starting the same session again should fail
        let result = mgr.start_session("new-session".to_string());
        assert!(result.is_err());

        // End the session, then restart it
        mgr.end_session("new-session").unwrap();
        let result = mgr.start_session("new-session".to_string());
        assert!(result.is_ok());
    }

    #[test]
    fn test_session_manager_end_session() {
        let mut mgr = SessionManager::new("default".to_string());
        mgr.start_session("s1".to_string()).unwrap();
        mgr.start_session("s2".to_string()).unwrap();
        assert_eq!(mgr.active_count(), 3);

        let info = mgr.end_session("s1").unwrap();
        assert_eq!(info.id, "s1");
        assert!(info.ended);
        assert_eq!(mgr.active_count(), 2);
        assert_eq!(mgr.total_count(), 3); // ended session still in map

        // Ending again should fail
        let result = mgr.end_session("s1");
        assert!(result.is_err());

        // Ending nonexistent should fail
        let result = mgr.end_session("nope");
        assert!(result.is_err());
    }

    #[test]
    fn test_session_manager_list() {
        let mut mgr = SessionManager::new("default".to_string());
        mgr.start_session("s1".to_string()).unwrap();
        mgr.start_session("s2".to_string()).unwrap();
        mgr.end_session("s1").unwrap();

        let all = mgr.list();
        assert_eq!(all.len(), 3);

        let active = mgr.list_active();
        assert_eq!(active.len(), 2);
        assert!(active.iter().all(|s| !s.ended));
    }

    #[test]
    fn test_session_manager_resolve_id() {
        let mgr = SessionManager::new("my-default".to_string());
        assert_eq!(mgr.resolve_id(None), "my-default");
        assert_eq!(mgr.resolve_id(Some("explicit")), "explicit");
    }

    #[test]
    fn snapshot_round_trip_preserves_state() {
        let mut mgr = SessionManager::new("root".to_string());
        let s = mgr.get_or_create("s1");
        s.add_observation(Observation {
            tool_name: "read".into(),
            content: "hello".into(),
            importance: 4,
            category: "discovery".into(),
            timestamp: Utc::now(),
        });
        assert_eq!(s.pending.len(), 1);

        let snaps = mgr.snapshot_all();
        assert!(snaps.iter().any(|s| s.id == "s1" && !s.pending.is_empty()));

        // Fresh manager: restoring brings the session back with its pending obs.
        let mut fresh = SessionManager::new("root-2".to_string());
        for snap in snaps {
            fresh.restore_snapshot(snap);
        }
        let restored = fresh.get("s1").expect("s1 restored");
        assert_eq!(restored.pending.len(), 1);
        assert_eq!(restored.observation_count, 1);
    }

    #[test]
    fn add_observation_sets_pending_since_on_first_only() {
        let mut s = Session::new("t".into());
        assert!(s.pending_since.is_none());
        let obs = Observation {
            tool_name: "t".into(),
            content: "c".into(),
            importance: 1,
            category: "d".into(),
            timestamp: Utc::now(),
        };
        s.add_observation(obs.clone());
        let first = s.pending_since.expect("set on first add");
        // Second add must NOT bump pending_since -- it tracks the OLDEST
        // pending observation, not the newest.
        std::thread::sleep(std::time::Duration::from_millis(5));
        s.add_observation(obs);
        let after = s.pending_since.expect("still set");
        assert_eq!(first, after, "pending_since tracks oldest, not newest");
    }

    #[test]
    fn drain_pending_clears_pending_since() {
        let mut s = Session::new("t".into());
        s.add_observation(Observation {
            tool_name: "t".into(),
            content: "c".into(),
            importance: 1,
            category: "d".into(),
            timestamp: Utc::now(),
        });
        assert!(s.pending_since.is_some());
        let drained = s.drain_pending();
        assert_eq!(drained.len(), 1);
        assert!(s.pending_since.is_none(), "drain clears the timer");
    }

    #[test]
    fn from_snapshot_restores_pending_since_when_backlog_exists() {
        let snap_with = SessionSnapshot {
            id: "s".into(),
            started_at: Utc::now(),
            observation_count: 2,
            stored_count: 0,
            pending: vec![Observation {
                tool_name: "t".into(),
                content: "c".into(),
                importance: 1,
                category: "d".into(),
                timestamp: Utc::now(),
            }],
            ended: false,
        };
        let s = Session::from_snapshot(snap_with);
        assert!(
            s.pending_since.is_some(),
            "restored backlog must start the flush clock"
        );

        let snap_empty = SessionSnapshot {
            id: "s".into(),
            started_at: Utc::now(),
            observation_count: 0,
            stored_count: 0,
            pending: vec![],
            ended: false,
        };
        let s = Session::from_snapshot(snap_empty);
        assert!(s.pending_since.is_none());
    }

    #[test]
    fn restore_snapshot_does_not_overwrite_existing() {
        let mut mgr = SessionManager::new("root".to_string());
        mgr.get_or_create("live");
        let snap = SessionSnapshot {
            id: "live".into(),
            started_at: Utc::now(),
            observation_count: 99,
            stored_count: 99,
            pending: vec![],
            ended: true,
        };
        let inserted = mgr.restore_snapshot(snap);
        assert!(!inserted, "should not replace existing live session");
        let live = mgr.get("live").unwrap();
        assert_eq!(live.observation_count, 0);
        assert!(!live.ended);
    }
}
