//! HTTP server setup for credd.

use tracing::{info, warn};

use kleos_cred::crypto::derive_key;
use kleos_lib::db::migrations::run_migrations;
use kleos_lib::db::Database;

use crate::build_router;
use crate::state::AppState;

/// Run the credd HTTP server.
#[tracing::instrument(skip(master_password, encryption_key), fields(listen = %listen, db_path = %db_path))]
pub async fn run(
    listen: &str,
    db_path: &str,
    master_password: &str,
    encryption_key: Option<[u8; 32]>,
) -> anyhow::Result<()> {
    // Connect to database (with optional at-rest encryption)
    let db = Database::connect_encrypted(db_path, encryption_key).await?;

    // Run migrations
    db.write(|conn| run_migrations(conn)).await?;

    // Derive master key from password (user_id 1 = admin)
    let master_key = derive_key(1, master_password.as_bytes(), None);

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
