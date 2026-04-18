use super::types::TacticalEntry;
use chrono::{Duration, Utc};
use std::collections::HashMap;

const TTL_HOURS: i64 = 24;
const FAILURE_THRESHOLD: u32 = 3;

/// Short-term memory for operational context with 24h TTL.
#[derive(Debug, Clone)]
pub struct TacticalMemory {
    entries: HashMap<String, TacticalEntry>,
}

impl Default for TacticalMemory {
    fn default() -> Self {
        Self::new()
    }
}

impl TacticalMemory {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Store a tactical memory entry.
    pub fn store(&mut self, key: String, value: String) {
        let now = Utc::now();
        self.entries.insert(
            key.clone(),
            TacticalEntry {
                key,
                value,
                created_at: now,
                expires_at: now + Duration::hours(TTL_HOURS),
                failure_count: 0,
            },
        );
    }

    /// Retrieve a tactical entry (returns None if expired).
    pub fn get(&self, key: &str) -> Option<&TacticalEntry> {
        let entry = self.entries.get(key)?;
        if Utc::now() > entry.expires_at || entry.failure_count >= FAILURE_THRESHOLD {
            return None;
        }
        Some(entry)
    }

    /// Record a failure for an entry. 3 consecutive failures invalidates it.
    pub fn record_failure(&mut self, key: &str) {
        if let Some(entry) = self.entries.get_mut(key) {
            entry.failure_count += 1;
        }
    }

    /// Reset failure count on success.
    pub fn record_success(&mut self, key: &str) {
        if let Some(entry) = self.entries.get_mut(key) {
            entry.failure_count = 0;
        }
    }

    /// Purge all expired entries.
    pub fn purge_expired(&mut self) {
        let now = Utc::now();
        self.entries
            .retain(|_, e| now <= e.expires_at && e.failure_count < FAILURE_THRESHOLD);
    }

    /// List all active entries.
    pub fn list_active(&self) -> Vec<&TacticalEntry> {
        let now = Utc::now();
        self.entries
            .values()
            .filter(|e| now <= e.expires_at && e.failure_count < FAILURE_THRESHOLD)
            .collect()
    }

    /// Count active entries.
    pub fn active_count(&self) -> usize {
        self.list_active().len()
    }
}
