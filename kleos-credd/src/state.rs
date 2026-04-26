//! Application state for credd daemon.

use std::ops::Deref;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use kleos_cred::crypto::KEY_SIZE;
use kleos_lib::db::Database;
use kleos_lib::ratelimit::RateLimiter;
use p256::ecdsa::VerifyingKey;
use p256::pkcs8::DecodePublicKey;
use p256::PublicKey;
use tracing::warn;
use zeroize::Zeroizing;

use kleos_cred::agent_keys_file::FileAgentKeyStore;

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
    /// PIV slot 9A (AUTHENTICATION) public key, loaded from
    /// `~/.config/cred/piv-9a-pubkey.pem` at startup. Used by the ECDH
    /// bootstrap handler to verify client signatures. `None` if the file
    /// is absent (POST /bootstrap/kleos-bearer returns 503 then).
    pub piv_9a_pubkey: Option<Arc<VerifyingKey>>,
    /// PIV slot 9D (KEY_MANAGEMENT) public key, loaded from
    /// `~/.config/cred/piv-9d-pubkey.pem` at startup. Informational only
    /// for the server (the YubiKey holds the corresponding private key
    /// and the ECDH op happens via `kleos_cred::piv::ecdh_agree`).
    pub piv_9d_pubkey: Option<Arc<PublicKey>>,
}

impl AppState {
    pub fn new(db: Database, master_key: [u8; KEY_SIZE]) -> Self {
        let (piv_9a_pubkey, piv_9d_pubkey) = load_piv_pubkeys();
        Self {
            db: Arc::new(db),
            master_key: Arc::new(master_key),
            rate_limiter: Arc::new(RateLimiter::new()),
            bootstrap_master: None,
            file_agent_keys: Arc::new(Mutex::new(FileAgentKeyStore::default())),
            piv_9a_pubkey,
            piv_9d_pubkey,
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
        let (piv_9a_pubkey, piv_9d_pubkey) = load_piv_pubkeys();
        Self {
            db: Arc::new(db),
            master_key: Arc::new(master_key),
            rate_limiter: Arc::new(RateLimiter::new()),
            bootstrap_master: bootstrap_master.map(Arc::new),
            file_agent_keys: Arc::new(Mutex::new(file_agent_keys)),
            piv_9a_pubkey,
            piv_9d_pubkey,
        }
    }
}

/// Standard cred config dir resolution (matches kleos_cred::piv::config_dir
/// and kleos_cred::yubikey::config_dir).
fn cred_config_dir() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".config"))
                .unwrap_or_else(|_| PathBuf::from("."))
        });
    base.join("cred")
}

/// Load the PIV 9A and 9D public keys from `~/.config/cred/piv-*.pem`.
/// Both are optional. If a file exists but fails to parse, log a warning
/// and treat that slot as absent so the ECDH endpoint returns 503 cleanly
/// rather than a 500.
fn load_piv_pubkeys() -> (Option<Arc<VerifyingKey>>, Option<Arc<PublicKey>>) {
    let dir = cred_config_dir();
    let pa = dir.join("piv-9a-pubkey.pem");
    let pd = dir.join("piv-9d-pubkey.pem");

    let key_9a = if pa.exists() {
        match std::fs::read_to_string(&pa) {
            Ok(pem) => match VerifyingKey::from_public_key_pem(&pem) {
                Ok(k) => Some(Arc::new(k)),
                Err(e) => {
                    warn!(path = %pa.display(), error = %e, "piv-9a pubkey unparseable; ECDH disabled");
                    None
                }
            },
            Err(e) => {
                warn!(path = %pa.display(), error = %e, "piv-9a pubkey read failed");
                None
            }
        }
    } else {
        None
    };

    let key_9d = if pd.exists() {
        match std::fs::read_to_string(&pd) {
            Ok(pem) => match PublicKey::from_public_key_pem(&pem) {
                Ok(k) => Some(Arc::new(k)),
                Err(e) => {
                    warn!(path = %pd.display(), error = %e, "piv-9d pubkey unparseable");
                    None
                }
            },
            Err(e) => {
                warn!(path = %pd.display(), error = %e, "piv-9d pubkey read failed");
                None
            }
        }
    } else {
        None
    };

    (key_9a, key_9d)
}

impl Deref for AppState {
    type Target = Database;

    fn deref(&self) -> &Self::Target {
        &self.db
    }
}
