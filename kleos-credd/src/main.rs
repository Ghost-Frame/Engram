//! Engram credential management daemon.
//!
//! HTTP server providing secure credential storage and retrieval
//! with two-tier authentication (master key vs agent keys).

use clap::Parser;
use kleos_cred::crypto::{derive_key, KEY_SIZE};
use kleos_cred::agent_keys_file::FileAgentKeyStore;
use kleos_credd::{bootstrap, server};
use kleos_lib::config::{Config, EncryptionMode};
use tracing::info;

#[derive(Parser)]
#[command(name = "engram-credd")]
#[command(about = "Engram credential management daemon")]
struct Args {
    /// Listen address
    #[arg(long, default_value = "127.0.0.1:4400", env = "CREDD_LISTEN")]
    listen: String,

    /// Database path
    #[arg(long, default_value = "kleos.db", env = "CREDD_DB_PATH")]
    db_path: String,

    /// Master password (only consulted when --auth-mode=password).
    #[arg(long, env = "CREDD_MASTER_PASSWORD")]
    master_password: Option<String>,

    /// How credd derives its master key. `yubikey` (default) does an HMAC-SHA1
    /// challenge against slot 2 and Argon2id-derives a 32-byte key, requiring
    /// no on-disk secrets. `password` reads --master-password / stdin and
    /// derives via the same Argon2id KDF -- documented escape hatch for
    /// installations without a YubiKey.
    #[arg(long, default_value = "yubikey", env = "CREDD_AUTH_MODE")]
    auth_mode: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    kleos_lib::config::migrate_env_prefix();

    let _otel_guard =
        kleos_lib::observability::init_tracing("engram-credd", "kleos_credd=info,tower_http=debug");

    let args = Args::parse();

    // Derive credd's own master key. YubiKey is the default and recommended
    // path: zero on-disk secrets, hardware-bound, single tap per restart.
    // The password path stays available for installations without a YubiKey;
    // operators opt into it via CREDD_AUTH_MODE=password.
    let master_key: [u8; KEY_SIZE] = match args.auth_mode.as_str() {
        "yubikey" => {
            info!("credd: deriving master key from YubiKey (slot 2 challenge-response)");
            kleos_cred::yubikey::derive_master_key()
                .map_err(|e| anyhow::anyhow!("YubiKey unlock failed: {e}"))?
        }
        "password" => {
            let password = match args.master_password {
                Some(pw) => pw,
                None => {
                    eprintln!("Enter master password: ");
                    rpassword::read_password()?
                }
            };
            info!("credd: deriving master key from password (CREDD_AUTH_MODE=password)");
            derive_key(1, password.as_bytes(), None)
        }
        other => {
            anyhow::bail!(
                "unknown CREDD_AUTH_MODE `{other}`; expected `yubikey` or `password`"
            );
        }
    };

    // Resolve at-rest DB encryption key (separate from credd's owner-key path).
    // ENGRAM_ENCRYPTION_MODE controls how the SQLite file itself is encrypted;
    // CREDD_AUTH_MODE controls how credd authenticates clients. Both can use
    // YubiKey independently.
    let enc_config = Config::from_env();
    let encryption_key = match enc_config.encryption.mode {
        EncryptionMode::None => None,
        EncryptionMode::Yubikey => {
            info!("at-rest encryption mode: yubikey -- touch slot 2 to unlock database");
            let challenge = kleos_cred::yubikey::get_or_create_challenge()
                .map_err(|e| anyhow::anyhow!("YubiKey challenge: {e}"))?;
            let response = kleos_cred::yubikey::challenge_response(&challenge)
                .map_err(|e| anyhow::anyhow!("YubiKey response: {e}"))?;
            Some(kleos_cred::crypto::derive_key(0, b"", Some(&response)))
        }
        _ => {
            let mode_name = format!("{:?}", enc_config.encryption.mode).to_ascii_lowercase();
            info!("at-rest encryption mode: {}", mode_name);
            kleos_lib::encryption::resolve_key(&enc_config)
                .map_err(|e| anyhow::anyhow!("encryption key: {e}"))?
        }
    };

    // Decrypt bootstrap.enc (if present) using the just-derived master key.
    // Absent blob is non-fatal -- credd serves everything except the
    // /bootstrap/kleos-bearer endpoint, which 404s in that case.
    let bootstrap_master = bootstrap::load_bootstrap_blob(&master_key).await?;
    if bootstrap_master.is_some() {
        info!(
            "credd: bootstrap.enc loaded from {}",
            bootstrap::blob_path().display()
        );
    }

    // Load file-backed bootstrap-agent keys (~/.config/cred/agent-keys.json).
    // Empty store on missing file; credd starts cleanly. Operator generates
    // tokens via `cred agent-key generate <id> --scope bootstrap/<slot>`.
    let file_agent_keys = FileAgentKeyStore::load()?;

    info!("starting credd on {}", args.listen);
    server::run(
        &args.listen,
        &args.db_path,
        master_key,
        bootstrap_master,
        file_agent_keys,
        encryption_key,
    )
    .await?;

    Ok(())
}
