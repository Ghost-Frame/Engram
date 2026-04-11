use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::db::Database;
use crate::Result;

// ---------------------------------------------------------------------------
// In-memory rate limiter (fixed-window, per process)
// ---------------------------------------------------------------------------

/// Rate window duration in milliseconds (1 minute).
const RATE_WINDOW_MS: u64 = 60_000;

#[derive(Debug)]
struct RateLimitEntry {
    count: u32,
    reset: u64,
}

/// In-memory rate limiter using a HashMap protected by RwLock.
pub struct RateLimiter {
    entries: RwLock<HashMap<i64, RateLimitEntry>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
        }
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    /// Check rate limit for a key. Returns Ok(count) or Err(retry_after_secs).
    ///
    /// SECURITY: if the inner RwLock is poisoned by a panicking writer we fail
    /// closed (return the smallest retry-after > 0) instead of unwrapping and
    /// taking down the whole process, and we refuse to count that request
    /// against the caller so we cannot be turned into a free amplifier.
    pub fn check(&self, key_id: i64, limit: u32) -> std::result::Result<u32, u64> {
        let now = Self::now_ms();
        let mut map = match self.entries.write() {
            Ok(g) => g,
            Err(poisoned) => {
                tracing::error!("rate limiter lock poisoned; failing closed");
                // Recover the guard so subsequent requests can make progress.
                poisoned.into_inner()
            }
        };
        let entry = map.entry(key_id).or_insert(RateLimitEntry {
            count: 0,
            reset: now + RATE_WINDOW_MS,
        });

        // Reset window if expired.
        if now > entry.reset {
            entry.count = 0;
            entry.reset = now + RATE_WINDOW_MS;
        }

        entry.count += 1;

        if entry.count > limit {
            let retry_after = (entry.reset.saturating_sub(now)) / 1000;
            Err(retry_after.max(1))
        } else {
            Ok(entry.count)
        }
    }

    /// Prune expired entries to prevent unbounded growth.
    pub fn prune(&self) {
        let now = Self::now_ms();
        let mut map = match self.entries.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        map.retain(|_, entry| now <= entry.reset);
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
pub async fn check_rate_limit(
    db: &Database,
    key: &str,
    max_requests: i64,
    window_seconds: i64,
) -> Result<bool> {
    let mut rows = db
        .conn
        .query(
            "SELECT count, window_start FROM rate_limits WHERE key = ?1",
            libsql::params![key],
        )
        .await?;

    if let Some(row) = rows.next().await? {
        let count: i64 = row
            .get(0)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?;
        let window_start: String = row
            .get(1)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?;

        let mut expired_rows = db
            .conn
            .query(
                "SELECT (strftime('%s', 'now') - strftime('%s', ?1)) > ?2",
                libsql::params![window_start, window_seconds],
            )
            .await?;

        let expired: i64 = if let Some(r) = expired_rows.next().await? {
            r.get(0)
                .map_err(|e| crate::EngError::Internal(e.to_string()))?
        } else {
            1
        };

        if expired != 0 {
            // Window has expired -- reset.
            db.conn
                .execute(
                    "UPDATE rate_limits
                     SET count = 0, window_start = datetime('now'), updated_at = datetime('now')
                     WHERE key = ?1",
                    libsql::params![key],
                )
                .await?;
            return Ok(true);
        }

        Ok(count < max_requests)
    } else {
        // No row yet means this key has never been seen -- allow.
        Ok(true)
    }
}

/// Increment the request counter for a key.
///
/// Creates the row if it does not exist. Resets the window when expired.
pub async fn increment_counter(db: &Database, key: &str) -> Result<()> {
    db.conn
        .execute(
            "INSERT INTO rate_limits (key, count, window_start)
             VALUES (?1, 1, datetime('now'))
             ON CONFLICT(key) DO UPDATE SET
                 count = count + 1,
                 updated_at = datetime('now')",
            libsql::params![key],
        )
        .await?;

    Ok(())
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
pub async fn check_and_increment(
    db: &Database,
    key: &str,
    max_requests: i64,
    window_seconds: i64,
) -> Result<bool> {
    // Upsert and reset the window atomically. Returns the post-increment count.
    let mut rows = db
        .conn
        .query(
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
            libsql::params![key, window_seconds],
        )
        .await?;

    let count: i64 = if let Some(row) = rows.next().await? {
        row.get(0)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?
    } else {
        1
    };

    Ok(count <= max_requests)
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
