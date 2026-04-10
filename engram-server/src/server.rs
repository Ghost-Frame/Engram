use axum::{middleware as axum_mw, Router};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::middleware::auth::auth_middleware;
use crate::routes;
use crate::state::AppState;

/// Build the Axum router with all routes and middleware applied.
/// Exposed as a public function so integration tests can build an in-process app.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .merge(routes::health::router())
        .merge(routes::docs::router())
        .merge(routes::memory::router())
        .merge(routes::admin::router())
        .merge(routes::tasks::router())
        .merge(routes::axon::router())
        .merge(routes::broca::router())
        .merge(routes::soma::router())
        .merge(routes::thymus::router())
        .merge(routes::loom::router())
        .merge(routes::episodes::router())
        .merge(routes::conversations::router())
        .merge(routes::graph::router())
        .merge(routes::intelligence::router())
        .merge(routes::skills::router())
        .merge(routes::personality::router())
        .merge(routes::platform::router())
        .merge(routes::security::router())
        .merge(routes::webhooks::router())
        .merge(routes::brain::router())
        .merge(routes::context::router())
        .merge(routes::inbox::router())
        .merge(routes::ingestion::router())
        .merge(routes::pack::router())
        .merge(routes::projects::router())
        .merge(routes::prompts::router())
        .merge(routes::scratchpad::router())
        .merge(routes::activity::router())
        .merge(routes::gate::router())
        .merge(routes::growth::router())
        .merge(routes::sessions::router())
        .merge(routes::agents::router())
        .merge(routes::artifacts::router())
        .merge(routes::auth_keys::router())
        .merge(routes::fsrs::router())
        .merge(routes::grounding::router())
        .merge(routes::search::router())
        .merge(routes::onboard::router())
        .merge(routes::portability::router())
        .layer(axum_mw::from_fn_with_state(state.clone(), auth_middleware))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

pub async fn run(state: AppState) -> Result<(), Box<dyn std::error::Error>> {
    let addr = format!("{}:{}", state.config.host, state.config.port);
    let app = build_router(state);
    tracing::info!("engram-server listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
