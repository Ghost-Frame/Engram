//! Circuit breaker for guarding calls to external services.
//!
//! Classic three-state machine:
//!
//! - Closed: calls pass through; consecutive failures increment a counter.
//!   When the counter reaches `failure_threshold` the breaker trips Open.
//! - Open: calls fail fast with `EngError::Internal("circuit open: ...")` until
//!   `reset_timeout` elapses, at which point the breaker enters HalfOpen.
//! - HalfOpen: up to `half_open_max_calls` probe calls are allowed through.
//!   A success resets the breaker to Closed. A failure trips it back to Open.

use crate::{EngError, Result};
use std::sync::RwLock;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Observable circuit state returned by [`CircuitBreaker::state`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

impl std::fmt::Display for CircuitState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => write!(f, "closed"),
            Self::Open => write!(f, "open"),
            Self::HalfOpen => write!(f, "half_open"),
        }
    }
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

enum State {
    Closed {
        consecutive_failures: u32,
    },
    Open {
        opened_at: Instant,
    },
    HalfOpen {
        /// Number of probe calls currently allowed in-flight.
        in_flight: u32,
    },
}

// ---------------------------------------------------------------------------
// CircuitBreaker
// ---------------------------------------------------------------------------

/// Thread-safe circuit breaker.
///
/// # Example
/// ```rust,ignore
/// let cb = CircuitBreaker::new(5, Duration::from_secs(30), 1);
/// let result = cb.call(|| async { do_http_call().await }).await;
/// ```
pub struct CircuitBreaker {
    failure_threshold: u32,
    reset_timeout: Duration,
    half_open_max_calls: u32,
    state: RwLock<State>,
}

impl CircuitBreaker {
    /// Create a new circuit breaker.
    ///
    /// - `failure_threshold`: consecutive failures before tripping Open.
    /// - `reset_timeout`: how long to stay Open before allowing a probe.
    /// - `half_open_max_calls`: how many concurrent probes to allow in HalfOpen.
    pub fn new(failure_threshold: u32, reset_timeout: Duration, half_open_max_calls: u32) -> Self {
        Self {
            failure_threshold,
            reset_timeout,
            half_open_max_calls,
            state: RwLock::new(State::Closed {
                consecutive_failures: 0,
            }),
        }
    }

    /// Return a snapshot of the current circuit state for metrics/logging.
    pub fn state(&self) -> CircuitState {
        let guard = self.state.read().expect("circuit_breaker RwLock poisoned");
        match *guard {
            State::Closed { .. } => CircuitState::Closed,
            State::Open { .. } => CircuitState::Open,
            State::HalfOpen { .. } => CircuitState::HalfOpen,
        }
    }

    /// Attempt to execute `op` through the circuit breaker.
    ///
    /// - Closed: runs the op; tracks consecutive failures.
    /// - Open: fails fast with [`EngError::Internal`] containing "circuit open".
    ///   Transitions to HalfOpen automatically when `reset_timeout` has elapsed.
    /// - HalfOpen: allows up to `half_open_max_calls` probes; success resets to
    ///   Closed, failure reopens.
    #[tracing::instrument(name = "circuit_breaker.call", skip_all)]
    pub async fn call<F, Fut, T>(&self, op: F) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        // -- Phase 1: decide whether to permit the call (write lock) ----------
        {
            let mut guard = self.state.write().expect("circuit_breaker RwLock poisoned");

            match &*guard {
                State::Open { opened_at } => {
                    if opened_at.elapsed() >= self.reset_timeout {
                        // Timeout elapsed -- transition to HalfOpen.
                        *guard = State::HalfOpen { in_flight: 0 };
                    } else {
                        return Err(EngError::Internal("circuit open".to_string()));
                    }
                }
                State::HalfOpen { in_flight } => {
                    if *in_flight >= self.half_open_max_calls {
                        return Err(EngError::Internal("circuit open".to_string()));
                    }
                    let current = *in_flight;
                    *guard = State::HalfOpen {
                        in_flight: current + 1,
                    };
                }
                State::Closed { .. } => {
                    // Allow through.
                }
            }
        }

        // -- Phase 2: run the op (no lock held) --------------------------------
        let result = op().await;

