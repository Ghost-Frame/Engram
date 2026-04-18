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
    start_vector_sync_replay_task,
};
use kleos_server::dreamer::{new_stats_handle, start_dreamer_task};
use kleos_server::state::AppState;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;

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

    // Start background PageRank refresh job if enabled.
    let pagerank_handles = if state.config.pagerank_enabled {
        let (token, handle) =
            start_pagerank_refresh_job(Arc::clone(&state.db), Arc::clone(&state.config));
        tracing::info!("background pagerank refresh job started");
        Some((token, handle))
    } else {
        tracing::info!("pagerank disabled -- skipping refresh job");
        None
    };

    // Start background dreamer (intelligence pipeline + brain dream cycle).
    let dreamer_handles = if state.config.dreamer_enabled {
        let (token, handle) = start_dreamer_task(
            Arc::clone(&state.db),
            Arc::clone(&state.config),
            state.brain.clone(),
            Arc::clone(&state.dreamer_stats),
            Arc::clone(&state.last_request_time),
        );
        tracing::info!(
            interval_secs = state.config.dream_interval_secs,
            "dreamer background task started"
        );
        Some((token, handle))
    } else {
        tracing::info!("dreamer disabled -- skipping");
        None
    };

    // Start infrastructure background tasks.
    let (checkpoint_token, checkpoint_handle) = start_auto_checkpoint_task(Arc::clone(&state.db));
    tracing::info!("auto-checkpoint background task started");

    let (job_cleanup_token, job_cleanup_handle) = start_job_cleanup_task(Arc::clone(&state.db));
    tracing::info!("job-cleanup background task started");

    let (vector_sync_token, vector_sync_handle) =
        start_vector_sync_replay_task(Arc::clone(&state.db));
    tracing::info!("vector-sync-replay background task started");

    let backup_handles = if state.config.backup_enabled {
        let (token, handle) = start_auto_backup_task(
            Arc::clone(&state.db),
            state.config.data_dir.clone(),
            state.config.backup_dir.clone(),
            state.config.backup_interval_secs,
            state.config.backup_retention,
            state.config.backup_retention_daily,
        );
        tracing::info!(
            interval_secs = state.config.backup_interval_secs,
            retention = state.config.backup_retention,
            retention_daily = state.config.backup_retention_daily,
            "auto-backup background task started"
        );
        Some((token, handle))
    } else {
        tracing::info!("auto-backup disabled -- skipping");
        None
    };

    // Monitor background tasks -- log if any exit unexpectedly (panic/abort).
    tokio::spawn(async move {
        // Keep tokens alive so tasks aren't cancelled.
        let _checkpoint_token = checkpoint_token;
        let _job_cleanup_token = job_cleanup_token;
        let _vector_sync_token = vector_sync_token;
        let _pagerank_token = pagerank_handles.as_ref().map(|(t, _)| t);
        let _backup_token = backup_handles.as_ref().map(|(t, _)| t);
        let _dreamer_token = dreamer_handles.as_ref().map(|(t, _)| t);

        let mut tasks: Vec<(&str, tokio::task::JoinHandle<()>)> = vec![
            ("auto-checkpoint", checkpoint_handle),
            ("job-cleanup", job_cleanup_handle),
            ("vector-sync-replay", vector_sync_handle),
        ];
        if let Some((_, handle)) = pagerank_handles {
            tasks.push(("pagerank-refresh", handle));
        }
        if let Some((_, handle)) = backup_handles {
            tasks.push(("auto-backup", handle));
        }
        if let Some((_, handle)) = dreamer_handles {
            tasks.push(("dreamer", handle));
        }

        // Wait for ANY task to exit -- they should all run forever.
        loop {
            if tasks.is_empty() {
                break;
            }
            let (idx, result) = {
                let mut futs: Vec<_> = tasks.iter_mut().map(|(_, h)| h).collect();
                // Select the first task that completes.
                let (result, index, _) = futures::future::select_all(&mut futs).await;
                (index, result)
            };
            let (name, _) = tasks.remove(idx);
            match result {
                Ok(()) => {
                    tracing::error!(task = name, "background task exited unexpectedly");
                }
                Err(e) => {
                    tracing::error!(task = name, error = %e, "background task panicked");
                }
            }
        }
    });

    if let Err(e) = kleos_server::server::run(state).await {
        tracing::error!("server error: {}", e);
        std::process::exit(1);
    }
}
