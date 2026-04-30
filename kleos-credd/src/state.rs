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
    /// PIV slot 9A (AUTHENTICATION) public keys, loaded from
    /// `~/.config/cred/piv-9a-pubkeys/*.pem` (multi-key) or the legacy
    /// `~/.config/cred/piv-9a-pubkey.pem` (single-key) at startup. The
    /// ECDH bootstrap handler tries each key until one verifies.
    pub piv_9a_pubkeys: Arc<Vec<VerifyingKey>>,
    /// PIV slot 9D (KEY_MANAGEMENT) public key, loaded from
    /// `~/.config/cred/piv-9d-pubkey.pem` at startup. Informational only
    /// for the server (the YubiKey holds the corresponding private key
    /// and the ECDH op happens via `kleos_cred::piv::ecdh_agree`).
    pub piv_9d_pubkey: Option<Arc<PublicKey>>,
}

impl AppState {
    pub fn new(db: Database, master_key: [u8; KEY_SIZE]) -> Self {
        let (piv_9a_pubkeys, piv_9d_pubkey) = load_piv_pubkeys();
        Self {
            db: Arc::new(db),
            master_key: Arc::new(master_key),
            rate_limiter: Arc::new(RateLimiter::new()),
            bootstrap_master: None,
            file_agent_keys: Arc::new(Mutex::new(FileAgentKeyStore::default())),
            piv_9a_pubkeys,
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
        let (piv_9a_pubkeys, piv_9d_pubkey) = load_piv_pubkeys();
        Self {
            db: Arc::new(db),
            master_key: Arc::new(master_key),
            rate_limiter: Arc::new(RateLimiter::new()),
            bootstrap_master: bootstrap_master.map(Arc::new),
            file_agent_keys: Arc::new(Mutex::new(file_agent_keys)),
            piv_9a_pubkeys,
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

/// Load PIV 9A pubkeys from `~/.config/cred/piv-9a-pubkeys/*.pem` (multi-key
/// directory) with fallback to the legacy `~/.config/cred/piv-9a-pubkey.pem`
/// (single file). Also loads 9D from `piv-9d-pubkey.pem` as before.
fn load_piv_pubkeys() -> (Arc<Vec<VerifyingKey>>, Option<Arc<PublicKey>>) {
    let dir = cred_config_dir();
    let mut keys_9a = Vec::new();

    // Multi-key directory (preferred)
    let keys_dir = dir.join("piv-9a-pubkeys");
    if keys_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&keys_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "pem") {
                    match std::fs::read_to_string(&path) {
                        Ok(pem) => match VerifyingKey::from_public_key_pem(&pem) {
                            Ok(k) => {
                                tracing::info!(path = %path.display(), "loaded 9A pubkey");
                                keys_9a.push(k);
                            }
                            Err(e) => warn!(path = %path.display(), error = %e, "9A pubkey unparseable"),
                        },
                        Err(e) => warn!(path = %path.display(), error = %e, "9A pubkey read failed"),
                    }
                }
            }
        }
    }

    // Legacy single-file fallback
    if keys_9a.is_empty() {
        let pa = dir.join("piv-9a-pubkey.pem");
        if pa.exists() {
            match std::fs::read_to_string(&pa) {
                Ok(pem) => match VerifyingKey::from_public_key_pem(&pem) {
                    Ok(k) => {
                        tracing::info!(path = %pa.display(), "loaded legacy 9A pubkey");
                        keys_9a.push(k);
                    }
                    Err(e) => warn!(path = %pa.display(), error = %e, "piv-9a pubkey unparseable; ECDH disabled"),
                },
                Err(e) => warn!(path = %pa.display(), error = %e, "piv-9a pubkey read failed"),
            }
        }
    }

    if keys_9a.is_empty() {
        warn!("no PIV 9A pubkeys found; ECDH bootstrap will be unavailable");
    } else {
        tracing::info!(count = keys_9a.len(), "PIV 9A pubkeys loaded");
    }

    let key_9d = {
        let pd = dir.join("piv-9d-pubkey.pem");
        if pd.exists() {
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
        }
    };

    (Arc::new(keys_9a), key_9d)
}

impl Deref for AppState {
    type Target = Database;

    fn deref(&self) -> &Self::Target {
        &self.db
    }
}
