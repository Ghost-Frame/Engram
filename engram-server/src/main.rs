mod error;
mod extractors;
mod middleware;
mod routes;
mod server;
mod state;

use engram_lib::config::Config;
use engram_lib::db::Database;
use engram_lib::embeddings::onnx::OnnxProvider;
use engram_lib::embeddings::EmbeddingProvider;
use engram_lib::llm::local::{LocalModelClient, OllamaConfig};
use engram_lib::reranker::Reranker;
use state::AppState;
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
    let db = Database::connect(&config.db_path)
        .await
        .expect("failed to connect to database");

    // Initialize embedding provider (graceful degradation if unavailable)
    let embedder: Option<Arc<dyn EmbeddingProvider>> =
        match OnnxProvider::new(&config).await {
            Ok(provider) => {
                tracing::info!("ONNX embedding provider ready");
                Some(Arc::new(provider))
            }
            Err(e) => {
                tracing::warn!("ONNX embedding provider failed to initialize: {}. Vector search disabled.", e);
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
                tracing::warn!("reranker failed to initialize: {}. Results will not be reranked.", e);
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

    let state = AppState {
        db: Arc::new(db),
        config: Arc::new(config),
        embedder,
        reranker,
        brain: None,
        llm,
        sessions: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        eidolon_config: None,
    };

    if let Err(e) = server::run(state).await {
        tracing::error!("server error: {}", e);
        std::process::exit(1);
    }
}
