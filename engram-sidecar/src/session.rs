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
        }
    }

    pub fn add_observation(&mut self, obs: Observation) -> usize {
        self.observation_count += 1;
        self.pending.push(obs);
        self.pending.len()
    }

    pub fn drain_pending(&mut self) -> Vec<Observation> {
        let drained: Vec<Observation> = self.pending.drain(..).collect();
        self.stored_count += drained.len();
        drained
    }

    pub fn end(&mut self) {
        self.ended = true;
    }
}

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
}
