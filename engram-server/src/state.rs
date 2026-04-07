use std::sync::Arc;
use engram_lib::config::Config;
use engram_lib::db::Database;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Database>,
    pub config: Arc<Config>,
}
