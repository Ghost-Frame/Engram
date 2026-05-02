//! Shared tracing + OpenTelemetry bootstrap for all Engram binaries.
//!
//! `init_tracing` always installs a `tracing_subscriber::fmt` layer so local
//! log output is unchanged. When either `OTEL_EXPORTER_OTLP_ENDPOINT` or
//! `ENGRAM_OTLP_ENDPOINT` is set, an OTLP/gRPC span exporter is added and
//! tagged with `service.name = <service_name>` via the OTel resource.
//!
//! The returned `OtelGuard` owns the SDK `TracerProvider`. When dropped it
//! triggers a best-effort shutdown so in-flight spans get flushed before the
//! process exits. Callers typically bind it to a `_guard` local in `main`.

use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry::KeyValue;
use opentelemetry_otlp::{SpanExporter, WithExportConfig};
use opentelemetry_sdk::trace::TracerProvider;
use opentelemetry_sdk::Resource;
use std::sync::OnceLock;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

/// RAII guard that holds the OTel tracer provider (if one was installed) and
/// shuts it down on drop. Ignored when OTLP export is not configured.
pub struct OtelGuard {
    provider: Option<TracerProvider>,
}

impl Drop for OtelGuard {
    fn drop(&mut self) {
        if self.provider.take().is_some() {
            global::shutdown_tracer_provider();
        }
    }
}

static INIT: OnceLock<()> = OnceLock::new();

/// Initialise tracing for a binary.
///
/// - `service_name`: OTel `service.name` resource attribute (e.g. `"engram-server"`).
/// - `default_filter`: fallback `EnvFilter` directive when `RUST_LOG` is unset.
///
/// Safe to call more than once per process; subsequent calls are no-ops and
/// return an empty guard.
pub fn init_tracing(service_name: &str, default_filter: &str) -> OtelGuard {
    // Set_global_default only accepts one subscriber per process. Re-entering
    // would panic, so we gate the whole installation behind OnceLock.
    if INIT.set(()).is_err() {
        return OtelGuard { provider: None };
    }

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));

    let fmt_layer = tracing_subscriber::fmt::layer();

    let otlp_endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::env::var("ENGRAM_OTLP_ENDPOINT")
                .ok()
                .filter(|s| !s.is_empty())
        });

    match otlp_endpoint {
        Some(endpoint) => match build_otlp_provider(service_name, &endpoint) {
            Ok(provider) => {
                let tracer = provider.tracer(service_name.to_string());
                let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

                let registry = tracing_subscriber::registry()
                    .with(filter)
                    .with(fmt_layer)
                    .with(otel_layer);
                if registry.try_init().is_ok() {
                    global::set_tracer_provider(provider.clone());
                    tracing::info!(
                        service = service_name,
                        endpoint = endpoint.as_str(),
                        "OTLP span exporter enabled"
                    );
                    OtelGuard {
                        provider: Some(provider),
                    }
                } else {
                    OtelGuard { provider: None }
                }
            }
            Err(e) => {
                let _ = tracing_subscriber::registry()
                    .with(filter)
                    .with(fmt_layer)
                    .try_init();
                tracing::warn!(
                    service = service_name,
                    endpoint = endpoint.as_str(),
                    error = %e,
                    "OTLP exporter init failed; continuing with fmt subscriber only"
                );
                OtelGuard { provider: None }
            }
        },
        None => {
            let _ = tracing_subscriber::registry()
                .with(filter)
                .with(fmt_layer)
                .try_init();
            OtelGuard { provider: None }
        }
    }
}

fn build_otlp_provider(
    service_name: &str,
    endpoint: &str,
) -> Result<TracerProvider, Box<dyn std::error::Error>> {
    let exporter = SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()?;

    let resource = Resource::new(vec![KeyValue::new(
        "service.name",
        service_name.to_string(),
    )]);

    let provider = TracerProvider::builder()
        .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
        .with_resource(resource)
        .build();
    Ok(provider)
}
