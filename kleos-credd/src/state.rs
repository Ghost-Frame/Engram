//! Application state for credd daemon.

use std::ops::Deref;
use std::sync::{Arc, Mutex};

use kleos_cred::crypto::KEY_SIZE;
use kleos_lib::db::Database;
use kleos_lib::ratelimit::RateLimiter;
use zeroize::Zeroizing;

use crate::agent_keys_file::FileAgentKeyStore;

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
    /// File-backed scoped agent-key store for `/bootstrap/kleos-bearer`.
    /// Separate from the DB-backed `cred_agent_keys` table used by the
    /// three-tier resolve handlers; lives at `~/.config/cred/agent-keys.json`
    /// so a fresh shell can read it before the cred DB is unlocked.
    pub file_agent_keys: Arc<Mutex<FileAgentKeyStore>>,
}

impl AppState {
    pub fn new(db: Database, master_key: [u8; KEY_SIZE]) -> Self {
        Self {
            db: Arc::new(db),
            master_key: Arc::new(master_key),
            rate_limiter: Arc::new(RateLimiter::new()),
            bootstrap_master: None,
            file_agent_keys: Arc::new(Mutex::new(FileAgentKeyStore::default())),
        }
    }

    /// Constructor variant that includes the bootstrap bearer (loaded by
    /// main.rs after deriving the master key) and the file-backed agent
    /// key store.
    pub fn with_bootstrap(
        db: Database,
        master_key: [u8; KEY_SIZE],
        bootstrap_master: Option<Zeroizing<String>>,
        file_agent_keys: FileAgentKeyStore,
    ) -> Self {
        Self {
            db: Arc::new(db),
            master_key: Arc::new(master_key),
            rate_limiter: Arc::new(RateLimiter::new()),
            bootstrap_master: bootstrap_master.map(Arc::new),
            file_agent_keys: Arc::new(Mutex::new(file_agent_keys)),
        }
    }
}

impl Deref for AppState {
    type Target = Database;

    fn deref(&self) -> &Self::Target {
        &self.db
    }
}