        // -- Phase 3: record outcome (write lock) ------------------------------
        {
            let mut guard = self.state.write().expect("circuit_breaker RwLock poisoned");

            match result {
                Ok(t) => {
                    *guard = State::Closed {
                        consecutive_failures: 0,
                    };
                    return Ok(t);
                }
                Err(ref _e) => {
                    match &*guard {
                        State::HalfOpen { .. } => {
                            // Probe failed -- reopen immediately.
                            *guard = State::Open {
                                opened_at: Instant::now(),
                            };
                        }
                        State::Closed { consecutive_failures } => {
                            let new_failures = consecutive_failures + 1;
                            if new_failures >= self.failure_threshold {
                                *guard = State::Open {
                                    opened_at: Instant::now(),
                                };
                            } else {
                                *guard = State::Closed {
                                    consecutive_failures: new_failures,
                                };
                            }
                        }
                        State::Open { .. } => {
                            // Should not happen (we gated above), but leave Open.
                        }
                    }
                }
            }
        }

        result
    }
}

// ---------------------------------------------------------------------------
// Legacy adapter types (re-exported from resilience::) for backwards compat
// ---------------------------------------------------------------------------

/// Legacy configuration struct used by existing code (e.g. reranker).
/// New code should call [`CircuitBreaker::new`] directly.
#[derive(Debug, Clone)]
pub struct BreakerConfig {
    /// Number of consecutive failures before tripping.
    pub failure_threshold: u32,
    /// Duration to stay Open before probing (HalfOpen).
    pub cooldown: Duration,
}

impl Default for BreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            cooldown: Duration::from_secs(30),
        }
    }
}

/// Legacy wrapper around [`CircuitBreaker`] that preserves the old
/// `CircuitError<E>` API used by the HTTP reranker.
pub struct LegacyCircuitBreaker(pub(crate) CircuitBreaker);

/// Error type used by the legacy [`LegacyCircuitBreaker::call`] API.
#[derive(Debug)]
pub enum CircuitError<E> {
    /// The breaker is currently open; the call was rejected.
    Open,
    /// The guarded closure ran and returned an error.
    Inner(E),
}

impl<E: std::fmt::Display> std::fmt::Display for CircuitError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open => write!(f, "circuit breaker open"),
            Self::Inner(e) => write!(f, "{}", e),
        }
    }
}

impl<E: std::fmt::Debug + std::fmt::Display> std::error::Error for CircuitError<E> {}

impl LegacyCircuitBreaker {
    pub fn new(config: BreakerConfig) -> Self {
        Self(CircuitBreaker::new(
            config.failure_threshold,
            config.cooldown,
            1,
        ))
    }

