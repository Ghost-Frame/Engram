use axum::{
    extract::DefaultBodyLimit,
    http::{header, HeaderName, HeaderValue},
    middleware as axum_mw, Router,
};
use std::time::Duration;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

/// Default request body limit: 2 MiB. Prevents memory exhaustion from oversized payloads.
const BODY_LIMIT_BYTES: usize = 2 * 1024 * 1024;

/// Default request timeout. Slow-loris attacks previously could tie up
/// connections indefinitely; this provides an upper bound per request.
/// Raised to 120s to accommodate ingestion routes; tighter per-route
/// timeouts are applied on health (1s), search (10s), and context (30s).
const REQUEST_TIMEOUT_SECS: u64 = 120;

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
        // Cache preflight for an hour so cross-origin GUIs do not re-OPTIONS
        // every API call.
        .max_age(Duration::from_secs(3600))
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
use crate::middleware::json_depth::json_depth_middleware;
use crate::middleware::rate_limit::{preauth_rate_limit_middleware, rate_limit_middleware};
use crate::middleware::safe_mode::safe_mode_middleware;
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
        .merge(routes::approvals::router())
        .merge(routes::errors::router())
        // Rate limit runs after auth (inner layer), then auth sets context (outer layer)
        .layer(axum_mw::from_fn_with_state(state.clone(), rate_limit_middleware))
        .layer(axum_mw::from_fn_with_state(state.clone(), auth_middleware))
        .layer(axum_mw::from_fn_with_state(
            state.clone(),
            preauth_rate_limit_middleware,
        ))
        // Safe mode blocks writes before auth runs so the 503 is always fast.
        .layer(axum_mw::from_fn_with_state(
            state.clone(),
            safe_mode_middleware,
        ));

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
        // JSON depth check runs after body limit but before routing so
        // downstream Json<T> extractors never see a stack-bomb payload.
        .layer(axum_mw::from_fn(json_depth_middleware))
        .layer(DefaultBodyLimit::max(BODY_LIMIT_BYTES))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(REQUEST_TIMEOUT_SECS),
        ))
        .layer(build_cors_layer())
        // SECURITY: baseline response hardening headers. Applied as overrides
        // so individual handlers cannot accidentally downgrade them.
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("strict-transport-security"),
            HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-permitted-cross-domain-policies"),
            HeaderValue::from_static("none"),
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Listen for SIGTERM / SIGINT so the server can drain in-flight requests
/// before exiting. Without this the process dies hard on signal and any
/// in-flight writes to SQLite can be cut mid-statement.
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!("failed to install ctrl-c handler: {}", e);
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => {
                tracing::error!("failed to install SIGTERM handler: {}", e);
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("shutdown signal received, draining connections");
}

pub async fn run(state: AppState) -> Result<(), Box<dyn std::error::Error>> {
    let addr = format!("{}:{}", state.config.host, state.config.port);
    let app = build_router(state);
    tracing::info!("engram-server listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    // SECURITY: install ConnectInfo<SocketAddr> so rate-limit middleware can
    // read the real TCP peer address instead of falling back to "unknown".
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}
