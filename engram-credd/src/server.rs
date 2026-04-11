//! HTTP server setup for credd.

use axum::{
    middleware,
    routing::{delete, get, post},
    Router,
};
use tower_http::trace::TraceLayer;
use tracing::info;

use engram_cred::crypto::derive_key;
use engram_lib::db::migrations::run_migrations;
use engram_lib::db::Database;

use crate::auth::auth_middleware;
use crate::handlers::{agents, resolve, secrets};
use crate::state::AppState;

/// Run the credd HTTP server.
pub async fn run(listen: &str, db_path: &str, master_password: &str) -> anyhow::Result<()> {
    // Connect to database
    let db = Database::connect(db_path).await?;

    // Run migrations
    run_migrations(&db.conn).await?;

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
        .route(
            "/agents/{name}",
            delete(agents::delete_handler),
        )
        .route(
            "/agents/{name}/revoke",
            post(agents::revoke_handler),
        )
        // Health check (no auth)
        .route("/health", get(health_handler))
        // Apply middleware
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    // Parse listen address
    let addr: std::net::SocketAddr = listen.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!("credd listening on {}", addr);
    axum::serve(listener, app).await?;

    Ok(())
}

async fn health_handler() -> &'static str {
    "ok"
}
