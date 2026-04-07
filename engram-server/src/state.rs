use std::sync::Arc;
use kleos_lib::config::Config;
use kleos_lib::db::Database;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Database>,
    pub config: Arc<Config>,
}
