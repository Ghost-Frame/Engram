//! Point-in-time recovery helpers built on top of the scheduled VACUUM INTO
//! backups produced by `start_auto_backup_task`.
//!
//! Snapshots live at two cadences:
//!
//!   - Hourly: `<backup_dir>/engram-backup-YYYYMMDD-HHMMSS.db`
//!   - Daily:  `<backup_dir>/daily/engram-backup-YYYYMMDD-HHMMSS.db`
//!
//! Given a target timestamp, [`find_snapshot_for`] returns the newest snapshot
//! whose `created_at` is at or before the target. [`prepare_restore`] copies
//! that snapshot to a caller-supplied destination path and runs the existing
//! integrity + restore probes so the operator can decide whether to swap the
//! live database for it. The live swap is intentionally out-of-band: a hot
//! swap of an open SQLite file from within the serving process is unsafe and
//! operator discretion is required.
//!
//! The WAL in SQLite is checkpointed into the main file by VACUUM INTO, so a
//! single snapshot file is self-contained and does not need a sidecar -wal.

use crate::db::backup::{integrity_check, restore_test};
use crate::{EngError, Result};
use chrono::{DateTime, NaiveDateTime, Utc};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

const HOURLY_PREFIX: &str = "engram-backup-";
const BACKUP_SUFFIX: &str = ".db";

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

/// Parse `engram-backup-YYYYMMDD-HHMMSS.db` into a UTC timestamp. Returns
/// `None` for any filename that does not match the expected shape.
pub fn parse_backup_time(path: &Path) -> Option<DateTime<Utc>> {
    let name = path.file_name()?.to_str()?;
    let stem = name
        .strip_prefix(HOURLY_PREFIX)?
        .strip_suffix(BACKUP_SUFFIX)?;
    let naive = NaiveDateTime::parse_from_str(stem, "%Y%m%d-%H%M%S").ok()?;
    Some(naive.and_utc())
}

fn collect_from(dir: &Path, kind: SnapshotKind) -> Vec<Snapshot> {
    let read_dir = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return Vec::new(),
    };
    read_dir
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            let created_at = parse_backup_time(&path)?;
            let size_bytes = entry.metadata().ok()?.len();
            Some(Snapshot {
                path,
                created_at,
                kind,
                size_bytes,
            })
        })
        .collect()
}

/// List all snapshots under `backup_dir` (hourly) and `backup_dir/daily`
/// (daily), sorted newest-first.
pub fn list_snapshots(backup_dir: &Path) -> Vec<Snapshot> {
    let mut snapshots = collect_from(backup_dir, SnapshotKind::Hourly);
    snapshots.extend(collect_from(&backup_dir.join("daily"), SnapshotKind::Daily));
    snapshots.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    snapshots
}

/// Return the newest snapshot with `created_at <= target`, or `None` if the
/// target predates every known snapshot.
pub fn find_snapshot_for(backup_dir: &Path, target: DateTime<Utc>) -> Option<Snapshot> {
    list_snapshots(backup_dir)
        .into_iter()
        .find(|s| s.created_at <= target)
}

