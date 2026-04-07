use std::sync::Arc;
use kleos_lib::config::Config;
use kleos_lib::db::Database;
use kleos_lib::embeddings::EmbeddingProvider;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Database>,
    pub config: Arc<Config>,
    pub embedder: Option<Arc<dyn EmbeddingProvider>>,
}
