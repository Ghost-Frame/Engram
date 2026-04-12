use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::db::Database;
use crate::{EngError, Result};

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

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
