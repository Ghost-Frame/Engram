// ============================================================================
// Prometheus metrics middleware + /metrics endpoint
//
// Records per-route request counts and latency histograms using the `metrics`
// crate facade. The `metrics-exporter-prometheus` recorder exposes the data
// on the /metrics endpoint for scraping.
// ============================================================================

use axum::{
    body::Body,
    extract::MatchedPath,
    http::{Request, StatusCode},
    middleware::Next,
    response::IntoResponse,
    response::Response,
    routing::get,
    Router,
};
use metrics::{counter, histogram};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use std::sync::OnceLock;
use std::time::Instant;

use crate::state::AppState;

/// Global handle to the Prometheus recorder. Initialized once at startup via
/// `init_metrics()`. The `/metrics` endpoint calls `render()` on this handle.
static PROM_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// Install the Prometheus metrics recorder. Call once at server startup
/// (before any metrics are recorded). Returns Err if already initialized.
pub fn init_metrics() {
    let builder = PrometheusBuilder::new();
    match builder.install_recorder() {
        Ok(handle) => {
            let _ = PROM_HANDLE.set(handle);
            tracing::info!("prometheus metrics recorder installed");
        }
        Err(e) => {
            tracing::warn!("failed to install prometheus recorder: {}", e);
        }
    }
}

// ---------------------------------------------------------------------------
// Middleware: record request count + latency per route
// ---------------------------------------------------------------------------

#[tracing::instrument(skip_all, fields(middleware = "server.metrics"))]
pub async fn metrics_middleware(req: Request<Body>, next: Next) -> Response {
    let path = req
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let method = req.method().to_string();

    let start = Instant::now();
    let response = next.run(req).await;
    let duration = start.elapsed().as_secs_f64();

    let status = response.status().as_u16().to_string();

    counter!("http_requests_total", "method" => method.clone(), "path" => path.clone(), "status" => status.clone())
        .increment(1);
    histogram!("http_request_duration_seconds", "method" => method, "path" => path, "status" => status)
        .record(duration);

    response
}

// ---------------------------------------------------------------------------
// /metrics endpoint (unauthenticated, for Prometheus scraping)
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new().route("/metrics/prometheus", get(metrics_handler))
}

async fn metrics_handler() -> impl IntoResponse {
    match PROM_HANDLE.get() {
        Some(handle) => (StatusCode::OK, handle.render()),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            "metrics not initialized".to_string(),
        ),
    }
}
