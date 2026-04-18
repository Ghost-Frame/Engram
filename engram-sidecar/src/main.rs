mod auth;
mod routes;
mod session;
mod store;
mod watcher;

use clap::Parser;
use engram_lib::llm::{local::LocalModelClient, OllamaConfig};
use std::sync::Arc;
use tokio::sync::RwLock;

use session::SessionManager;
use store::SessionStore;

#[derive(Parser, Debug, Clone)]
#[command(
    name = "engram-sidecar",
    about = "Engram memory sidecar for agent sessions"
)]
struct Cli {
    #[arg(short, long, default_value = "7711", env = "ENGRAM_SIDECAR_PORT")]
    port: u16,

    #[arg(long, default_value = "127.0.0.1", env = "ENGRAM_SIDECAR_HOST")]
    host: String,

    #[arg(long)]
    session_id: Option<String>,

    #[arg(long, default_value = "sidecar", env = "ENGRAM_SIDECAR_SOURCE")]
    source: String,

    #[arg(long, default_value = "1", env = "ENGRAM_SIDECAR_USER_ID")]
    user_id: i64,

    /// Shared-secret token clients must send as `Authorization: Bearer <token>`.
    /// If unset, a fresh token is generated at startup.
    #[arg(long, env = "ENGRAM_SIDECAR_TOKEN")]
    token: Option<String>,

    /// Engram server URL for memory storage/retrieval.
    #[arg(long, env = "ENGRAM_URL")]
    engram_url: String,

    /// API key for authenticating with the Engram server.
    #[arg(long, env = "ENGRAM_API_KEY")]
    engram_api_key: Option<String>,

    /// Enable file watcher for Claude Code session JSONL files.
    /// Watches ~/.claude/projects/ (or CLAUDE_SESSIONS_DIR) for changes.
    #[arg(long, env = "ENGRAM_SIDECAR_WATCH")]
    watch: bool,

    /// Directory to watch for session files (default: ~/.claude/projects).
    #[arg(long, env = "CLAUDE_SESSIONS_DIR")]
    watch_dir: Option<String>,

    /// SQLite path for session persistence. When unset, sessions are kept
    /// in memory only and lost on restart.
    #[arg(long, env = "ENGRAM_SIDECAR_STORE_PATH")]
    store_path: Option<String>,

    /// Interval (seconds) between persistent-store checkpoints.
    #[arg(long, env = "ENGRAM_SIDECAR_STORE_INTERVAL_SECS", default_value = "60")]
    store_interval_secs: u64,

    /// Size-based flush threshold: flush pending observations to the server
    /// once a session reaches this many queued observations. Set to 1 to
    /// restore the pre-batching per-observation flush behavior.
    #[arg(long, env = "ENGRAM_SIDECAR_BATCH_SIZE", default_value = "10")]
    batch_size: usize,

    /// Time-based flush interval (milliseconds): any session with pending
    /// observations older than this is flushed even if it hasn't hit
    /// --batch-size yet. Keeps latency bounded on low-traffic sessions.
    #[arg(long, env = "ENGRAM_SIDECAR_BATCH_INTERVAL_MS", default_value = "2000")]
    batch_interval_ms: u64,
}

#[derive(Clone)]
pub struct SidecarState {
    pub client: reqwest::Client,
    pub engram_url: String,
    pub engram_api_key: Option<String>,
    pub llm: Arc<LocalModelClient>,
    pub sessions: Arc<RwLock<SessionManager>>,
    pub source: String,
    pub user_id: i64,
    pub token: Option<String>,
    /// Optional persistent session store. When present, the sidecar flushes
    /// every session on a timer and exposes `/session/{id}/resume`.
    pub session_store: Option<SessionStore>,
    /// Size-based flush trigger: observe-path flushes once a session's
    /// pending queue reaches this many observations. Minimum 1.
    pub batch_size: usize,
    /// Time-based flush trigger (milliseconds): the background flusher
    /// drains any session whose oldest pending observation is older than
    /// this. Zero disables the time trigger entirely.
    pub batch_interval_ms: u64,
}

