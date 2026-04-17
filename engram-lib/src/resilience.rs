// ============================================================================
// Resilience primitives: circuit breaker + exponential backoff retry
// ============================================================================
//
// Reusable helpers for guarding calls to external dependencies (reranker,
// LLM, webhooks). The primitives are intentionally lightweight so they can
// wrap any `async fn` without pulling in a large framework.
//
// ## Circuit breaker
//
//   State transitions:
//
//     Closed -- N consecutive failures --> Open
//     Open   -- cooldown elapses       --> HalfOpen
//     HalfOpen -- probe succeeds       --> Closed
//     HalfOpen -- probe fails          --> Open
//
//   While Open the breaker rejects with `CircuitError::Open` without
//   invoking the guarded closure, so the caller can fall back (for a
//   fail-open service) or surface a degraded result.
//
// ## Retry
//
// `retry_with_backoff` re-runs an operation up to `max_attempts` times with
// exponential backoff (`base * 2^attempt`). Only the caller knows which
// errors are transient, so the closure returns `Result<T, E>` and the
// helper retries on every `Err`. Wrap in your own "is this transient?"
// predicate if you need finer control.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Configuration for a circuit breaker.
#[derive(Debug, Clone)]
pub struct BreakerConfig {
    /// Number of consecutive failures before the breaker trips open.
    pub failure_threshold: u32,
    /// Duration the breaker stays Open before allowing a probe (HalfOpen).
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Closed,
    Open,
    HalfOpen,
}

struct Inner {
    state: State,
    consecutive_failures: u32,
    opened_at: Option<Instant>,
}

/// Thread-safe circuit breaker.
pub struct CircuitBreaker {
    inner: Mutex<Inner>,
    config: BreakerConfig,
}

/// Errors emitted by `CircuitBreaker::call`.
#[derive(Debug)]
pub enum CircuitError<E> {
    /// The breaker is currently open; the call was rejected without
    /// invoking the closure.
    Open,
    /// The closure ran and returned an error.
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

impl CircuitBreaker {
    pub fn new(config: BreakerConfig) -> Self {
        Self {
            inner: Mutex::new(Inner {
                state: State::Closed,
                consecutive_failures: 0,
                opened_at: None,
            }),
            config,
        }
    }

    /// Current state as observed at this instant (mostly for tests +
    /// metrics).
    pub fn state(&self) -> &'static str {
        match self.inner.lock().expect("breaker mutex").state {
            State::Closed => "closed",
            State::Open => "open",
            State::HalfOpen => "half_open",
        }
    }

    /// Attempt the call. If the breaker is Open and cooldown has not
    /// elapsed, returns `CircuitError::Open` without running the closure.
    pub async fn call<F, Fut, T, E>(&self, f: F) -> Result<T, CircuitError<E>>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T, E>>,
    {
        // Phase 1: decide whether to permit the call.
        {
            let mut inner = self.inner.lock().expect("breaker mutex");
            if inner.state == State::Open {
                let opened_at = inner.opened_at.unwrap_or_else(Instant::now);
                if opened_at.elapsed() >= self.config.cooldown {
                    inner.state = State::HalfOpen;
                } else {
                    return Err(CircuitError::Open);
                }
            }
        }

        // Phase 2: run the closure without holding the lock.
        match f().await {
            Ok(t) => {
                let mut inner = self.inner.lock().expect("breaker mutex");
                inner.state = State::Closed;
                inner.consecutive_failures = 0;
                inner.opened_at = None;
                Ok(t)
            }
            Err(e) => {
                let mut inner = self.inner.lock().expect("breaker mutex");
                inner.consecutive_failures = inner.consecutive_failures.saturating_add(1);
                if inner.state == State::HalfOpen
                    || inner.consecutive_failures >= self.config.failure_threshold
                {
                    inner.state = State::Open;
                    inner.opened_at = Some(Instant::now());
                }
                Err(CircuitError::Inner(e))
            }
        }
    }
}

