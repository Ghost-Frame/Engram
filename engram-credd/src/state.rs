//! Application state for credd daemon.

use std::ops::Deref;
use std::sync::Arc;

use engram_cred::crypto::KEY_SIZE;
use engram_lib::db::Database;

/// Application state shared across handlers.
#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Database>,
    pub master_key: Arc<[u8; KEY_SIZE]>,
}

impl AppState {
    pub fn new(db: Database, master_key: [u8; KEY_SIZE]) -> Self {
        Self {
            db: Arc::new(db),
            master_key: Arc::new(master_key),
        }
    }
}

impl Deref for AppState {
    type Target = Database;

    fn deref(&self) -> &Self::Target {
        &self.db
    }
}
