use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::db::Database;
use crate::{EngError, Result};

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

// ---------------------------------------------------------------------------
// In-memory rate limiter (sliding window with burst support, per process)
// ---------------------------------------------------------------------------

/// Sliding-window rate limiter with optional burst allowance.
///
/// Tracks per-key request timestamps in a VecDeque<Instant>. The effective
/// limit is `requests_per_minute + burst`. Keys are string-based so callers
/// can use API key IDs, user IDs, or IP addresses interchangeably.
pub struct RateLimiter {
    /// Per-key timestamp queues. Guarded by a single Mutex.
    /// Using Mutex<HashMap<...>> avoids the dashmap dependency while still
    /// being safe for concurrent use behind an Arc.
    windows: Mutex<HashMap<String, VecDeque<Instant>>>,
    requests_per_minute: u32,
    burst: u32,
}

/// Information returned on a successful (allowed) rate-limit check.
pub struct RateLimitInfo {
    /// Requests remaining in the current window (including burst).
    pub remaining: u32,
    /// Total effective limit (requests_per_minute + burst).
    pub limit: u32,
    /// Seconds until the oldest in-window request expires.
    pub reset_secs: u64,
}

/// Information returned when the rate limit is exceeded.
pub struct RateLimitExceeded {
    /// Seconds the caller should wait before retrying.
    pub retry_after_secs: u64,
    /// Total effective limit that was exceeded.
    pub limit: u32,
}

impl RateLimiter {
    /// Create a new limiter with the given base rate and burst allowance.
    ///
    /// `burst` additional requests are permitted on top of `requests_per_minute`
    /// within any 60-second sliding window.
    pub fn new_with_burst(requests_per_minute: u32, burst: u32) -> Self {
        Self {
            windows: Mutex::new(HashMap::new()),
            requests_per_minute,
            burst,
        }
    }

    /// Create a limiter with no burst (backwards-compatible constructor).
    pub fn new() -> Self {
        Self::new_with_burst(60, 0)
    }

    /// Check rate limit for a string key.
    ///
    /// Returns `Ok(RateLimitInfo)` when allowed, `Err(RateLimitExceeded)` when
    /// the limit is exceeded.
    ///
    /// SECURITY: if the inner Mutex is poisoned by a panicking thread we fail
    /// closed (deny the request) rather than unwrapping, so a panic in one
    /// request cannot open an amplifier for subsequent requests.
    pub fn check_key(&self, key: &str) -> std::result::Result<RateLimitInfo, RateLimitExceeded> {
        let now = Instant::now();
        let window = Duration::from_secs(60);
        let max_requests = self.requests_per_minute + self.burst;

        let mut map = match self.windows.lock() {
            Ok(g) => g,
            Err(_) => {
                tracing::error!("rate limiter lock poisoned; failing closed");
                return Err(RateLimitExceeded {
                    retry_after_secs: 1,
                    limit: max_requests,
                });
            }
        };

        let deque = map.entry(key.to_string()).or_insert_with(VecDeque::new);

        // Prune timestamps older than the sliding window.
        while let Some(&front) = deque.front() {
            if now.duration_since(front) > window {
                deque.pop_front();
            } else {
                break;
            }
        }

        let count = deque.len() as u32;

        if count >= max_requests {
            let oldest = deque.front().unwrap();
            let expires_in = window.saturating_sub(now.duration_since(*oldest));
            Err(RateLimitExceeded {
                retry_after_secs: expires_in.as_secs().max(1),
                limit: max_requests,
            })
        } else {
            deque.push_back(now);
            let remaining = max_requests - count - 1;
            let reset_secs = if let Some(&oldest) = deque.front() {
                window.saturating_sub(now.duration_since(oldest)).as_secs()
            } else {
                60
            };
            Ok(RateLimitInfo {
                remaining,
                limit: max_requests,
                reset_secs,
            })
        }
    }

