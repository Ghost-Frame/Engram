use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DbPoolConfig {
    pub max_readers: usize,
    pub writer_count: usize,
    pub busy_timeout_ms: u64,
    pub wal_autocheckpoint: u64,
}

impl Default for DbPoolConfig {
    fn default() -> Self {
        let cpu_count = std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1);

        Self {
            max_readers: cpu_count * 2,
            writer_count: 1,
            busy_timeout_ms: 5_000,
            wal_autocheckpoint: 10_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SnapshotKind {
    Hourly,
    Daily,
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub path: PathBuf,
    pub created_at: DateTime<Utc>,
    pub kind: SnapshotKind,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreparedRestore {
    pub snapshot: Snapshot,
    pub dest_path: PathBuf,
    pub integrity_ok: bool,
    pub schema_version: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_count: Option<i64>,
}

/// Outcome of a restore-test probe on a backup file.
#[derive(Debug, Clone)]
pub struct RestoreReport {
    /// Value from `PRAGMA schema_version`.
    pub schema_version: i64,
    /// Row count of the `memories` table, or `None` if that table is absent.
    /// Absence does not fail the probe -- a fresh database legitimately has
    /// no `memories` yet -- but it is surfaced so callers can flag surprises.
    pub memory_count: Option<i64>,
    /// Count of tables reported by `sqlite_master`. Used as a liveness signal
    /// even when `memories` hasn't been created yet.
    pub table_count: i64,
}

#[derive(Debug, Clone, Copy)]
pub enum CheckpointMode {
    Passive,
    Full,
    Restart,
    Truncate,
}

impl CheckpointMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Passive => "PASSIVE",
            Self::Full => "FULL",
            Self::Restart => "RESTART",
            Self::Truncate => "TRUNCATE",
        }
    }
}

/// Summary of post-import integrity checks. Each field is a row count for a
/// condition that should be zero on a healthy import. A non-zero value means
/// the migrate tool (or operator) has cleanup work before enabling live
/// traffic.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PostImportValidation {
    /// Memories whose user_id does not resolve to a row in users.
    pub memories_orphan_user: i64,
    /// Memory rows marked latest that share a root with another latest row.
    pub memories_duplicate_latest: i64,
    /// Memories with a NULL active embedding column.
    pub memories_missing_embedding: i64,
    /// memory_links rows whose source or target memory no longer exists.
    pub links_orphan: i64,
    /// audit_log rows with NULL user_id (pre-tenant legacy rows).
    pub audit_log_null_user: i64,
    /// session_quality rows with user_id = 0 (pre-migration-6 drift).
    pub session_quality_zero_user: i64,
    /// behavioral_drift_events rows with user_id = 0.
    pub behavioral_drift_zero_user: i64,
}

impl PostImportValidation {
    /// True when every field is zero.
    pub fn is_clean(&self) -> bool {
        self.memories_orphan_user == 0
            && self.memories_duplicate_latest == 0
            && self.memories_missing_embedding == 0
            && self.links_orphan == 0
            && self.audit_log_null_user == 0
            && self.session_quality_zero_user == 0
            && self.behavioral_drift_zero_user == 0
    }
}
