use engram_lib::config::{Config, EidolonConfig};
use engram_lib::cred::CreddClient;
use engram_lib::db::Database;
use engram_lib::embeddings::EmbeddingProvider;
use engram_lib::gate::PendingApproval;
use engram_lib::llm::local::LocalModelClient;
use engram_lib::reranker::Reranker;
use engram_lib::services::brain::BrainBackend;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::{broadcast, watch, Mutex, RwLock};

pub struct SessionBroadcast {
    pub buffer: VecDeque<String>,
    pub tx: broadcast::Sender<String>,
}

impl SessionBroadcast {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(1024);
        SessionBroadcast {
            buffer: VecDeque::new(),
            tx,
        }
    }
}

impl Default for SessionBroadcast {
    fn default() -> Self {
        Self::new()
    }
}

/// SECURITY (MT-F10): session map keyed by `(user_id, session_id)` so two
/// tenants cannot collide on the same opaque session id.
pub type SessionMap =
    Arc<RwLock<HashMap<(i64, String), Arc<tokio::sync::Mutex<SessionBroadcast>>>>>;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Database>,
    pub config: Arc<Config>,
    pub credd: Arc<CreddClient>,
    pub embedder: Arc<RwLock<Option<Arc<dyn EmbeddingProvider>>>>,
    pub reranker: Arc<RwLock<Option<Arc<Reranker>>>>,
    pub brain: Option<Arc<dyn BrainBackend>>,
    #[allow(dead_code)]
    pub llm: Option<Arc<LocalModelClient>>,
    pub sessions: SessionMap,
    #[allow(dead_code)]
    pub eidolon_config: Option<EidolonConfig>,
    /// Notification channel for approval events. TUI clients can subscribe to
    /// be notified when approvals are created or decided.
    pub approval_notify: Option<watch::Sender<()>>,
    /// Pending tool approvals waiting for a human decision via the respond endpoint.
    #[allow(clippy::type_complexity)]
    pub pending_approvals: Arc<Mutex<HashMap<i64, (PendingApproval, tokio::sync::oneshot::Sender<bool>)>>>,
    /// When true, write operations return 503 to prevent data corruption during crash loops.
    pub safe_mode: Arc<AtomicBool>,
}
