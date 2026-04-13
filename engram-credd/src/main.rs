//! Engram credential management daemon.
//!
//! HTTP server providing secure credential storage and retrieval
//! with two-tier authentication (master key vs agent keys).

use clap::Parser;
use engram_credd::server;
use engram_lib::config::{Config, EncryptionMode};
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser)]
#[command(name = "engram-credd")]
#[command(about = "Engram credential management daemon")]
struct Args {
    /// Listen address
    #[arg(long, default_value = "127.0.0.1:4400", env = "CREDD_LISTEN")]
    listen: String,

    /// Database path
    #[arg(long, default_value = "engram.db", env = "CREDD_DB_PATH")]
    db_path: String,

    /// Master password (from stdin if not provided)
    #[arg(long, env = "CREDD_MASTER_PASSWORD")]
    master_password: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "engram_credd=info,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let args = Args::parse();

    // Get master password
    let master_password = match args.master_password {
        Some(pw) => pw,
        None => {
            eprintln!("Enter master password: ");
            rpassword::read_password()?
        }
    };

    // Resolve at-rest encryption key (reads ENGRAM_ENCRYPTION_MODE env).
    let enc_config = Config::from_env();
    let encryption_key = match enc_config.encryption.mode {
        EncryptionMode::None => None,
        EncryptionMode::Yubikey => {
            info!("encryption mode: yubikey -- touch slot 2 to unlock database...");
            let challenge = engram_cred::yubikey::get_or_create_challenge()
                .map_err(|e| anyhow::anyhow!("YubiKey challenge: {e}"))?;
            let response = engram_cred::yubikey::challenge_response(&challenge)
                .map_err(|e| anyhow::anyhow!("YubiKey response: {e}"))?;
            Some(engram_cred::crypto::derive_key(0, b"", Some(&response)))
        }
        _ => {
            let mode_name = format!("{:?}", enc_config.encryption.mode).to_ascii_lowercase();
            info!("encryption mode: {}", mode_name);
            engram_lib::encryption::resolve_key(&enc_config)
                .map_err(|e| anyhow::anyhow!("encryption key: {e}"))?
        }
    };

    info!("Starting credd on {}", args.listen);
    server::run(
        &args.listen,
        &args.db_path,
        &master_password,
        encryption_key,
    )
    .await?;

    Ok(())
}