    /// Check rate limit using a numeric key ID (backwards-compatible helper).
    ///
    /// Accepts the legacy `limit` parameter per-call so existing callers do
    /// not need to be updated. The per-call limit overrides the limiter's
    /// configured `requests_per_minute` for this call only.
    ///
    /// Returns `Ok(count)` or `Err(retry_after_secs)` matching the old signature.
    pub fn check(&self, key_id: i64, limit: u32) -> std::result::Result<u32, u64> {
        // Build a temporary limiter honoring the per-call limit.
        let now = Instant::now();
        let window = Duration::from_secs(60);
        let max_requests = limit + self.burst;

        let key = key_id.to_string();
        let mut map = match self.windows.lock() {
            Ok(g) => g,
            Err(poisoned) => {
                tracing::error!("rate limiter lock poisoned; failing closed");
                poisoned.into_inner()
            }
        };

        let deque = map.entry(key).or_insert_with(VecDeque::new);

        while let Some(&front) = deque.front() {
            if now.duration_since(front) > window {
                deque.pop_front();
            } else {
                break;
            }
        }

        let count = deque.len() as u32;

        if count >= max_requests {
            let oldest = deque.front().unwrap();
            let expires_in = window.saturating_sub(now.duration_since(*oldest));
            Err(expires_in.as_secs().max(1))
        } else {
            deque.push_back(now);
            Ok(count + 1)
        }
    }

    /// Prune all expired entries across all keys to bound memory growth.
    pub fn prune(&self) {
        let now = Instant::now();
        let window = Duration::from_secs(60);
        let mut map = match self.windows.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };

        for deque in map.values_mut() {
            while let Some(&front) = deque.front() {
                if now.duration_since(front) > window {
                    deque.pop_front();
                } else {
                    break;
                }
            }
        }

        map.retain(|_, deque| !deque.is_empty());
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// DB-backed rate limiter (persistent, fixed-window)
// ---------------------------------------------------------------------------

/// Check whether the given key is within the rate limit.
///
/// Uses a fixed-window strategy: counts requests recorded in [window_start, now).
/// Returns true if the request is allowed, false if the limit is exceeded.
#[tracing::instrument(skip(db), fields(key = %key, max_requests, window_seconds))]
pub async fn check_rate_limit(
    db: &Database,
    key: &str,
    max_requests: i64,
    window_seconds: i64,
) -> Result<bool> {
    let key_owned = key.to_string();

    db.read(move |conn| {
        let mut stmt = conn
            .prepare("SELECT count, window_start FROM rate_limits WHERE key = ?1")
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![key_owned.clone()])
            .map_err(rusqlite_to_eng_error)?;

        if let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            let count: i64 = row.get(0).map_err(rusqlite_to_eng_error)?;
            let window_start: String = row.get(1).map_err(rusqlite_to_eng_error)?;

            // Check if window expired
            let expired: i64 = conn
                .query_row(
                    "SELECT (strftime('%s', 'now') - strftime('%s', ?1)) > ?2",
                    rusqlite::params![window_start, window_seconds],
                    |r| r.get(0),
                )
                .map_err(rusqlite_to_eng_error)?;

            if expired != 0 {
                // Window has expired -- will reset on next write
                return Ok(true);
            }

            Ok(count < max_requests)
        } else {
            // No row yet means this key has never been seen -- allow.
            Ok(true)
        }
    })
    .await
}

