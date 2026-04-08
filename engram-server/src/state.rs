use std::sync::Arc;
use kleos_lib::config::Config;
use kleos_lib::db::Database;
use kleos_lib::embeddings::EmbeddingProvider;
use kleos_lib::reranker::Reranker;
use kleos_lib::services::brain::BrainManager;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Database>,
    pub config: Arc<Config>,
    pub embedder: Option<Arc<dyn EmbeddingProvider>>,
    pub reranker: Option<Arc<Reranker>>,
    pub brain: Option<Arc<BrainManager>>,
}