/// Copy the chosen snapshot to `dest_path` (out-of-place) and probe it with
/// the existing integrity and restore helpers. The live database is never
/// touched: the operator is responsible for swapping the prepared file in.
#[tracing::instrument(skip(backup_dir, dest_path))]
pub async fn prepare_restore(
    backup_dir: &Path,
    target: DateTime<Utc>,
    dest_path: &Path,
) -> Result<PreparedRestore> {
    let snapshot = find_snapshot_for(backup_dir, target).ok_or_else(|| {
        EngError::NotFound(format!(
            "no snapshot found at or before {}",
            target.to_rfc3339()
        ))
    })?;

    let src = snapshot.path.clone();
    let dst = dest_path.to_path_buf();
    let dst_for_copy = dst.clone();
    tokio::task::spawn_blocking(move || fs::copy(&src, &dst_for_copy))
        .await
        .map_err(|e| EngError::Internal(format!("restore copy join: {e}")))?
        .map_err(|e| EngError::Internal(format!("restore copy failed: {e}")))?;

    let integrity = integrity_check(&dst).await?;
    let integrity_ok = integrity.is_empty();
    let report = restore_test(&dst).await?;

    Ok(PreparedRestore {
        snapshot,
        dest_path: dst,
        integrity_ok,
        schema_version: report.schema_version,
        memory_count: report.memory_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::io::Write;

    fn write_snapshot_file(path: &Path, payload: &str) {
        let mut f = fs::File::create(path).expect("create snapshot file");
        f.write_all(payload.as_bytes()).expect("write payload");
    }

    fn unique_dir(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("{}-{}", prefix, uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    fn make_valid_sqlite(path: &Path) {
        let conn = rusqlite::Connection::open(path).expect("open sqlite");
        conn.execute_batch(
            "CREATE TABLE memories (id INTEGER PRIMARY KEY, content TEXT); \
             INSERT INTO memories (content) VALUES ('a'), ('b');",
        )
        .expect("schema");
    }

    #[test]
    fn parse_backup_time_parses_hourly_filename() {
        let path = PathBuf::from("/tmp/engram-backup-20260101-123456.db");
        let ts = parse_backup_time(&path).expect("parse");
        assert_eq!(ts, Utc.with_ymd_and_hms(2026, 1, 1, 12, 34, 56).unwrap());
    }

    #[test]
    fn parse_backup_time_rejects_unrelated_filenames() {
        assert!(parse_backup_time(Path::new("/tmp/random.txt")).is_none());
        assert!(parse_backup_time(Path::new("/tmp/engram-backup-bad.db")).is_none());
    }

    #[test]
    fn list_snapshots_merges_hourly_and_daily_desc() {
        let dir = unique_dir("engram-pitr-list");
        let daily = dir.join("daily");
        fs::create_dir_all(&daily).unwrap();

        write_snapshot_file(&dir.join("engram-backup-20260101-000000.db"), "h1");
        write_snapshot_file(&dir.join("engram-backup-20260102-000000.db"), "h2");
        write_snapshot_file(&daily.join("engram-backup-20260101-120000.db"), "d1");
        write_snapshot_file(&dir.join("unrelated.db"), "noise");

        let snaps = list_snapshots(&dir);
        assert_eq!(snaps.len(), 3);
        assert_eq!(
            snaps[0].created_at,
            Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap()
        );
        assert_eq!(snaps[0].kind, SnapshotKind::Hourly);
        assert_eq!(
            snaps[2].created_at,
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_snapshot_for_returns_latest_before_target() {
        let dir = unique_dir("engram-pitr-find");
        write_snapshot_file(&dir.join("engram-backup-20260101-000000.db"), "a");
        write_snapshot_file(&dir.join("engram-backup-20260101-060000.db"), "b");
        write_snapshot_file(&dir.join("engram-backup-20260101-120000.db"), "c");

        let target = Utc.with_ymd_and_hms(2026, 1, 1, 7, 0, 0).unwrap();
        let snap = find_snapshot_for(&dir, target).expect("snapshot");
        assert_eq!(
            snap.created_at,
            Utc.with_ymd_and_hms(2026, 1, 1, 6, 0, 0).unwrap()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_snapshot_for_inclusive_equal_target() {
        let dir = unique_dir("engram-pitr-equal");
        write_snapshot_file(&dir.join("engram-backup-20260101-060000.db"), "b");
        let target = Utc.with_ymd_and_hms(2026, 1, 1, 6, 0, 0).unwrap();
        let snap = find_snapshot_for(&dir, target).expect("snapshot");
        assert_eq!(snap.created_at, target);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_snapshot_for_returns_none_when_target_predates_all() {
        let dir = unique_dir("engram-pitr-before");
        write_snapshot_file(&dir.join("engram-backup-20260101-060000.db"), "b");
        let target = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        assert!(find_snapshot_for(&dir, target).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn prepare_restore_copies_and_verifies() {
        let dir = unique_dir("engram-pitr-restore");
        let src_name = "engram-backup-20260101-060000.db";
        let src = dir.join(src_name);
        make_valid_sqlite(&src);

        let dest = dir.join("restored.db");
        let target = Utc.with_ymd_and_hms(2026, 1, 1, 7, 0, 0).unwrap();
        let prepared = prepare_restore(&dir, target, &dest)
            .await
            .expect("prepare_restore");
        assert!(prepared.integrity_ok, "integrity should pass");
        assert_eq!(prepared.memory_count, Some(2));
        assert_eq!(prepared.dest_path, dest);
        assert!(dest.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn prepare_restore_returns_not_found_without_snapshot() {
        let dir = unique_dir("engram-pitr-empty");
        let dest = dir.join("restored.db");
        let target = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let err = prepare_restore(&dir, target, &dest).await.unwrap_err();
        assert!(matches!(err, EngError::NotFound(_)));
        let _ = fs::remove_dir_all(&dir);
    }
}
