//! engram-credd library exports for testing.

pub mod auth;
pub mod handlers;
pub mod server;
pub mod state;

use axum::{middleware, routing::{delete, get, post}, Router};
use tower_http::trace::TraceLayer;

use auth::auth_middleware;
use handlers::{agents, resolve, secrets};
use state::AppState;

/// Build the credd router (for testing).
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
        // Health check (no auth)
        .route("/health", get(health_handler))
        // Apply middleware
        .layer(middleware::from_fn_with_state(state.clone(), auth_middleware))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn health_handler() -> &'static str {
    "ok"
}
