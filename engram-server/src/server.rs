use axum::{middleware as axum_mw, Router};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::middleware::auth::auth_middleware;
use crate::routes;
use crate::state::AppState;

pub async fn run(state: AppState) -> Result<(), Box<dyn std::error::Error>> {
    let addr = format!("{}:{}", state.config.host, state.config.port);

    let app = Router::new()
        .merge(routes::health::router())
        .merge(routes::memory::router())
        .merge(routes::admin::router())
        .merge(routes::tasks::router())
        .merge(routes::axon::router())
        .merge(routes::broca::router())
        .merge(routes::soma::router())
        .merge(routes::thymus::router())
        .merge(routes::loom::router())
        .layer(axum_mw::from_fn_with_state(state.clone(), auth_middleware))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state.clone());

    tracing::info!("engram-server listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
