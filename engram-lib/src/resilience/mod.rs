//! Resilience primitives: circuit breaker, retry, and dead-letter support.
//!
//! Provides a unified [`ServiceGuard`] that combines all three patterns for
//! guarding calls to internal services (axon, brain, broca, chiasm, loom,
//! soma, thymus, reranker, embedder).
//!
//! # Quick start
//!
//! ```rust,ignore
//! let guard = ServiceGuard::new(
//!     "reranker",
//!     CircuitBreaker::new(5, Duration::from_secs(30), 1),
//!     RetryPolicy::default_transient(),
//!     db.clone(),
//! );
//!
//! let result = guard
//!     .call("rerank", serde_json::json!({"query": q}), || async {
//!         http_rerank(q).await
//!     })
//!     .await;
//! ```
//!
//! # State machine (circuit breaker)
//!
//! ```text
//! Closed -- N consecutive failures --> Open
//! Open   -- reset_timeout elapses  --> HalfOpen
//! HalfOpen -- probe succeeds       --> Closed
//! HalfOpen -- probe fails          --> Open
//! ```
//!
//! # Retry
//!
//! Transient failures (Internal / Database) are retried up to
//! `RetryPolicy::max_attempts` times with exponential backoff. Auth,
//! InvalidInput, and NotFound errors are not retried.
//!
//! # Dead letter
//!
//! When all retry attempts fail or the circuit is open, a row is written to
//! `service_dead_letters` so operators can inspect and replay it.

pub mod circuit_breaker;
pub mod dead_letter;
pub mod retry;

// Re-export the primary public API.
pub use circuit_breaker::{CircuitBreaker, CircuitState};
pub use dead_letter::{record_dead_letter, ServiceDeadLetter};
pub use retry::{with_retry, RetryPolicy};

// Legacy re-exports for backwards compatibility with existing callsites
// (e.g. reranker/mod.rs which imports BreakerConfig, CircuitError,
//  CircuitBreaker by the old name).
pub use circuit_breaker::{BreakerConfig, CircuitError, LegacyCircuitBreaker};

// The old `retry_with_backoff` symbol used by reranker/mod.rs.
pub use retry::retry_with_backoff;

use crate::db::Database;
use crate::{EngError, Result};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// ServiceGuard
// ---------------------------------------------------------------------------

/// Unified resilience wrapper for internal service calls.
///
/// Combines a circuit breaker, a retry policy, and a dead-letter store.
/// Construct one per logical service endpoint and hold it in application state.
pub struct ServiceGuard {
    breaker: CircuitBreaker,
    retry: RetryPolicy,
    db: Arc<Database>,
    service_name: String,
}

impl ServiceGuard {
    /// Create a new `ServiceGuard`.
    pub fn new(
        service_name: impl Into<String>,
        breaker: CircuitBreaker,
        retry: RetryPolicy,
        db: Arc<Database>,
    ) -> Self {
        Self {
            breaker,
            retry,
            db,
            service_name: service_name.into(),
        }
    }

    /// Convenience constructor with sensible defaults:
    /// - Circuit breaker: 5 failures, 30 s reset, 1 half-open probe.
    /// - Retry: default transient policy (3 attempts, 200 ms base, 5 s max).
    pub fn with_defaults(service_name: impl Into<String>, db: Arc<Database>) -> Self {
        Self::new(
            service_name,
            CircuitBreaker::new(5, std::time::Duration::from_secs(30), 1),
            RetryPolicy::default_transient(),
            db,
        )
    }

    /// Current circuit state. Useful for health checks and metrics.
    pub fn circuit_state(&self) -> CircuitState {
        self.breaker.state()
    }

    /// Execute `op` with retry and circuit breaker protection.
    ///
    /// # Behaviour
    ///
    /// 1. If the circuit is Open, fail fast and write a "circuit_open"
    ///    dead-letter row, then return an error.
    /// 2. Otherwise attempt `op` through the circuit breaker and retry policy.
    /// 3. If all retries fail, write a dead-letter row with the final error.
    /// 4. On success, return the value (no dead-letter row written).
    #[tracing::instrument(
        name = "service_guard.call",
        skip(self, payload, op),
        fields(service = %self.service_name, operation)
    )]
    pub async fn call<F, Fut, T>(
        &self,
        operation: &str,
        payload: serde_json::Value,
        op: F,
    ) -> Result<T>
    where
        F: Fn() -> Fut + Send + Sync,
        Fut: std::future::Future<Output = Result<T>> + Send,
        T: Send,
    {
        // Fast-path: circuit already open -- dead-letter immediately.
        if self.breaker.state() == CircuitState::Open {
            let err_msg = format!("circuit open for service '{}'", self.service_name);
            self.write_dead_letter(operation, payload, "circuit_open", 0).await;
            return Err(EngError::Internal(err_msg));
        }

        // Attempt with retry inside the circuit breaker guard.
        let service_name = &self.service_name;
        let retry = &self.retry;
        let breaker = &self.breaker;

        let mut attempt = 0u32;

        loop {
            attempt += 1;

            let result = breaker.call(&op).await;

            match result {
                Ok(t) => return Ok(t),
                Err(e) => {
                    // If the circuit just tripped Open, stop retrying.
                    let circuit_now_open = breaker.state() == CircuitState::Open;

                    let exhausted = attempt >= retry.max_attempts;
                    let retryable = (retry.retry_on)(&e) && !circuit_now_open;

                    if !retryable || exhausted {
                        let err_tag = if circuit_now_open {
                            "circuit_open_after_failure"
                        } else {
                            "exhausted"
                        };
                        let err_str = e.to_string();
                        self.write_dead_letter(operation, payload, &err_str, attempt).await;
                        tracing::warn!(
                            service = %service_name,
                            operation,
                            attempt,
                            %err_tag,
                            "service call dead-lettered"
                        );
                        return Err(e);
                    }

                    // Compute delay and sleep before next attempt.
                    let exp = (attempt - 1).min(30);
                    let base_ms = retry.base_delay.as_millis() as u64;
                    let backoff_ms = base_ms
                        .saturating_mul(1u64 << exp)
                        .min(retry.max_delay.as_millis() as u64);

                    tracing::debug!(
                        service = %service_name,
                        operation,
                        attempt,
                        sleep_ms = backoff_ms,
                        error = %e,
                        "retrying service call after transient error"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                }
            }
        }
    }

    /// Internal helper: write a dead-letter row, swallowing errors to avoid
    /// masking the original failure.
    async fn write_dead_letter(
        &self,
        operation: &str,
        payload: serde_json::Value,
        error: &str,
        retry_count: u32,
    ) {
        if let Err(e) = record_dead_letter(
            &self.db,
            &self.service_name,
            operation,
            payload,
            error,
            retry_count,
        )
        .await
        {
            tracing::error!(
                service = %self.service_name,
                operation,
                dead_letter_error = %e,
                "failed to write dead-letter row"
            );
        }
    }
}
