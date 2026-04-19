use kleos_lib::config::{Config, EncryptionMode};
use kleos_lib::cred::CreddClient;
use kleos_lib::db::Database;
use kleos_lib::embeddings::onnx::OnnxProvider;
use kleos_lib::embeddings::EmbeddingProvider;
use kleos_lib::jobs::pagerank_refresh::start_pagerank_refresh_job;
use kleos_lib::llm::{local::LocalModelClient, OllamaConfig};
use kleos_lib::reranker::{self, Reranker};
use kleos_lib::services::brain::create_brain_backend;
use kleos_server::background::{
    start_auto_backup_task, start_auto_checkpoint_task, start_job_cleanup_task,
    start_session_reaper_task, start_vector_sync_replay_task,
};
use kleos_server::dreamer::{new_stats_handle, start_dreamer_task};
use kleos_server::state::AppState;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() {
    kleos_lib::config::migrate_env_prefix();

    let _otel_guard = kleos_lib::observability::init_tracing(
        "engram-server",
        "kleos_server=debug,tower_http=debug",
    );

    let config = Config::load();

    // Install Prometheus metrics recorder before any metrics are emitted.
    kleos_server::middleware::metrics::init_metrics();

    // Resolve at-rest encryption key based on configured mode.
    let encryption_key = match config.encryption.mode {
        EncryptionMode::None => None,
        EncryptionMode::Yubikey => {
            tracing::info!("encryption mode: yubikey -- touch slot 2 to unlock database...");
            let challenge = kleos_cred::yubikey::get_or_create_challenge().unwrap_or_else(|e| {
                eprintln!("failed to load YubiKey challenge: {e}");
                std::process::exit(1);
            });
            let response =
                kleos_cred::yubikey::challenge_response(&challenge).unwrap_or_else(|e| {
                    eprintln!("YubiKey challenge-response failed: {e}");
                    std::process::exit(1);
                });
            Some(kleos_cred::crypto::derive_key(0, b"", Some(&response)))
        }
        _ => {
            let mode_name = format!("{:?}", config.encryption.mode).to_ascii_lowercase();
            tracing::info!("encryption mode: {}", mode_name);
            kleos_lib::encryption::resolve_key(&config).unwrap_or_else(|e| {
                eprintln!("encryption key resolution failed: {e}");
                std::process::exit(1);
            })
        }
    };

    let db = Database::connect_with_config(&config, encryption_key)
        .await
        .expect("failed to connect to database");

    // Wrap in Arc early so background tasks (reranker, embedder) can share it.
    let db_arc = Arc::new(db);

    // Deferred embedder/reranker initialization -- server starts immediately, models load in background
    let embedder: Arc<tokio::sync::RwLock<Option<Arc<dyn EmbeddingProvider>>>> =
        Arc::new(tokio::sync::RwLock::new(None));
    let reranker: Arc<tokio::sync::RwLock<Option<Arc<dyn Reranker>>>> =
        Arc::new(tokio::sync::RwLock::new(None));

    // Spawn background task to load embedding model
    {
        let embedder = Arc::clone(&embedder);
        let config = config.clone();
        tokio::spawn(async move {
            tracing::info!("loading ONNX embedding model in background...");
            match OnnxProvider::new(&config).await {
                Ok(provider) => {
                    // 6.11 pre-warm: one dummy embed so the first real
                    // request avoids ONNX session + allocator cold start.
                    let prewarm_start = std::time::Instant::now();
                    match provider.embed("warmup").await {
                        Ok(_) => tracing::info!(
                            elapsed_ms = prewarm_start.elapsed().as_millis() as u64,
                            "embedder pre-warm complete"
                        ),
                        Err(e) => tracing::warn!("embedder pre-warm failed: {}", e),
                    }
                    let mut guard = embedder.write().await;
                    *guard = Some(Arc::new(provider));
                    tracing::info!("ONNX embedding provider ready");
                }
                Err(e) => {
                    tracing::warn!(
                        "ONNX embedding provider failed to initialize: {}. Vector search disabled.",
                        e
                    );
                }
            }
        });
    }

    // Spawn background task to load reranker
    if config.reranker_enabled {
        let reranker = Arc::clone(&reranker);
        let config = config.clone();
        let reranker_db = Arc::clone(&db_arc);
        tokio::spawn(async move {
            tracing::info!("loading reranker in background...");
            match reranker::create_reranker(&config, Some(reranker_db)).await {
                Ok(Some(r)) => {
                    tracing::info!(backend = r.backend_name(), "reranker ready");
                    let mut guard = reranker.write().await;
                    *guard = Some(r);
                }
                Ok(None) => {
                    tracing::info!("reranker disabled by backend config");
                }
                Err(e) => {
                    tracing::warn!(
                        "reranker failed to initialize: {}. Results will not be reranked.",
                        e
                    );
                }
            }
        });
    }

    // Initialize local LLM client (graceful degradation if unavailable)
    let llm: Option<Arc<LocalModelClient>> = {
        let config = OllamaConfig::from_env();
        let client = LocalModelClient::new(config);
        if client.probe().await {
            tracing::info!("local LLM client ready");
            Some(Arc::new(client))
        } else {
            tracing::warn!("local LLM unavailable. LLM-dependent features disabled.");
            None
        }
    };

    // Initialize brain backend (Hopfield in-process or subprocess eidolon)
    let data_dir = config.data_dir.clone();
    let brain = create_brain_backend(Arc::clone(&db_arc), &data_dir, 1).await;

    // Approval notification channel for TUI clients
    let (approval_tx, _) = tokio::sync::watch::channel(());

    // Record this startup as a potential crash/restart event, then decide
    // whether the server has been crash-looping.
    if let Err(e) = kleos_lib::admin::record_crash(&db_arc).await {
        tracing::warn!("failed to record startup crash timestamp: {}", e);
    }
    let safe_mode_active = kleos_lib::admin::should_enter_safe_mode(&db_arc)
        .await
        .unwrap_or(false);
    if safe_mode_active {
        tracing::warn!("SAFE MODE ACTIVE: 3+ restarts in last 5 minutes");
        tracing::warn!("Write operations will return 503");
        tracing::warn!("POST /admin/safe-mode/exit to recover");
    }

    let state = AppState {
        db: db_arc,
        credd: Arc::new(CreddClient::from_config(&config)),
        config: Arc::new(config),
        embedder,
        reranker,
        brain,
        llm,
        sessions: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        eidolon_config: None,
        approval_notify: Some(approval_tx),
        pending_approvals: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        safe_mode: Arc::new(AtomicBool::new(safe_mode_active)),
        dreamer_stats: new_stats_handle(),
        last_request_time: Arc::new(AtomicU64::new(0)),
    };

    // R8 R-008: every background task is described by a factory so the
    // supervisor can respawn it after a panic. Each factory captures the Arc
    // state it needs and returns a fresh (CancellationToken, JoinHandle) on
    // each invocation.
    let mut supervised: Vec<Supervised> = Vec::new();

    if state.config.pagerank_enabled {
        let db = Arc::clone(&state.db);
        let cfg = Arc::clone(&state.config);
        supervised.push(Supervised::spawn("pagerank-refresh", move || {
            start_pagerank_refresh_job(Arc::clone(&db), Arc::clone(&cfg))
        }));
        tracing::info!("background pagerank refresh job started");
    } else {
        tracing::info!("pagerank disabled -- skipping refresh job");
    }

    if state.config.dreamer_enabled {
        let db = Arc::clone(&state.db);
        let cfg = Arc::clone(&state.config);
        let brain = state.brain.clone();
        let llm = state.llm.clone();
        let stats = Arc::clone(&state.dreamer_stats);
        let last_req = Arc::clone(&state.last_request_time);
        supervised.push(Supervised::spawn("dreamer", move || {
            start_dreamer_task(
                Arc::clone(&db),
                Arc::clone(&cfg),
                brain.clone(),
                llm.clone(),
                Arc::clone(&stats),
                Arc::clone(&last_req),
            )
        }));
        tracing::info!(
            interval_secs = state.config.dream_interval_secs,
            "dreamer background task started"
        );
    } else {
        tracing::info!("dreamer disabled -- skipping");
    }

    {
        let db = Arc::clone(&state.db);
        supervised.push(Supervised::spawn("auto-checkpoint", move || {
            start_auto_checkpoint_task(Arc::clone(&db))
        }));
        tracing::info!("auto-checkpoint background task started");
    }

    {
        let db = Arc::clone(&state.db);
        supervised.push(Supervised::spawn("job-cleanup", move || {
            start_job_cleanup_task(Arc::clone(&db))
        }));
        tracing::info!("job-cleanup background task started");
    }

    {
        let db = Arc::clone(&state.db);
        supervised.push(Supervised::spawn("vector-sync-replay", move || {
            start_vector_sync_replay_task(Arc::clone(&db))
        }));
        tracing::info!("vector-sync-replay background task started");
    }

    if state.config.backup_enabled {
        let db = Arc::clone(&state.db);
        let data_dir = state.config.data_dir.clone();
        let backup_dir = state.config.backup_dir.clone();
        let interval = state.config.backup_interval_secs;
        let retention = state.config.backup_retention;
        let retention_daily = state.config.backup_retention_daily;
        supervised.push(Supervised::spawn("auto-backup", move || {
            start_auto_backup_task(
                Arc::clone(&db),
                data_dir.clone(),
                backup_dir.clone(),
                interval,
                retention,
                retention_daily,
            )
        }));
        tracing::info!(
            interval_secs = state.config.backup_interval_secs,
            retention = state.config.backup_retention,
            retention_daily = state.config.backup_retention_daily,
            "auto-backup background task started"
        );
    } else {
        tracing::info!("auto-backup disabled -- skipping");
    }

    {
        let sessions = Arc::clone(&state.sessions);
        supervised.push(Supervised::spawn("session-reaper", move || {
            start_session_reaper_task(Arc::clone(&sessions))
        }));
        tracing::info!("session-reaper background task started");
    }

    // R8 R-008: share one CancellationToken between the HTTP server and the
    // supervisor so SIGTERM propagates through both. Without this the
    // background tasks keep writing to SQLite as axum tears down.
    let shutdown = CancellationToken::new();
    {
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            kleos_server::server::shutdown_signal().await;
            shutdown.cancel();
        });
    }

    let supervisor_handle = {
        let shutdown = shutdown.clone();
        tokio::spawn(async move { supervise(supervised, shutdown).await })
    };

    if let Err(e) = kleos_server::server::run(state, shutdown.clone()).await {
        tracing::error!("server error: {}", e);
        shutdown.cancel();
        let _ = supervisor_handle.await;
        std::process::exit(1);
    }

    // Graceful path: axum shutdown already observed the same token, so we
    // just need to wait for the supervisor to drain its children.
    shutdown.cancel();
    if let Err(e) = supervisor_handle.await {
        tracing::error!(error = %e, "supervisor task exit error");
    }
}

