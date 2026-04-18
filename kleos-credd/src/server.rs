//! HTTP server setup for credd.

use axum::{
    extract::DefaultBodyLimit,
    middleware,
    routing::{delete, get, post},
    Router,
};
use std::time::Duration;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use tracing::{info, warn};

use kleos_cred::crypto::derive_key;
use kleos_lib::db::migrations::run_migrations;
use kleos_lib::db::Database;

use crate::auth::{auth_middleware, preauth_rate_limit};
use crate::handlers::{agents, resolve, secrets};
use crate::state::AppState;

/// Request body limit for credd: 1 MiB. Prevents memory exhaustion from
/// oversized secret payloads or proxy bodies.
const CREDD_BODY_LIMIT: usize = 1024 * 1024;

/// Per-request timeout: 30 seconds. Prevents slow-loris and hung-upstream
/// from tying up handler tasks indefinitely.
const CREDD_REQUEST_TIMEOUT_SECS: u64 = 30;

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

    // Build router
    let app = Router::new()
        // Secret management
        .route("/secrets", get(secrets::list_handler))
        .route(
            "/secret/{category}/{*name}",
            get(secrets::get_handler)
                .post(secrets::store_handler)
                .put(secrets::update_handler)
                .delete(secrets::delete_handler),
        )
        // Three-tier resolve
        .route("/resolve/text", post(resolve::resolve_text_handler))
        .route("/resolve/proxy", post(resolve::proxy_handler))
        .route("/resolve/raw", post(resolve::raw_handler))
        // Agent key management
        .route("/agents", get(agents::list_handler))
        .route("/agents", post(agents::create_handler))
        .route("/agents/{name}", delete(agents::delete_handler))
        .route("/agents/{name}/revoke", post(agents::revoke_handler))
        // Health check (no auth)
        .route("/health", get(health_handler))
        // Apply middleware (outermost layer executes first)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            preauth_rate_limit,
        ))
        // SECURITY: request hardening layers. DefaultBodyLimit prevents
        // memory exhaustion, TimeoutLayer prevents slow-loris / hung proxy.
        .layer(DefaultBodyLimit::max(CREDD_BODY_LIMIT))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(CREDD_REQUEST_TIMEOUT_SECS),
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

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

async fn health_handler() -> &'static str {
    "ok"
}
