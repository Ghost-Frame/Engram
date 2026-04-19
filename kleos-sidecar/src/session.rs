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

pub struct Session {
    pub id: String,
    pub started_at: DateTime<Utc>,
    pub observation_count: usize,
    pub stored_count: usize,
    pub pending: Vec<Observation>,
    pub ended: bool,
    /// When the oldest pending observation was enqueued. Drives the time-based
    /// flush trigger in the sidecar's background batcher.
    pub pending_since: Option<Instant>,
    /// Updated on every observation add or append. Used by the idle-expiry sweep.
    pub last_activity: Instant,
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
            last_activity: Instant::now(),
        }
    }

    pub fn add_observation(&mut self, obs: Observation) -> usize {
        self.observation_count += 1;
        self.last_activity = Instant::now();
        if self.pending.is_empty() {
            self.pending_since = Some(Instant::now());
        }
        self.pending.push(obs);
        self.pending.len()
    }

    /// Drain the pending queue. Does NOT bump stored_count -- callers must
    /// invoke `record_stored(n)` only after the upstream confirms how many
    /// observations were actually persisted. This split lets a partial-failure
    /// response restore the failed suffix via `requeue(...)` without the
    /// counter temporarily over-reporting.
    pub fn drain_pending(&mut self) -> Vec<Observation> {
        let drained: Vec<Observation> = self.pending.drain(..).collect();
        self.pending_since = None;
        drained
    }

    /// Bump stored_count by `n`. Call after the upstream confirms the count.
    pub fn record_stored(&mut self, n: usize) {
        self.stored_count = self.stored_count.saturating_add(n);
    }

    /// Prepend `observations` back to the front of the pending queue. Used to
    /// requeue a failed flush batch ahead of any observations that arrived
    /// while the flush was in flight. Re-arms `pending_since` if needed so the
    /// time-based flush trigger fires again for the restored batch.
    pub fn requeue(&mut self, mut observations: Vec<Observation>) {
        if observations.is_empty() {
            return;
        }
        observations.append(&mut self.pending);
        self.pending = observations;
        if self.pending_since.is_none() {
            self.pending_since = Some(Instant::now());
        }
        self.last_activity = Instant::now();
    }

    pub fn end(&mut self) {
        self.ended = true;
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

    /// Remove sessions that have been idle longer than `idle_ttl`. Active sessions
    /// with pending observations are skipped even if idle -- they will be swept
    /// on the next cycle after their queue drains. Returns the count removed.
    pub fn expire_idle(&mut self, idle_ttl: std::time::Duration) -> usize {
        let before = self.sessions.len();
        self.sessions.retain(|id, session| {
            // Never expire the default session or sessions with pending observations.
            if id == &self.default_session_id || !session.pending.is_empty() {
                return true;
            }
            session.last_activity.elapsed() < idle_ttl
        });
        before - self.sessions.len()
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
        // drain_pending no longer inflates stored_count. The caller increments
        // it via record_stored() only after the upstream confirms success.
        assert_eq!(s.stored_count, 0);
        s.record_stored(drained.len());
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

        let s = mgr.get_or_create("session-2");
        assert_eq!(s.id, "session-2");
        assert_eq!(mgr.active_count(), 2);

        let s = mgr.get_or_create("session-2");
        assert_eq!(s.id, "session-2");
        assert_eq!(mgr.active_count(), 2);
    }

    #[test]
    fn test_session_manager_start_session() {
        let mut mgr = SessionManager::new("default".to_string());

        let result = mgr.start_session("new-session".to_string());
        assert!(result.is_ok());
        assert_eq!(mgr.active_count(), 2);

        let result = mgr.start_session("new-session".to_string());
        assert!(result.is_err());

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
        assert_eq!(mgr.total_count(), 3);

        let result = mgr.end_session("s1");
        assert!(result.is_err());

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
        std::thread::sleep(std::time::Duration::from_millis(5));
        s.add_observation(obs);
        let after = s.pending_since.expect("still set");
        assert_eq!(first, after, "pending_since tracks oldest, not newest");
    }

    #[test]
    fn record_stored_bumps_count_saturating() {
        let mut s = Session::new("t".into());
        assert_eq!(s.stored_count, 0);
        s.record_stored(3);
        s.record_stored(4);
        assert_eq!(s.stored_count, 7);
        s.record_stored(usize::MAX);
        assert_eq!(s.stored_count, usize::MAX);
    }

    #[test]
    fn requeue_prepends_failed_and_rearms_pending_since() {
        let mut s = Session::new("t".into());
        let obs = |n: u32| Observation {
            tool_name: format!("t{}", n),
            content: format!("c{}", n),
            importance: 1,
            category: "d".into(),
            timestamp: Utc::now(),
        };

        // Add two, drain (which clears pending_since), add one more that
        // arrived while the flush was in flight.
        s.add_observation(obs(1));
        s.add_observation(obs(2));
        let failed = s.drain_pending();
        assert!(s.pending_since.is_none());
        s.add_observation(obs(3));

        // Requeue the failed drain. The restored entries must land ahead of
        // the newer observation and pending_since must be set again.
        s.requeue(failed);
        assert_eq!(s.pending.len(), 3);
        assert_eq!(s.pending[0].tool_name, "t1");
        assert_eq!(s.pending[1].tool_name, "t2");
        assert_eq!(s.pending[2].tool_name, "t3");
        assert!(s.pending_since.is_some());
    }

    #[test]
    fn requeue_empty_is_noop() {
        let mut s = Session::new("t".into());
        s.requeue(Vec::new());
        assert!(s.pending.is_empty());
        assert!(s.pending_since.is_none());
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
}