#[tokio::main]
async fn main() {
    let _otel_guard =
        engram_lib::observability::init_tracing("engram-sidecar", "engram_sidecar=debug");

    let cli = Cli::parse();

    // HTTP client for Engram server API calls
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("failed to create HTTP client");

    // Create LLM client and probe once. Even if probe fails, we keep the
    // client so it can be re-probed later when Ollama becomes available.
    let llm: Arc<LocalModelClient> = {
        let llm_config = OllamaConfig::from_env();
        let client = LocalModelClient::new(llm_config);
        if client.probe().await {
            tracing::info!("local LLM client ready for sidecar");
        } else {
            tracing::warn!(
                "local LLM unavailable at startup -- will re-probe on first compress request"
            );
        }
        Arc::new(client)
    };

    let session_id = cli
        .session_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    tracing::info!(default_session_id = %session_id, "starting sidecar (multi-session enabled)");

    let token = match cli
        .token
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(t) => Some(t.to_string()),
        None => {
            let generated = auth::generate_token();
            // SECURITY: log that a token was generated but do NOT log the value.
            // Use --token flag or ENGRAM_SIDECAR_TOKEN env to set explicitly.
            tracing::warn!(
                host = %cli.host,
                "ENGRAM_SIDECAR_TOKEN not set; generated one-time sidecar token (printed to stderr)"
            );
            // SECURITY (SEC-LOW-5): print token once to stderr so the launching
            // process can capture it. Only the first 8 hex chars are shown in the
            // log line; the full value is on a separate machine-parseable line.
            // Ensure stderr is NOT forwarded to persistent log files.
            eprintln!("SIDECAR_TOKEN={}", generated);
            tracing::debug!(
                token_prefix = &generated[..8.min(generated.len())],
                "sidecar token generated (see stderr for full value)"
            );
            Some(generated)
        }
    };
    if token.is_some() {
        tracing::info!("sidecar shared-secret auth enabled");
    } else {
        tracing::info!(
            host = %cli.host,
            "no ENGRAM_SIDECAR_TOKEN set; running without auth (localhost-only)"
        );
    }

    // Open the persistent session store if configured, and hydrate the
    // manager with any sessions recovered from disk. A failure here is
    // non-fatal: we log + fall back to in-memory only, because losing
    // persistence should not take down the sidecar.
    let mut manager = SessionManager::new(session_id);
    let session_store = if let Some(path) = cli
        .store_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        match SessionStore::open(path).await {
            Ok(store) => {
                match store.load_all().await {
                    Ok(snaps) => {
                        let recovered = snaps.len();
                        for snap in snaps {
                            manager.restore_snapshot(snap);
                        }
                        tracing::info!(
                            path = path,
                            recovered,
                            "restored sessions from persistent store"
                        );
                    }
                    Err(e) => {
                        tracing::warn!("failed to load sessions from store: {}", e);
                    }
                }
                Some(store)
            }
            Err(e) => {
                tracing::warn!(
                    path = path,
                    "failed to open session store -- running in-memory only: {}",
                    e
                );
                None
            }
        }
    } else {
        tracing::info!("ENGRAM_SIDECAR_STORE_PATH unset -- sessions will not survive restart");
        None
    };

    let state = SidecarState {
        client,
        engram_url: cli.engram_url,
        engram_api_key: cli.engram_api_key,
        llm,
        sessions: Arc::new(RwLock::new(manager)),
        source: cli.source,
        user_id: cli.user_id,
        token,
        session_store: session_store.clone(),
        batch_size: cli.batch_size.max(1),
        batch_interval_ms: cli.batch_interval_ms,
    };

    tracing::info!(
        batch_size = state.batch_size,
        batch_interval_ms = state.batch_interval_ms,
        "observation batching configured"
    );

    // Start file watcher in background (if enabled)
    if cli.watch {
        if let Some(ref dir) = cli.watch_dir {
            std::env::set_var("CLAUDE_SESSIONS_DIR", dir);
        }
        let _watcher_handle = watcher::start(state.clone());
    }

    // Kick off the persistent-store checkpoint loop when a store is set.
    // The loop wakes every N seconds, snapshots every session, and writes
    // the batch in one transaction. Errors are logged but never fatal.
    if let Some(store) = session_store {
        let sessions = Arc::clone(&state.sessions);
        let interval = cli.store_interval_secs.max(5);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(interval));
            tick.tick().await; // skip the immediate first tick
            loop {
                tick.tick().await;
                let snaps = {
                    let guard = sessions.read().await;
                    guard.snapshot_all()
                };
                if snaps.is_empty() {
                    continue;
                }
                let count = snaps.len();
                match store.save_batch(snaps).await {
                    Ok(()) => tracing::debug!(count, "session store checkpoint complete"),
                    Err(e) => tracing::warn!("session store checkpoint failed: {}", e),
                }
            }
        });
        tracing::info!(
            interval_secs = interval,
            "session store checkpoint task started"
        );
    }

    // Time-based batch flusher. Periodically scans for sessions whose oldest
    // pending observation has aged past `batch_interval_ms` and flushes them
    // so low-traffic sessions don't wait for `batch_size` before writing.
    // Disabled when --batch-interval-ms is 0.
    if state.batch_interval_ms > 0 {
        let flusher_state = state.clone();
        let interval_ms = state.batch_interval_ms;
        // Check twice per interval so worst-case latency is ~1.5x interval.
        let tick_ms = interval_ms.div_ceil(2).max(100);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_millis(tick_ms));
            tick.tick().await; // skip the immediate first tick
            let threshold = std::time::Duration::from_millis(interval_ms);
            loop {
                tick.tick().await;
                // Snapshot candidate session ids under a read lock; flush_pending
                // takes its own write lock per session so we can't hold one here.
                let candidates: Vec<String> = {
                    let guard = flusher_state.sessions.read().await;
                    guard
                        .list()
                        .into_iter()
                        .filter(|info| info.pending_count > 0 && !info.ended)
                        .map(|info| info.id)
                        .collect()
                };
                for sid in candidates {
                    let due = {
                        let guard = flusher_state.sessions.read().await;
                        guard
                            .get(&sid)
                            .and_then(|s| s.pending_since)
                            .map(|t| t.elapsed() >= threshold)
                            .unwrap_or(false)
                    };
                    if due {
                        let flushed = routes::flush_pending(&flusher_state, &sid).await;
                        if flushed > 0 {
                            tracing::debug!(
                                session_id = %sid,
                                flushed,
                                "time-based batch flush"
                            );
                        }
                    }
                }
            }
        });
        tracing::info!(
            interval_ms = state.batch_interval_ms,
            tick_ms,
            "time-based batch flusher started"
        );
    } else {
        tracing::info!("time-based batch flusher disabled (batch_interval_ms=0)");
    }

    let app = routes::router(state);
    let addr = format!("{}:{}", cli.host, cli.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("failed to bind");

    tracing::info!(addr = %addr, "sidecar listening");

    axum::serve(listener, app).await.expect("server error");
}
