use engram_lib::config::Config;
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
    let db = Database::connect_with_config(&config)
        .await
        .expect("failed to connect to database");

    // Initialize embedding provider (graceful degradation if unavailable)
    let embedder: Option<Arc<dyn EmbeddingProvider>> = match OnnxProvider::new(&config).await {
        Ok(provider) => {
            tracing::info!("ONNX embedding provider ready");
            Some(Arc::new(provider))
        }
        Err(e) => {
            tracing::warn!(
                "ONNX embedding provider failed to initialize: {}. Vector search disabled.",
                e
            );
            None
        }
    };

    let reranker: Option<Arc<Reranker>> = if config.reranker_enabled {
        match Reranker::new(&config).await {
            Ok(r) => {
                tracing::info!("cross-encoder reranker ready");
                Some(Arc::new(r))
            }
            Err(e) => {
                tracing::warn!(
                    "reranker failed to initialize: {}. Results will not be reranked.",
                    e
                );
                None
            }
        }
    } else {
        None
    };

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
        safe_mode: Arc::new(AtomicBool::new(safe_mode_active)),
    };

    // Start background PageRank refresh job if enabled.
    let _pagerank_token = if state.config.pagerank_enabled {
        let token = start_pagerank_refresh_job(Arc::clone(&state.db), Arc::clone(&state.config));
        tracing::info!("background pagerank refresh job started");
        Some(token)
    } else {
        tracing::info!("pagerank disabled -- skipping refresh job");
        None
    };

    // Start infrastructure background tasks.
    let _checkpoint_token = start_auto_checkpoint_task(Arc::clone(&state.db));
    tracing::info!("auto-checkpoint background task started");

    let _job_cleanup_token = start_job_cleanup_task(Arc::clone(&state.db));
    tracing::info!("job-cleanup background task started");

    let _vector_sync_token = start_vector_sync_replay_task(Arc::clone(&state.db));
    tracing::info!("vector-sync-replay background task started");

    if let Err(e) = engram_server::server::run(state).await {
        tracing::error!("server error: {}", e);
        std::process::exit(1);
    }
}
