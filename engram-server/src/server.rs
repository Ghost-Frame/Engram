use axum::{extract::DefaultBodyLimit, http::HeaderValue, middleware as axum_mw, Router};
use std::time::Duration;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

/// Default request body limit: 2 MiB. Prevents memory exhaustion from oversized payloads.
const BODY_LIMIT_BYTES: usize = 2 * 1024 * 1024;

/// Default request timeout. Slow-loris attacks previously could tie up
/// connections indefinitely; this provides an upper bound per request.
const REQUEST_TIMEOUT_SECS: u64 = 60;

/// Build a CORS layer from the `ENGRAM_ALLOWED_ORIGINS` env var (comma
/// separated). When the variable is unset we fall back to the same origin
/// only (no cross-origin access), which is the safest default.
///
/// SECURITY: `CorsLayer::permissive()` was previously used here; combined with
/// cookie-based GUI auth that exposed every tenant's data to any internet
/// origin via CSRF. Callers that need third-party origins must enumerate them
/// explicitly.
fn build_cors_layer() -> CorsLayer {
    let base = CorsLayer::new()
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PUT,
            axum::http::Method::PATCH,
            axum::http::Method::DELETE,
            axum::http::Method::OPTIONS,
        ])
        .allow_headers([
            axum::http::header::AUTHORIZATION,
            axum::http::header::CONTENT_TYPE,
            axum::http::header::ACCEPT,
            axum::http::header::COOKIE,
        ])
        .allow_credentials(true);

    match std::env::var("ENGRAM_ALLOWED_ORIGINS") {
        Ok(raw) => {
            let origins: Vec<HeaderValue> = raw
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .filter_map(|s| HeaderValue::from_str(s).ok())
                .collect();
            if origins.is_empty() {
                tracing::warn!(
                    "ENGRAM_ALLOWED_ORIGINS set but empty/invalid; CORS will reject all origins"
                );
                base.allow_origin(AllowOrigin::list(Vec::<HeaderValue>::new()))
            } else {
                base.allow_origin(AllowOrigin::list(origins))
            }
        }
        Err(_) => {
            tracing::info!(
                "ENGRAM_ALLOWED_ORIGINS not set; CORS restricted (set explicitly to enable cross-origin access)"
            );
            base.allow_origin(AllowOrigin::list(Vec::<HeaderValue>::new()))
        }
    }
}

use crate::middleware::auth::auth_middleware;
use crate::middleware::rate_limit::rate_limit_middleware;
use crate::routes;
use crate::state::AppState;

/// Build the Axum router with all routes and middleware applied.
/// Exposed as a public function so integration tests can build an in-process app.
pub fn build_router(state: AppState) -> Router {
    // API routes that require bearer token auth
    let api_routes = Router::new()
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
        .merge(routes::jobs::router())
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
        // Rate limit runs after auth (inner layer), then auth sets context (outer layer)
        .layer(axum_mw::from_fn_with_state(
            state.clone(),
            rate_limit_middleware,
        ))
        .layer(axum_mw::from_fn_with_state(state.clone(), auth_middleware));

    // GUI routes handle their own cookie-based auth
    let gui_routes = routes::gui::router();

    Router::new()
        .merge(api_routes)
        .merge(gui_routes)
        // GUI SPA middleware intercepts HTML requests to SPA routes before API handlers
        .layer(axum_mw::from_fn_with_state(
            state.clone(),
            routes::gui::gui_spa_middleware,
        ))
        .layer(DefaultBodyLimit::max(BODY_LIMIT_BYTES))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(REQUEST_TIMEOUT_SECS),
        ))
        .layer(build_cors_layer())
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
