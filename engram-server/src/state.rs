use engram_lib::config::{Config, EidolonConfig};
use engram_lib::db::Database;
use engram_lib::embeddings::EmbeddingProvider;
use engram_lib::llm::local::LocalModelClient;
use engram_lib::reranker::Reranker;
use engram_lib::services::brain::BrainManager;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, watch, RwLock};

pub struct SessionBroadcast {
    pub buffer: Vec<String>,
    pub tx: broadcast::Sender<String>,
}

impl SessionBroadcast {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(1024);
        SessionBroadcast {
            buffer: Vec::new(),
            tx,
        }
    }
}

impl Default for SessionBroadcast {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Database>,
    pub config: Arc<Config>,
    pub embedder: Option<Arc<dyn EmbeddingProvider>>,
    pub reranker: Option<Arc<Reranker>>,
    pub brain: Option<Arc<BrainManager>>,
    #[allow(dead_code)]
    pub llm: Option<Arc<LocalModelClient>>,
    pub sessions: Arc<RwLock<HashMap<String, Arc<tokio::sync::Mutex<SessionBroadcast>>>>>,
    #[allow(dead_code)]
    pub eidolon_config: Option<EidolonConfig>,
    /// Notification channel for approval events. TUI clients can subscribe to
    /// be notified when approvals are created or decided.
    pub approval_notify: Option<watch::Sender<()>>,
}