/// R8 R-008: one respawnable background task.
///
/// `factory` constructs a fresh `(CancellationToken, JoinHandle)` each time it
/// is invoked so the supervisor can restart the task after a panic without
/// carrying over the cancelled token from the previous generation.
struct Supervised {
    name: &'static str,
    factory: Box<dyn FnMut() -> (CancellationToken, JoinHandle<()>) + Send>,
    cancel: CancellationToken,
    handle: JoinHandle<()>,
    consecutive_failures: u32,
}

impl Supervised {
    fn spawn<F>(name: &'static str, mut factory: F) -> Self
    where
        F: FnMut() -> (CancellationToken, JoinHandle<()>) + Send + 'static,
    {
        let (cancel, handle) = factory();
        Self {
            name,
            factory: Box::new(factory),
            cancel,
            handle,
            consecutive_failures: 0,
        }
    }
}

/// R8 R-008: supervise background tasks with exponential-backoff respawn and
/// a shared shutdown token. Exits cleanly only after every child's
/// CancellationToken has been signalled and its JoinHandle awaited.
async fn supervise(mut tasks: Vec<Supervised>, shutdown: CancellationToken) {
    const MAX_BACKOFF: Duration = Duration::from_secs(300);

    loop {
        if tasks.is_empty() {
            return;
        }
        if shutdown.is_cancelled() {
            for t in &tasks {
                t.cancel.cancel();
            }
            for t in tasks {
                let _ = t.handle.await;
            }
            return;
        }

        let (idx, result) = {
            let mut futs: Vec<_> = tasks.iter_mut().map(|t| &mut t.handle).collect();
            tokio::select! {
                _ = shutdown.cancelled() => {
                    continue;
                }
                (r, i, _) = futures::future::select_all(&mut futs) => (i, r),
            }
        };

        {
            let t = &tasks[idx];
            match &result {
                Ok(()) => tracing::error!(task = t.name, "background task exited unexpectedly"),
                Err(e) => {
                    tracing::error!(task = t.name, error = %e, "background task panicked")
                }
            }
        }

        let backoff = {
            let t = &mut tasks[idx];
            t.consecutive_failures = t.consecutive_failures.saturating_add(1);
            let exp = t.consecutive_failures.min(8).saturating_sub(1);
            Duration::from_secs(2u64.pow(exp)).min(MAX_BACKOFF)
        };
        let name = tasks[idx].name;
        let attempts = tasks[idx].consecutive_failures;
        tracing::warn!(
            task = name,
            secs = backoff.as_secs(),
            attempts,
            "respawning after backoff"
        );

        tokio::select! {
            _ = shutdown.cancelled() => continue,
            _ = tokio::time::sleep(backoff) => {}
        }

        let (new_cancel, new_handle) = (tasks[idx].factory)();
        tasks[idx].cancel = new_cancel;
        tasks[idx].handle = new_handle;
        tracing::info!(task = name, attempts, "background task respawned");
    }
}
