use std::sync::Arc;
use engram_lib::config::Config;
use engram_lib::db::Database;
use engram_lib::embeddings::EmbeddingProvider;
use engram_lib::reranker::Reranker;
use engram_lib::services::brain::BrainManager;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Database>,
    pub config: Arc<Config>,
    pub embedder: Option<Arc<dyn EmbeddingProvider>>,
    pub reranker: Option<Arc<Reranker>>,
    pub brain: Option<Arc<BrainManager>>,
}
