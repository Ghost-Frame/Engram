use kleos_lib::artifacts_crypto::ArtifactEncryption;
use kleos_lib::config::{Config, EidolonConfig};
use kleos_lib::cred::CreddClient;
use kleos_lib::db::Database;
use kleos_lib::embeddings::EmbeddingProvider;
use kleos_lib::gate::PendingApproval;
use kleos_lib::llm::local::LocalModelClient;
use kleos_lib::reranker::Reranker;
use kleos_lib::services::brain::BrainBackend;
use kleos_lib::tenant::TenantRegistry;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;
use tokio::sync::{broadcast, watch, Mutex, RwLock, Semaphore};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

/// Holds a broadcast channel and ring buffer for streaming session events to SSE subscribers.
pub struct SessionBroadcast {
    pub buffer: VecDeque<String>,
    pub tx: broadcast::Sender<String>,
    /// Monotonic-milliseconds timestamp of the most recent append. Used by
    /// the session reaper to evict idle entries (R8 R-010).
    pub last_activity: Arc<AtomicU64>,
}

/// Constructor and helper methods for [`SessionBroadcast`].
impl SessionBroadcast {
    /// Creates a `SessionBroadcast` with a 1024-slot broadcast channel and an empty ring buffer.
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(1024);
        SessionBroadcast {
            buffer: VecDeque::new(),
            tx,
            last_activity: Arc::new(AtomicU64::new(crate::dreamer::monotonic_millis())),
        }
    }
}

/// Provides a default [`SessionBroadcast`] by delegating to [`SessionBroadcast::new`].
impl Default for SessionBroadcast {
    /// Delegates to [`SessionBroadcast::new`].
    fn default() -> Self {
        Self::new()
    }
}

/// SECURITY (MT-F10): session map keyed by `(user_id, session_id)` so two
/// tenants cannot collide on the same opaque session id.
pub type SessionMap =
    Arc<RwLock<HashMap<(i64, String), Arc<tokio::sync::Mutex<SessionBroadcast>>>>>;

/// Central Axum application state shared across all request handlers.
///
/// Every field is `Arc`-wrapped (or cheaply `Clone`) so the derived `Clone` is shallow.
#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Database>,
    pub config: Arc<Config>,
    pub credd: Arc<CreddClient>,
    pub embedder: Arc<RwLock<Option<Arc<dyn EmbeddingProvider>>>>,
    pub reranker: Arc<RwLock<Option<Arc<dyn Reranker>>>>,
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
    pub pending_approvals:
        Arc<Mutex<HashMap<i64, (PendingApproval, tokio::sync::oneshot::Sender<bool>)>>>,
    /// When true, write operations return 503 to prevent data corruption during crash loops.
    pub safe_mode: Arc<AtomicBool>,
    /// Running stats from the background dreamer task.
    pub dreamer_stats: crate::dreamer::DreamerStatsHandle,
    /// Unix-seconds timestamp of the most recent HTTP request. Used by the
    /// dreamer to gate heavy consolidation work behind a period of idleness.
    pub last_request_time: Arc<AtomicU64>,
    /// Tenant registry for multi-tenant dreamer and background jobs.
    pub tenant_registry: Option<Arc<TenantRegistry>>,
    /// Semaphore throttling concurrent auto-GC spawns inside HandoffsDb.
    /// Always present; the HandoffsDb facade is rebuilt per request from
    /// the tenant registry's reserved "handoffs" shard.
    pub handoffs_gc_sem: Arc<Semaphore>,
    /// Shutdown token propagated into all background tasks so SIGTERM drains
    /// in-flight work rather than abandoning it (H-005/M-008).
    pub shutdown_token: CancellationToken,
    /// Process-wide JoinSet for all fire-and-forget background tasks.
    /// Wired to shutdown_token so tasks receive cancellation on SIGTERM.
    /// Arc<Mutex<...>> because handlers must lock briefly to push into it.
    pub background_tasks: Arc<Mutex<JoinSet<()>>>,
    /// Per-pattern semaphores throttle fire-and-forget spawn sites (H-005).
    /// Default: 64 permits each; override via KLEOS_BG_SEM_<NAME>=N.
    pub fact_extract_sem: Arc<Semaphore>,
    pub brain_absorb_sem: Arc<Semaphore>,
    pub ingest_sem: Arc<Semaphore>,
    /// Bounded channel into the dedicated audit-log worker ([57]): the
    /// middleware try_sends events here so the response path never awaits
    /// a permit or lock for audit persistence.
    pub audit_tx: tokio::sync::mpsc::Sender<crate::middleware::audit::AuditEvent>,
    pub replay_guard: Arc<kleos_lib::auth_piv::ReplayGuard>,
    pub session_manager: Arc<kleos_lib::auth_piv::SessionManager>,
    /// Broadcast channel for real-time Axon event delivery to SSE subscribers.
    /// The sender is cloned cheaply into each SSE connection via `subscribe()`.
    pub axon_broadcast: broadcast::Sender<serde_json::Value>,
    /// Optional AES-256-GCM encryption for artifact data blobs.
    /// Initialized from KLEOS_ARTIFACT_KEY env var (empty = disabled).
    pub artifact_encryption: Arc<ArtifactEncryption>,
    /// The database SQLCipher key (None when encryption is disabled). Needed by
    /// the backup/PITR routes to open encrypted snapshot files for verification
    /// and restore; the main DB pool already holds its own copy.
    pub encryption_key: Option<[u8; 32]>,
}

/// Accessor methods that clone `Arc`'d providers without holding locks across awaits.
impl AppState {
    /// Clone out the currently-loaded embedder without holding the RwLock
    /// across an await. The inner value is an `Arc` so the clone is cheap,
    /// and releasing the guard before `.embed()` means a concurrent reload
    /// (write lock) is not blocked for the entire embedding round-trip.
    pub async fn current_embedder(&self) -> Option<Arc<dyn EmbeddingProvider>> {
        self.embedder.read().await.clone()
    }

    /// Clone out the currently-loaded reranker. Same rationale as
    /// [`current_embedder`]: never hold the RwLock across an await.
    pub async fn current_reranker(&self) -> Option<Arc<dyn Reranker>> {
        self.reranker.read().await.clone()
    }
}
