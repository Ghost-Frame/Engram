//! Application state for credd daemon.

use std::ops::Deref;
use std::sync::Arc;

use kleos_cred::crypto::KEY_SIZE;
use kleos_lib::db::Database;
use kleos_lib::ratelimit::RateLimiter;
use zeroize::Zeroizing;

/// Application state shared across handlers.
#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Database>,
    pub master_key: Arc<[u8; KEY_SIZE]>,
    pub rate_limiter: Arc<RateLimiter>,
    /// Decrypted bare Kleos bearer loaded from `bootstrap.enc` at startup.
    /// `None` if the blob is absent (the `/bootstrap/kleos-bearer` endpoint
    /// returns 404 in that case). Wrapped in `Zeroizing` so the bearer is
    /// scrubbed from memory when the AppState is dropped.
    pub bootstrap_master: Option<Arc<Zeroizing<String>>>,
}

impl AppState {
    pub fn new(db: Database, master_key: [u8; KEY_SIZE]) -> Self {
        Self {
            db: Arc::new(db),
            master_key: Arc::new(master_key),
            rate_limiter: Arc::new(RateLimiter::new()),
            bootstrap_master: None,
        }
    }

    /// Constructor variant that includes the bootstrap bearer (loaded by
    /// main.rs after deriving the master key).
    pub fn with_bootstrap(
        db: Database,
        master_key: [u8; KEY_SIZE],
        bootstrap_master: Option<Zeroizing<String>>,
    ) -> Self {
        Self {
            db: Arc::new(db),
            master_key: Arc::new(master_key),
            rate_limiter: Arc::new(RateLimiter::new()),
            bootstrap_master: bootstrap_master.map(Arc::new),
        }
    }
}

impl Deref for AppState {
    type Target = Database;

    fn deref(&self) -> &Self::Target {
        &self.db
    }
}