/// Run `op` up to `max_attempts` times. On each error wait
/// `base_delay * 2^(attempt - 1)` then retry. Returns the last error if
/// all attempts fail.
#[tracing::instrument(skip_all, fields(max_attempts, base_delay_ms = base_delay.as_millis() as u64))]
pub async fn retry_with_backoff<F, Fut, T, E>(
    max_attempts: usize,
    base_delay: Duration,
    mut op: F,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    let mut attempt = 0usize;
    loop {
        attempt += 1;
        match op().await {
            Ok(t) => return Ok(t),
            Err(e) => {
                if attempt >= max_attempts.max(1) {
                    return Err(e);
                }
                let exp = attempt.saturating_sub(1).min(30) as u32;
                let delay = base_delay.saturating_mul(1u32 << exp);
                tokio::time::sleep(delay).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    fn tiny_config() -> BreakerConfig {
        BreakerConfig {
            failure_threshold: 2,
            cooldown: Duration::from_millis(40),
        }
    }

    #[tokio::test]
    async fn closed_stays_closed_on_success() {
        let cb = CircuitBreaker::new(tiny_config());
        for _ in 0..5 {
            let r: Result<i32, CircuitError<&str>> = cb.call(|| async { Ok(1) }).await;
            assert_eq!(r.ok(), Some(1));
        }
        assert_eq!(cb.state(), "closed");
    }

    #[tokio::test]
    async fn trips_open_after_threshold_failures() {
        let cb = CircuitBreaker::new(tiny_config());
        for _ in 0..2 {
            let r: Result<i32, CircuitError<&str>> = cb.call(|| async { Err("boom") }).await;
            assert!(matches!(r, Err(CircuitError::Inner("boom"))));
        }
        assert_eq!(cb.state(), "open");

        // Further calls should be rejected without running the closure.
        let called = Arc::new(AtomicU32::new(0));
        let c = called.clone();
        let r: Result<i32, CircuitError<&str>> = cb
            .call(|| async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(1)
            })
            .await;
        assert!(matches!(r, Err(CircuitError::Open)));
        assert_eq!(called.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn half_open_success_closes_breaker() {
        let cb = CircuitBreaker::new(tiny_config());
        for _ in 0..2 {
            let _: Result<i32, CircuitError<&str>> = cb.call(|| async { Err("boom") }).await;
        }
        assert_eq!(cb.state(), "open");
        tokio::time::sleep(Duration::from_millis(60)).await;

        let r: Result<i32, CircuitError<&str>> = cb.call(|| async { Ok(42) }).await;
        assert_eq!(r.ok(), Some(42));
        assert_eq!(cb.state(), "closed");
    }

    #[tokio::test]
    async fn half_open_failure_reopens_immediately() {
        let cb = CircuitBreaker::new(tiny_config());
        for _ in 0..2 {
            let _: Result<i32, CircuitError<&str>> = cb.call(|| async { Err("boom") }).await;
        }
        tokio::time::sleep(Duration::from_millis(60)).await;

        let r: Result<i32, CircuitError<&str>> = cb.call(|| async { Err("still broken") }).await;
        assert!(matches!(r, Err(CircuitError::Inner("still broken"))));
        assert_eq!(cb.state(), "open");
    }

    #[tokio::test]
    async fn retry_succeeds_on_second_attempt() {
        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();
        let r: Result<&'static str, &'static str> =
            retry_with_backoff(3, Duration::from_millis(1), || {
                let c = c.clone();
                async move {
                    let n = c.fetch_add(1, Ordering::SeqCst);
                    if n < 1 {
                        Err("transient")
                    } else {
                        Ok("done")
                    }
                }
            })
            .await;
        assert_eq!(r.ok(), Some("done"));
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn retry_returns_last_error_after_exhausting_attempts() {
        let r: Result<i32, &'static str> =
            retry_with_backoff(3, Duration::from_millis(1), || async { Err("nope") }).await;
        assert_eq!(r.err(), Some("nope"));
    }
}