    /// Current state string for metrics/tests.
    pub fn state(&self) -> &'static str {
        match self.0.state() {
            CircuitState::Closed => "closed",
            CircuitState::Open => "open",
            CircuitState::HalfOpen => "half_open",
        }
    }

    /// Execute `f` through the circuit breaker, returning `CircuitError<E>` on
    /// failure for backwards compatibility.
    pub async fn call<F, Fut, T, E>(&self, f: F) -> std::result::Result<T, CircuitError<E>>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = std::result::Result<T, E>>,
        E: std::fmt::Debug + std::fmt::Display,
    {
        // Check open state first using internal state directly.
        {
            let guard = self.0.state.read().expect("circuit_breaker RwLock poisoned");
            if let State::Open { opened_at } = &*guard {
                if opened_at.elapsed() < self.0.reset_timeout {
                    return Err(CircuitError::Open);
                }
            }
        }

        // Transition logic (duplicate of CircuitBreaker::call but for E != EngError).
        {
            let mut guard = self.0.state.write().expect("circuit_breaker RwLock poisoned");
            match &*guard {
                State::Open { opened_at } => {
                    if opened_at.elapsed() >= self.0.reset_timeout {
                        *guard = State::HalfOpen { in_flight: 0 };
                    } else {
                        return Err(CircuitError::Open);
                    }
                }
                State::HalfOpen { in_flight } => {
                    if *in_flight >= self.0.half_open_max_calls {
                        return Err(CircuitError::Open);
                    }
                    let current = *in_flight;
                    *guard = State::HalfOpen {
                        in_flight: current + 1,
                    };
                }
                State::Closed { .. } => {}
            }
        }

        let result = f().await;

        {
            let mut guard = self.0.state.write().expect("circuit_breaker RwLock poisoned");
            match &result {
                Ok(_) => {
                    *guard = State::Closed {
                        consecutive_failures: 0,
                    };
                }
                Err(_) => match &*guard {
                    State::HalfOpen { .. } => {
                        *guard = State::Open {
                            opened_at: Instant::now(),
                        };
                    }
                    State::Closed { consecutive_failures } => {
                        let new_failures = consecutive_failures + 1;
                        if new_failures >= self.0.failure_threshold {
                            *guard = State::Open {
                                opened_at: Instant::now(),
                            };
                        } else {
                            *guard = State::Closed {
                                consecutive_failures: new_failures,
                            };
                        }
                    }
                    State::Open { .. } => {}
                },
            }
        }

        result.map_err(CircuitError::Inner)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    fn make_breaker(threshold: u32, timeout_ms: u64) -> CircuitBreaker {
        CircuitBreaker::new(threshold, Duration::from_millis(timeout_ms), 1)
    }

    // -- Closed -> Open on threshold failures --------------------------------

    #[tokio::test]
    async fn closed_transitions_to_open_on_threshold_failures() {
        let cb = make_breaker(3, 1000);
        assert_eq!(cb.state(), CircuitState::Closed);

        for _ in 0..2 {
            let r: Result<()> = cb.call(|| async { Err(EngError::Internal("boom".into())) }).await;
            assert!(r.is_err());
            assert_eq!(cb.state(), CircuitState::Closed);
        }

        // Third failure -- should trip open.
        let r: Result<()> = cb.call(|| async { Err(EngError::Internal("boom".into())) }).await;
        assert!(r.is_err());
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[tokio::test]
    async fn open_rejects_without_calling_op() {
        let cb = make_breaker(1, 10_000);
        // Trip the breaker.
        let _: Result<()> = cb.call(|| async { Err(EngError::Internal("x".into())) }).await;
        assert_eq!(cb.state(), CircuitState::Open);

        let called = Arc::new(AtomicU32::new(0));
        let c = called.clone();
        let r: Result<i32> = cb
            .call(|| async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(1)
            })
            .await;
        assert!(r.is_err());
        assert_eq!(called.load(Ordering::SeqCst), 0, "op must not be called while open");
    }

    // -- Open -> HalfOpen after reset timeout --------------------------------

    #[tokio::test]
    async fn open_transitions_to_half_open_after_timeout() {
        let cb = make_breaker(1, 50);
        let _: Result<()> = cb.call(|| async { Err(EngError::Internal("x".into())) }).await;
        assert_eq!(cb.state(), CircuitState::Open);

        tokio::time::sleep(Duration::from_millis(80)).await;

        // Calling now should enter HalfOpen and allow the probe through.
        let called = Arc::new(AtomicU32::new(0));
        let c = called.clone();
        let r: Result<i32> = cb
            .call(|| async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(42)
            })
            .await;
        assert_eq!(r.ok(), Some(42));
        assert_eq!(called.load(Ordering::SeqCst), 1);
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    // -- HalfOpen -> Closed on success ---------------------------------------

    #[tokio::test]
    async fn half_open_success_closes_breaker() {
        let cb = make_breaker(1, 30);
        let _: Result<()> = cb.call(|| async { Err(EngError::Internal("x".into())) }).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        let r: Result<i32> = cb.call(|| async { Ok(7) }).await;
        assert_eq!(r.ok(), Some(7));
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    // -- HalfOpen -> Open on failure -----------------------------------------

    #[tokio::test]
    async fn half_open_failure_reopens_breaker() {
        let cb = make_breaker(1, 30);
        let _: Result<()> = cb.call(|| async { Err(EngError::Internal("x".into())) }).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        let r: Result<()> = cb
            .call(|| async { Err(EngError::Internal("still broken".into())) })
            .await;
        assert!(r.is_err());
        assert_eq!(cb.state(), CircuitState::Open);
    }

    // -- Success resets failure counter in Closed ----------------------------

    #[tokio::test]
    async fn success_resets_failure_counter() {
        let cb = make_breaker(3, 1000);

        // Two failures -- still Closed.
        for _ in 0..2 {
            let _: Result<()> = cb.call(|| async { Err(EngError::Internal("x".into())) }).await;
        }
        assert_eq!(cb.state(), CircuitState::Closed);

        // Success -- counter resets.
        let r: Result<i32> = cb.call(|| async { Ok(1) }).await;
        assert_eq!(r.ok(), Some(1));

        // Now 3 more failures needed to trip (counter was reset).
        for _ in 0..2 {
            let _: Result<()> = cb.call(|| async { Err(EngError::Internal("x".into())) }).await;
        }
        assert_eq!(cb.state(), CircuitState::Closed);
    }
}
