// SPDX-License-Identifier: Elastic-2.0

//! Headless SSH agent daemon.
//!
//! Presents a Unix `SSH_AUTH_SOCK` socket and brokers all listing and signing
//! operations to a running phylaxd instance over loopback HTTP.  Private keys
//! never leave phylaxd.

mod http_provider;

use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use kleos_phylax_ssh_agent::server::AgentServer;
use kleos_phylax_ssh_agent::KeyProvider as _;
use tokio_util::sync::CancellationToken;

use crate::http_provider::HttpKeyProvider;

/// Command-line arguments and environment-variable fallbacks.
#[derive(Debug, Parser)]
#[command(name = "phylax-ssh-agentd", about = "Headless SSH agent backed by phylaxd")]
struct Args {
    /// Path to the Unix socket to listen on.
    #[arg(
        long,
        env = "PHYLAX_SSH_AUTH_SOCK",
        default_value = "/run/phylax/ssh-agent.sock"
    )]
    socket: String,

    /// Base URL of the phylaxd HTTP API.
    #[arg(
        long,
        env = "PHYLAXD_URL",
        default_value = "http://127.0.0.1:3100"
    )]
    phylaxd_url: String,

    /// Path to a file containing the bearer token (token is read and trimmed).
    /// If omitted, falls back to the PHYLAX_BEARER environment variable.
    #[arg(long, env = "PHYLAX_BEARER_FILE")]
    bearer_file: Option<String>,
}

/// Read the bearer token: file (trimmed) takes priority, then env var.
async fn read_bearer(bearer_file: Option<&str>) -> anyhow::Result<String> {
    if let Some(path) = bearer_file {
        let contents = tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("failed to read bearer file: {path}"))?;
        return Ok(contents.trim().to_string());
    }

    // Fall back to environment variable.
    std::env::var("PHYLAX_BEARER")
        .context("no bearer token: set PHYLAX_BEARER_FILE or PHYLAX_BEARER")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialise logging from RUST_LOG (defaults to info).
    env_logger::init();

    let args = Args::parse();

    // Ensure the socket parent directory exists.
    if let Some(parent) = std::path::Path::new(&args.socket).parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("failed to create socket dir: {}", parent.display()))?;
        }
    }

    // Resolve the bearer token.
    let bearer = read_bearer(args.bearer_file.as_deref()).await?;

    log::info!(
        "Connecting to phylaxd at {} ...",
        args.phylaxd_url
    );

    // Build the HTTP provider -- eagerly fetches the identity list.
    let provider = Arc::new(
        HttpKeyProvider::connect(&args.phylaxd_url, bearer)
            .await
            .context("failed to connect to phylaxd")?,
    );

    log::info!(
        "Loaded {} SSH identities from phylaxd",
        provider.identities().len()
    );

    // Set up a cancellation token that fires on SIGTERM or SIGINT.
    let cancel = CancellationToken::new();
    let cancel_signals = cancel.clone();

    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};

            let mut sigterm = signal(SignalKind::terminate())
                .expect("failed to register SIGTERM handler");
            let mut sigint = signal(SignalKind::interrupt())
                .expect("failed to register SIGINT handler");

            tokio::select! {
                _ = sigterm.recv() => log::info!("Received SIGTERM"),
                _ = sigint.recv()  => log::info!("Received SIGINT"),
            }
        }

        #[cfg(not(unix))]
        {
            tokio::signal::ctrl_c()
                .await
                .expect("failed to listen for Ctrl-C");
            log::info!("Received Ctrl-C");
        }

        cancel_signals.cancel();
    });

    // Build and run the agent server.
    let server = AgentServer::new(args.socket.clone(), provider);

    log::info!("SSH agent socket: {}", args.socket);

    server
        .run(cancel)
        .await
        .context("SSH agent server error")?;

    log::info!("phylax-ssh-agentd shut down cleanly");
    Ok(())
}
