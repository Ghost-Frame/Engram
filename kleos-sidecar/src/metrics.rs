use std::sync::OnceLock;

use metrics::{counter, gauge, histogram};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// Install the Prometheus recorder and retain the render handle.
/// Call once at startup. Subsequent calls are no-ops.
pub fn init() {
    if HANDLE.get().is_some() {
        return;
    }
    match PrometheusBuilder::new().install_recorder() {
        Ok(handle) => {
            let _ = HANDLE.set(handle);
        }
        Err(e) => {
            tracing::warn!("failed to install Prometheus recorder: {}", e);
        }
    }
}

/// Render current metrics in Prometheus text exposition format.
/// Returns an empty string if init() was never called.
pub fn render() -> String {
    match HANDLE.get() {
        Some(h) => h.render(),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Typed wrappers so call sites stay readable
// ---------------------------------------------------------------------------

pub fn inc_observations(count: u64) {
    counter!("sidecar_observations_total").increment(count);
}

/// Count one batch flush by terminal outcome.
pub fn inc_flush(result: &'static str) {
    counter!("sidecar_batch_flush_total", "result" => result).increment(1);
}

/// Count one local code-context attempt by outcome and operating mode.
pub fn inc_code_context(outcome: &'static str, mode: &'static str) {
    counter!("sidecar_code_context_total", "outcome" => outcome, "mode" => mode).increment(1);
}

/// Count one cached or live upstream health probe result.
pub fn inc_health_probe(result: &'static str) {
    counter!("sidecar_health_probe_total", "result" => result).increment(1);
}

/// Record wall-clock latency for one observation flush.
pub fn record_flush_latency(seconds: f64) {
    histogram!("sidecar_flush_latency_seconds").record(seconds);
}

/// Record wall-clock latency for local index refresh and retrieval.
pub fn record_code_context_latency(seconds: f64) {
    histogram!("sidecar_code_context_latency_seconds").record(seconds);
}

/// Record the number of snippets selected before shadow or injection policy.
pub fn record_code_context_snippets(count: f64) {
    histogram!("sidecar_code_context_snippets").record(count);
}

/// Record the approximate code tokens selected before policy application.
pub fn record_code_context_tokens(count: f64) {
    histogram!("sidecar_code_context_tokens").record(count);
}

/// Set the current number of active sidecar sessions.
pub fn set_active_sessions(n: f64) {
    gauge!("sidecar_active_sessions").set(n);
}

/// Set the total number of observations waiting for storage.
pub fn set_pending_depth(n: f64) {
    gauge!("sidecar_pending_depth").set(n);
}
