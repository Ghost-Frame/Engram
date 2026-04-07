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

    let state = AppState {
        db: Arc::new(db),
        config: Arc::new(config),
        embedder,
    };

    if let Err(e) = server::run(state).await {
        tracing::error!("server error: {}", e);
        std::process::exit(1);
    }
}
