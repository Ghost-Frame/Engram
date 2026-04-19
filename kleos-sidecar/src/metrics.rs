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

pub fn inc_flush(result: &'static str) {
    counter!("sidecar_batch_flush_total", "result" => result).increment(1);
}

pub fn inc_compress(outcome: &'static str) {
    counter!("sidecar_compress_total", "outcome" => outcome).increment(1);
}

pub fn inc_health_probe(result: &'static str) {
    counter!("sidecar_health_probe_total", "result" => result).increment(1);
}

pub fn record_flush_latency(seconds: f64) {
    histogram!("sidecar_flush_latency_seconds").record(seconds);
}

pub fn record_compress_latency(seconds: f64) {
    histogram!("sidecar_compress_latency_seconds").record(seconds);
}

pub fn set_active_sessions(n: f64) {
    gauge!("sidecar_active_sessions").set(n);
}

pub fn set_pending_depth(n: f64) {
    gauge!("sidecar_pending_depth").set(n);
}
