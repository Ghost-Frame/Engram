//! HTTP server setup for credd.

use tracing::{info, warn};

use kleos_cred::crypto::KEY_SIZE;
use kleos_lib::db::migrations::run_migrations;
use kleos_lib::db::Database;

use crate::build_router;
use crate::state::AppState;

/// Run the credd HTTP server.
///
/// `master_key` is the 32-byte AES-256-GCM key credd uses to encrypt and
/// authenticate cred entries. The caller (main.rs) derives it from a YubiKey
/// challenge-response (default) or a password (opt-in), so this function does
/// not care how it was produced.
#[tracing::instrument(skip(master_key, encryption_key), fields(listen = %listen, db_path = %db_path))]
pub async fn run(
    listen: &str,
    db_path: &str,
    master_key: [u8; KEY_SIZE],
    encryption_key: Option<[u8; 32]>,
) -> anyhow::Result<()> {
    // Connect to database (with optional at-rest encryption)
    let db = Database::connect_encrypted(db_path, encryption_key).await?;

    // Run migrations
    db.write(|conn| run_migrations(conn)).await?;

    let state = AppState::new(db, master_key);

    // Build the router via the shared builder so integration tests and the
    // binary server exercise the same middleware stack (including the
    // preauth brute-force throttle).
    let app = build_router(state);

    // Parse listen address
    let addr: std::net::SocketAddr = listen.parse()?;

    // SECURITY: warn if binding to a non-loopback address. credd is
    // designed for local-only access; exposing it to the network widens
    // the attack surface for brute-force token guessing.
    if !addr.ip().is_loopback() {
        warn!(
            "credd is binding to non-loopback address {}. \
             Ensure network access is restricted (firewall, VPN, etc.).",
            addr
        );
    }

    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!("credd listening on {}", addr);
    // Install ConnectInfo so future rate-limiting middleware can read peer IP.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;

    Ok(())
}
