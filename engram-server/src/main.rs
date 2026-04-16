use engram_lib::config::{Config, EncryptionMode};
use engram_lib::cred::CreddClient;
use engram_lib::db::Database;
use engram_lib::embeddings::onnx::OnnxProvider;
use engram_lib::embeddings::EmbeddingProvider;
use engram_lib::jobs::pagerank_refresh::start_pagerank_refresh_job;
use engram_lib::llm::local::{LocalModelClient, OllamaConfig};
use engram_lib::reranker::Reranker;
use engram_lib::services::brain::create_brain_backend;
use engram_server::background::{
    start_auto_checkpoint_task, start_job_cleanup_task, start_vector_sync_replay_task,
};
use engram_server::state::AppState;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "engram_server=debug,tower_http=debug".into()),
        )
        .init();

    let config = Config::from_env();

    // Resolve at-rest encryption key based on configured mode.
    let encryption_key = match config.encryption.mode {
        EncryptionMode::None => None,
        EncryptionMode::Yubikey => {
            tracing::info!("encryption mode: yubikey -- touch slot 2 to unlock database...");
            let challenge = engram_cred::yubikey::get_or_create_challenge().unwrap_or_else(|e| {
                eprintln!("failed to load YubiKey challenge: {e}");
                std::process::exit(1);
            });
            let response =
                engram_cred::yubikey::challenge_response(&challenge).unwrap_or_else(|e| {
                    eprintln!("YubiKey challenge-response failed: {e}");
                    std::process::exit(1);
                });
            Some(engram_cred::crypto::derive_key(0, b"", Some(&response)))
        }
        _ => {
            let mode_name = format!("{:?}", config.encryption.mode).to_ascii_lowercase();
            tracing::info!("encryption mode: {}", mode_name);
            engram_lib::encryption::resolve_key(&config).unwrap_or_else(|e| {
                eprintln!("encryption key resolution failed: {e}");
                std::process::exit(1);
            })
        }
    };

    let db = Database::connect_with_config(&config, encryption_key)
        .await
        .expect("failed to connect to database");

    // Deferred embedder/reranker initialization -- server starts immediately, models load in background
    let embedder: Arc<tokio::sync::RwLock<Option<Arc<dyn EmbeddingProvider>>>> =
        Arc::new(tokio::sync::RwLock::new(None));
    let reranker: Arc<tokio::sync::RwLock<Option<Arc<Reranker>>>> =
        Arc::new(tokio::sync::RwLock::new(None));

    // Spawn background task to load embedding model
    {
        let embedder = Arc::clone(&embedder);
        let config = config.clone();
        tokio::spawn(async move {
            tracing::info!("loading ONNX embedding model in background...");
            match OnnxProvider::new(&config).await {
                Ok(provider) => {
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

    // Spawn background task to load reranker model
    if config.reranker_enabled {
        let reranker = Arc::clone(&reranker);
        let config = config.clone();
        tokio::spawn(async move {
            tracing::info!("loading cross-encoder reranker in background...");
            match Reranker::new(&config).await {
                Ok(r) => {
                    let mut guard = reranker.write().await;
                    *guard = Some(Arc::new(r));
                    tracing::info!("cross-encoder reranker ready");
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

    let db_arc = Arc::new(db);

    // Initialize brain backend (Hopfield in-process or subprocess eidolon)
    let data_dir = config.data_dir.clone();
    let brain = create_brain_backend(Arc::clone(&db_arc), &data_dir, 1).await;

    // Approval notification channel for TUI clients
    let (approval_tx, _) = tokio::sync::watch::channel(());

    // Record this startup as a potential crash/restart event, then decide
    // whether the server has been crash-looping.
    if let Err(e) = engram_lib::admin::record_crash(&db_arc).await {
        tracing::warn!("failed to record startup crash timestamp: {}", e);
    }
    let safe_mode_active = engram_lib::admin::should_enter_safe_mode(&db_arc)
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

    // Start infrastructure background tasks.
    let (checkpoint_token, checkpoint_handle) =
        start_auto_checkpoint_task(Arc::clone(&state.db));
    tracing::info!("auto-checkpoint background task started");

    let (job_cleanup_token, job_cleanup_handle) =
        start_job_cleanup_task(Arc::clone(&state.db));
    tracing::info!("job-cleanup background task started");

    let (vector_sync_token, vector_sync_handle) =
        start_vector_sync_replay_task(Arc::clone(&state.db));
    tracing::info!("vector-sync-replay background task started");

    // Monitor background tasks -- log if any exit unexpectedly (panic/abort).
    tokio::spawn(async move {
        // Keep tokens alive so tasks aren't cancelled.
        let _checkpoint_token = checkpoint_token;
        let _job_cleanup_token = job_cleanup_token;
        let _vector_sync_token = vector_sync_token;
        let _pagerank_token = pagerank_handles.as_ref().map(|(t, _)| t);

        let mut tasks: Vec<(&str, tokio::task::JoinHandle<()>)> = vec![
            ("auto-checkpoint", checkpoint_handle),
            ("job-cleanup", job_cleanup_handle),
            ("vector-sync-replay", vector_sync_handle),
        ];
        if let Some((_, handle)) = pagerank_handles {
            tasks.push(("pagerank-refresh", handle));
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

    if let Err(e) = engram_server::server::run(state).await {
        tracing::error!("server error: {}", e);
        std::process::exit(1);
    }
}
