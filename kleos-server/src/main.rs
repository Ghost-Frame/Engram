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
    start_job_worker_task, start_session_reaper_task, start_vector_sync_replay_task,
};
use kleos_server::dreamer::{new_stats_handle, start_dreamer_task};
use kleos_server::state::AppState;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() {
    kleos_lib::config::migrate_env_prefix();

    let _otel_guard = kleos_lib::observability::init_tracing(
        "kleos-server",
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
    let brain = create_brain_backend(Arc::clone(&db_arc), &data_dir).await;
    // M-014: keep a handle so we can call shutdown() after the server exits.
    let brain_for_shutdown = brain.clone();

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

    // C-R3-004: tenant sharding is ON by default. Set ENGRAM_TENANT_SHARDING
    // to "0" or "false" (case-insensitive) to fall back to monolith-only
    // single-user mode. Multi-user deployments MUST keep sharding enabled --
    // the ResolvedDb extractor refuses non-system users when the registry is
    // missing, so disabling sharding renders the server effectively
    // single-user (only user_id=1 keeps working).
    let tenant_sharding_enabled = match std::env::var("ENGRAM_TENANT_SHARDING") {
        Ok(v) => {
            let s = v.trim().to_ascii_lowercase();
            !(s == "0" || s == "false" || s == "off" || s == "no")
        }
        Err(_) => true,
    };
    let tenant_registry = if tenant_sharding_enabled {
        use kleos_lib::tenant::{TenantConfig, TenantRegistry};
        let reg = TenantRegistry::new(
            &config.data_dir,
            TenantConfig::default(),
            config.vector_dimensions,
        )
        .expect("failed to initialize tenant registry");
        tracing::info!("tenant sharding enabled (default)");
        Some(Arc::new(reg))
    } else {
        tracing::warn!(
            "tenant sharding DISABLED via ENGRAM_TENANT_SHARDING; \
             non-system users (user_id != 1) will receive 503 from any \
             tenant-scoped route. This mode is single-user only."
        );
        None
    };

    let handoffs_gc_sem = Arc::new(Semaphore::new(
        std::env::var("KLEOS_BG_SEM_GC")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8usize),
    ));
    // Eagerly create the reserved "handoffs" tenant so the shard exists at
    // first /handoffs/* request. The route handler resolves it via the
    // registry; failure here is fatal because the standalone handoffs.db
    // path no longer exists.
    if let Some(reg) = tenant_registry.as_ref() {
        if let Err(e) = reg
            .get_or_create(kleos_lib::tenant::HANDOFFS_TENANT_ID)
            .await
        {
            tracing::warn!(
                "failed to pre-warm handoffs tenant shard: {e}; routes will retry on first request"
            );
        } else {
            tracing::info!(
                "handoffs tenant shard ready: tenants/{}/",
                kleos_lib::tenant::HANDOFFS_TENANT_ID
            );
        }
    } else {
        tracing::warn!(
            "tenant sharding disabled; /handoffs/* routes will return 503 until enabled"
        );
    }

    // H-005: per-pattern semaphores cap concurrent fire-and-forget background tasks.
    // Each defaults to 64 permits; set KLEOS_BG_SEM_<NAME>=N to override.
    fn bg_sem(name: &str, default: usize) -> Arc<Semaphore> {
        let key = format!("KLEOS_BG_SEM_{}", name.to_ascii_uppercase());
        let n = std::env::var(&key)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default);
        Arc::new(Semaphore::new(n))
    }
    let fact_extract_sem = bg_sem("FACT_EXTRACT", 64);
    let brain_absorb_sem = bg_sem("BRAIN_ABSORB", 64);
    let audit_log_sem = bg_sem("AUDIT_LOG", 64);
    let ingest_sem = bg_sem("INGEST", 64);
    let background_tasks = Arc::new(tokio::sync::Mutex::new(JoinSet::<()>::new()));

    // Create the shutdown token early so it can be stored in AppState and shared
    // with background tasks spawned from HTTP handlers (H-005/M-008).
    let shutdown = CancellationToken::new();
    {
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            kleos_server::server::shutdown_signal().await;
            shutdown.cancel();
        });
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
        tenant_registry,
        handoffs_gc_sem,
        shutdown_token: shutdown.clone(),
        background_tasks: Arc::clone(&background_tasks),
        fact_extract_sem,
        brain_absorb_sem,
        audit_log_sem,
        ingest_sem,
        replay_guard: Arc::new(kleos_lib::auth_piv::ReplayGuard::new()),
        session_manager: Arc::new(
            kleos_lib::auth_piv::SessionManager::from_env_or_generate()
                .expect("failed to initialize session manager"),
        ),
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
        let registry = state.tenant_registry.clone();
        supervised.push(Supervised::spawn("dreamer", move || {
            start_dreamer_task(
                Arc::clone(&db),
                Arc::clone(&cfg),
                brain.clone(),
                llm.clone(),
                Arc::clone(&stats),
                Arc::clone(&last_req),
                registry.clone(),
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

    // Register handlers before the worker starts consuming so a pending job
    // claimed on the first tick finds its handler. Handlers close over
    // Arc<Database> -- the handler Fn is itself Arc-wrapped by the registry.
    register_job_handlers(Arc::clone(&state.db)).await;

    {
        let db = Arc::clone(&state.db);
        supervised.push(Supervised::spawn("job-worker", move || {
            start_job_worker_task(Arc::clone(&db))
        }));
        tracing::info!("job-worker background task started");
    }

    {
        let db = Arc::clone(&state.db);
        let registry = state.tenant_registry.clone();
        supervised.push(Supervised::spawn("vector-sync-replay", move || {
            start_vector_sync_replay_task(Arc::clone(&db), registry.clone())
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

    // R8 R-008: shutdown token already created and wired to the signal above;
    // the supervisor uses the same token so SIGTERM propagates through both.
    let supervisor_handle = {
        let shutdown = shutdown.clone();
        tokio::spawn(async move { supervise(supervised, shutdown).await })
    };

    if let Err(e) = kleos_server::server::run(state, shutdown.clone()).await {
        tracing::error!("server error: {}", e);
        shutdown.cancel();
        let _ = supervisor_handle.await;
        // Drain any remaining background tasks before exiting.
        let mut bg = background_tasks.lock().await;
        bg.abort_all();
        while bg.join_next().await.is_some() {}
        std::process::exit(1);
    }

    // Graceful path: axum shutdown already observed the same token, so we
    // just need to wait for the supervisor to drain its children.
    shutdown.cancel();
    if let Err(e) = supervisor_handle.await {
        tracing::error!(error = %e, "supervisor task exit error");
    }

    // Drain background tasks spawned from HTTP handlers with a 30-second cap.
    // These are fire-and-forget tasks (audit writes, fact extraction, brain
    // absorb, ingestion) that may still be in flight after axum drains HTTP.
    {
        let mut bg = background_tasks.lock().await;
        let drain_timeout = Duration::from_secs(30);
        tokio::select! {
            _ = async {
                while bg.join_next().await.is_some() {}
            } => {}
            _ = tokio::time::sleep(drain_timeout) => {
                tracing::warn!("background tasks drain timed out; aborting remainder");
                bg.abort_all();
                while bg.join_next().await.is_some() {}
            }
        }
    }

    // M-014: shut down the brain subprocess and its reader tasks.
    if let Some(b) = brain_for_shutdown {
        b.shutdown().await;
    }
}

/// Register every durable-job handler the server knows about. Handlers are
/// registered exactly once at startup, before the worker loop begins
/// consuming, so a pending job claimed on the first tick finds its handler.
///
/// Each handler closure captures the `Arc<Database>` it needs. The registry
/// wraps the closure in another `Arc`, so cheap handler clones are fine.
async fn register_job_handlers(db: Arc<Database>) {
    // ingestion.fact_extract -- durable fast_extract_facts invocation.
    // Payload: { "memory_id": i64, "content": string, "user_id": i64,
    //            "episode_id": i64|null }
    {
        let db = Arc::clone(&db);
        kleos_lib::jobs::register_job_handler("ingestion.fact_extract", move |payload| {
            let db = Arc::clone(&db);
            async move {
                let memory_id = payload.get("memory_id").and_then(|v| v.as_i64()).ok_or(
                    kleos_lib::EngError::InvalidInput(
                        "ingestion.fact_extract payload missing memory_id".into(),
                    ),
                )?;
                let content = payload
                    .get("content")
                    .and_then(|v| v.as_str())
                    .ok_or(kleos_lib::EngError::InvalidInput(
                        "ingestion.fact_extract payload missing content".into(),
                    ))?
                    .to_string();
                let user_id = payload.get("user_id").and_then(|v| v.as_i64()).ok_or(
                    kleos_lib::EngError::InvalidInput(
                        "ingestion.fact_extract payload missing user_id".into(),
                    ),
                )?;
                let episode_id = payload.get("episode_id").and_then(|v| v.as_i64());
                kleos_lib::intelligence::extraction::fast_extract_facts(
                    db.as_ref(),
                    &content,
                    memory_id,
                    user_id,
                    episode_id,
                )
                .await
                .map(|_| ())
            }
        })
        .await;
    }

    tracing::info!("durable job handlers registered");
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
