//! engram-credd library exports for testing.

pub mod auth;
pub mod bootstrap;
pub mod handlers;
pub mod listener;
pub mod server;
pub mod state;

use axum::{
    extract::DefaultBodyLimit,
    middleware,
    routing::{delete, get, post},
    Router,
};
use std::time::Duration;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use auth::{auth_middleware, preauth_rate_limit};
use handlers::{agents, bootstrap_bearer, resolve, secrets};
use state::AppState;

/// Request body limit: 1 MiB. Prevents memory exhaustion from oversized
/// secret payloads or proxy bodies.
pub const CREDD_BODY_LIMIT: usize = 1024 * 1024;

/// Per-request timeout: 30 seconds. Prevents slow-loris / hung upstream
/// from tying up handler tasks indefinitely.
pub const CREDD_REQUEST_TIMEOUT_SECS: u64 = 30;

/// Build the credd router used by both the binary server and integration
/// tests. Tests previously used a slimmed-down version that bypassed the
/// preauth rate limiter; sharing this builder keeps brute-force regressions
/// covered.
pub fn build_router(state: AppState) -> Router {
    Router::new()
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
        // Bootstrap bearer (broker per-agent Kleos bearers without on-disk plaintext)
        .route(
            "/bootstrap/kleos-bearer",
            get(bootstrap_bearer::get_bootstrap_kleos_bearer),
        )
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
        // SECURITY: request hardening -- same limits as the binary server.
        .layer(DefaultBodyLimit::max(CREDD_BODY_LIMIT))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(CREDD_REQUEST_TIMEOUT_SECS),
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn health_handler() -> &'static str {
    "ok"
}
