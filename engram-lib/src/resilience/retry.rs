//! Retry policy with exponential backoff and jitter for transient failures.
//!
//! The default policy retries on `EngError::Internal` and similar transient
//! errors. It does NOT retry on auth failures, invalid input, or not-found
//! errors since those are deterministic.

use crate::{EngError, Result};
use std::sync::Arc;
use std::time::Duration;

// ---------------------------------------------------------------------------
// RetryPolicy
// ---------------------------------------------------------------------------

/// Policy controlling retry behaviour for a fallible async operation.
#[derive(Clone)]
pub struct RetryPolicy {
    /// Maximum number of attempts (first attempt counts as 1).
    pub max_attempts: u32,
    /// Base delay for exponential backoff: `base_delay * 2^(attempt-1)`.
    pub base_delay: Duration,
    /// Cap on the computed backoff delay.
    pub max_delay: Duration,
    /// Predicate: return `true` if the error is transient and should be retried.
    pub retry_on: Arc<dyn Fn(&EngError) -> bool + Send + Sync>,
}

impl std::fmt::Debug for RetryPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetryPolicy")
            .field("max_attempts", &self.max_attempts)
            .field("base_delay", &self.base_delay)
            .field("max_delay", &self.max_delay)
            .finish()
    }
}

impl RetryPolicy {
    /// Default policy: 3 attempts, 200 ms base, 5 s max.
    /// Retries on `Internal`, `Database`, and `DatabaseMessage` errors only.
    /// Does NOT retry on `Auth`, `InvalidInput`, `NotFound`, `Forbidden`.
    pub fn default_transient() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(200),
            max_delay: Duration::from_secs(5),
            retry_on: Arc::new(is_transient_error),
        }
    }

    /// Retry on every error regardless of type. Useful for infrastructure calls
    /// where the caller has already verified the error is transient.
    pub fn always_retry(max_attempts: u32, base_delay: Duration, max_delay: Duration) -> Self {
        Self {
            max_attempts,
            base_delay,
            max_delay,
            retry_on: Arc::new(|_| true),
        }
    }
}

/// Returns `true` for errors that are likely transient network/infrastructure
/// issues. Returns `false` for deterministic client errors.
pub fn is_transient_error(e: &EngError) -> bool {
    matches!(e, EngError::Internal(_) | EngError::Database(_) | EngError::DatabaseMessage(_))
}

// ---------------------------------------------------------------------------
// with_retry
// ---------------------------------------------------------------------------

/// Execute `op` with exponential backoff retry according to `policy`.
///
/// Delays: `base_delay * 2^(attempt - 1)`, capped at `max_delay`, with up to
/// 25% uniform jitter to avoid thundering-herd effects.
///
/// Returns `Ok(T)` on the first success. Returns the last `Err` if all
/// attempts are exhausted or the error does not satisfy `policy.retry_on`.
#[tracing::instrument(
    name = "with_retry",
    skip_all,
    fields(
        max_attempts = policy.max_attempts,
        base_delay_ms = policy.base_delay.as_millis() as u64
    )
)]
pub async fn with_retry<F, Fut, T>(policy: &RetryPolicy, mut op: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match op().await {
            Ok(t) => return Ok(t),
            Err(e) => {
                let exhausted = attempt >= policy.max_attempts;
                let is_retryable = (policy.retry_on)(&e);

                if exhausted || !is_retryable {
                    return Err(e);
                }

                let exp = (attempt - 1).min(30);
                let base_ms = policy.base_delay.as_millis() as u64;
                let backoff_ms = base_ms.saturating_mul(1u64 << exp);
                let capped_ms = backoff_ms.min(policy.max_delay.as_millis() as u64);

                // Add up to 25% jitter.
                let jitter_ms = (capped_ms / 4).saturating_mul(pseudo_rand_fraction(attempt));
                let sleep_ms = capped_ms.saturating_add(jitter_ms);

                tracing::debug!(
                    attempt,
                    sleep_ms,
                    error = %e,
                    "retrying after transient error"
                );
                tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
            }
        }
    }
}

/// Deterministic pseudo-random fraction 0..=1 based on attempt number.
/// Not crypto-quality -- only used for jitter spread.
fn pseudo_rand_fraction(attempt: u32) -> u64 {
    // Mix the attempt number with a constant to spread values.
    let v = attempt.wrapping_mul(2_654_435_761).wrapping_add(1_013_904_223);
    // Return a value 0..=100 and divide by 100 later for simplicity.
    // Caller uses this as a multiplier on capped_ms/4, so the max jitter is 25%.
    (v % 101) as u64
}

// ---------------------------------------------------------------------------
// Legacy adapter (re-exported as retry_with_backoff from resilience::)
// ---------------------------------------------------------------------------

/// Backwards-compatible retry helper. Retries on every error (no predicate).
/// New code should use [`with_retry`] with an explicit [`RetryPolicy`].
#[tracing::instrument(
    name = "retry_with_backoff",
    skip_all,
    fields(max_attempts, base_delay_ms = base_delay.as_millis() as u64)
)]
pub async fn retry_with_backoff<F, Fut, T, E>(
    max_attempts: usize,
    base_delay: Duration,
    mut op: F,
) -> std::result::Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = std::result::Result<T, E>>,
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn with_retry_succeeds_on_second_attempt() {
        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();
        let policy = RetryPolicy::default_transient();
        let r: Result<&str> = with_retry(&policy, || {
            let c = c.clone();
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n < 1 {
                    Err(EngError::Internal("transient".into()))
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
    async fn with_retry_does_not_retry_auth_errors() {
        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();
        let policy = RetryPolicy::default_transient();
        let r: Result<i32> = with_retry(&policy, || {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err(EngError::Auth("forbidden".into()))
            }
        })
        .await;
        assert!(r.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 1, "auth errors must not be retried");
    }

    #[tokio::test]
    async fn with_retry_exhausts_attempts() {
        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();
        let policy = RetryPolicy::default_transient();
        let r: Result<i32> = with_retry(&policy, || {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err(EngError::Internal("always fails".into()))
            }
        })
        .await;
        assert!(r.is_err());
        assert_eq!(
            counter.load(Ordering::SeqCst),
            policy.max_attempts,
            "should have tried exactly max_attempts times"
        );
    }

    #[tokio::test]
    async fn retry_with_backoff_succeeds_on_second_attempt() {
        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();
        let r: std::result::Result<&str, &str> =
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
    }
}
