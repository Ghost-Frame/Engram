use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::routes;

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let app = Router::new()
        .nest("/health", routes::health::router())
        .nest("/api/memory", routes::memory::router())
        .nest("/api/services", routes::services::router())
        .nest("/api/guard", routes::guard::router())
        .nest("/api/admin", routes::admin::router())
        .nest("/api/graph", routes::graph::router())
        .nest("/api/episodes", routes::episodes::router())
        .nest("/api/webhooks", routes::webhooks::router())
        .nest("/gui", routes::gui::router())
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    let addr = "127.0.0.1:7700";
    tracing::info!("engram-server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
