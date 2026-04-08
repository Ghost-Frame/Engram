use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct SessionState {
    pub agent: String,
    pub mode: Option<String>,
    pub source_boosts: HashMap<String, f64>,
    pub recent_queries: VecDeque<String>,
}

const MAX_RECENT_QUERIES: usize = 50;

impl SessionState {
    pub fn new(agent: String, mode: Option<String>) -> Self {
        Self {
            agent,
            mode,
            source_boosts: HashMap::new(),
            recent_queries: VecDeque::new(),
        }
    }

    pub fn record_query(&mut self, query: String) {
        if self.recent_queries.len() >= MAX_RECENT_QUERIES {
            self.recent_queries.pop_front();
        }
        self.recent_queries.push_back(query);
    }

    pub fn set_mode(&mut self, mode: Option<String>) {
        self.mode = mode;
    }

    pub fn set_source_boost(&mut self, source: String, boost: f64) {
        self.source_boosts.insert(source, boost);
    }
}

pub type SharedSession = Arc<RwLock<SessionState>>;

pub fn new_shared_session(agent: String, mode: Option<String>) -> SharedSession {
    Arc::new(RwLock::new(SessionState::new(agent, mode)))
}