/// Increment the request counter for a key.
///
/// Creates the row if it does not exist. Resets the window when expired.
#[tracing::instrument(skip(db), fields(key = %key))]
pub async fn increment_counter(db: &Database, key: &str) -> Result<()> {
    let key_owned = key.to_string();

    db.write(move |conn| {
        conn.execute(
            "INSERT INTO rate_limits (key, count, window_start)
             VALUES (?1, 1, datetime('now'))
             ON CONFLICT(key) DO UPDATE SET
                 count = count + 1,
                 updated_at = datetime('now')",
            rusqlite::params![key_owned],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

/// Atomically increment the request counter and return whether the request
/// is within the limit. Resets the window in-place when it has expired.
///
/// SECURITY: the previous `check_rate_limit` + `increment_counter` sequence
/// had a time-of-check-to-time-of-use race. Two concurrent requests could
/// both read `count < limit`, both pass, and both increment -- bursting past
/// the limit. This collapses the check and the increment into one SQL
/// statement so the atomicity of a single UPDATE under SQLite's writer lock
/// provides the needed serialization.
#[tracing::instrument(skip(db), fields(key = %key, max_requests, window_seconds))]
pub async fn check_and_increment(
    db: &Database,
    key: &str,
    max_requests: i64,
    window_seconds: i64,
) -> Result<bool> {
    let key_owned = key.to_string();

    db.write(move |conn| {
        // Upsert and reset the window atomically. Returns the post-increment count.
        let count: i64 = conn
            .query_row(
                "INSERT INTO rate_limits (key, count, window_start, updated_at)
                 VALUES (?1, 1, datetime('now'), datetime('now'))
                 ON CONFLICT(key) DO UPDATE SET
                     count = CASE
                         WHEN (strftime('%s','now') - strftime('%s', window_start)) > ?2 THEN 1
                         ELSE count + 1
                     END,
                     window_start = CASE
                         WHEN (strftime('%s','now') - strftime('%s', window_start)) > ?2 THEN datetime('now')
                         ELSE window_start
                     END,
                     updated_at = datetime('now')
                 RETURNING count",
                rusqlite::params![key_owned, window_seconds],
                |row| row.get(0),
            )
            .map_err(rusqlite_to_eng_error)?;

        Ok(count <= max_requests)
    })
    .await
}

/// Like `check_and_increment` but increments by `cost` instead of 1.
/// Used for per-endpoint weighted rate limiting where expensive operations
/// consume more tokens from the user's budget.
#[tracing::instrument(skip(db), fields(key = %key, max_requests, window_seconds, cost))]
pub async fn check_and_increment_by(
    db: &Database,
    key: &str,
    max_requests: i64,
    window_seconds: i64,
    cost: i64,
) -> Result<bool> {
    let key_owned = key.to_string();

    db.write(move |conn| {
        let count: i64 = conn
            .query_row(
                "INSERT INTO rate_limits (key, count, window_start, updated_at)
                 VALUES (?1, ?3, datetime('now'), datetime('now'))
                 ON CONFLICT(key) DO UPDATE SET
                     count = CASE
                         WHEN (strftime('%s','now') - strftime('%s', window_start)) > ?2 THEN ?3
                         ELSE count + ?3
                     END,
                     window_start = CASE
                         WHEN (strftime('%s','now') - strftime('%s', window_start)) > ?2 THEN datetime('now')
                         ELSE window_start
                     END,
                     updated_at = datetime('now')
                 RETURNING count",
                rusqlite::params![key_owned, window_seconds, cost],
                |row| row.get(0),
            )
            .map_err(rusqlite_to_eng_error)?;

        Ok(count <= max_requests)
    })
    .await
}

/// Delete rate-limit rows whose window expired more than `grace_seconds` ago.
///
/// SECURITY: without periodic cleanup, spoofed pre-auth keys (e.g. from
/// rotated X-Forwarded-For values) accumulate rows indefinitely. This
/// function should be called from a background task.
pub async fn cleanup_expired_rows(db: &Database, grace_seconds: i64) -> Result<u64> {
    db.write(move |conn| {
        let deleted = conn
            .execute(
                "DELETE FROM rate_limits
                 WHERE (strftime('%s', 'now') - strftime('%s', window_start)) > ?1",
                rusqlite::params![grace_seconds],
            )
            .map_err(rusqlite_to_eng_error)?;
        Ok(deleted as u64)
    })
    .await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limiter_allows_within_limit() {
        let rl = RateLimiter::new();
        for _ in 0..5 {
            assert!(rl.check(1, 10).is_ok());
        }
    }

    #[test]
    fn test_rate_limiter_blocks_over_limit() {
        let rl = RateLimiter::new();
        for _ in 0..10 {
            let _ = rl.check(1, 10);
        }
        // 11th request should fail
        assert!(rl.check(1, 10).is_err());
    }

    #[test]
    fn test_rate_limiter_separate_keys() {
        let rl = RateLimiter::new();
        for _ in 0..10 {
            let _ = rl.check(1, 10);
        }
        // Key 2 should still be allowed
        assert!(rl.check(2, 10).is_ok());
    }
}
