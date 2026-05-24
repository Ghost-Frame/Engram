//! Background disk-usage sampler for E2 soft disk quota enforcement.
//!
//! `sample_active_tenants` walks each resident tenant's shard directory with
//! walkdir, updates `tenant_state.disk_bytes_estimate` inside the shard, and
//! flips the handle's `read_only` flag when the disk quota is exceeded.
//!
//! The sampler runs as a periodic tokio task (not through the durable jobs
//! queue) because it is: (a) cheap to re-run on restart, (b) never retried
//! with backoff, (c) not durable.

use crate::tenant::registry::TenantRegistry;
use crate::{EngError, Result};
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, warn};
use walkdir::WalkDir;

/// Walk `path` recursively and return the total byte size of all regular files.
///
/// Symlinks are not followed. Individual file stat errors are logged and
/// skipped so one unreadable file does not abort the entire walk.
pub fn du_bytes(path: &Path) -> std::io::Result<u64> {
    let mut total: u64 = 0;
    for entry in WalkDir::new(path).follow_links(false) {
        match entry {
            Ok(e) => {
                if e.file_type().is_file() {
                    match e.metadata() {
                        Ok(m) => total += m.len(),
                        Err(err) => {
                            debug!("disk sampler: stat failed for {:?}: {}", e.path(), err);
                        }
                    }
                }
            }
            Err(err) => {
                debug!("disk sampler: walk error: {}", err);
            }
        }
    }
    Ok(total)
}

/// Sample disk usage for all resident tenant handles and enforce the disk quota.
///
/// For each resident handle:
/// 1. Run `du_bytes` on the shard directory (blocking, via spawn_blocking).
/// 2. Update `tenant_state.disk_bytes_estimate` and `disk_sampled_at` in the shard.
/// 3. Compare against `quota.disk_bytes`; set `handle.read_only` accordingly.
///
/// Non-fatal errors for individual tenants are logged and skipped so one
/// broken shard does not block the entire sample pass.
pub async fn sample_active_tenants(registry: &Arc<TenantRegistry>) -> Result<()> {
    let handles = registry.snapshot_all_handles().await;

    for handle in handles {
        let shard_path = handle.shard_path().to_path_buf();
        let handle_clone = Arc::clone(&handle);

        // Blocking walkdir run moved off the async executor.
        let bytes_result = tokio::task::spawn_blocking(move || du_bytes(&shard_path)).await;

        let disk_bytes = match bytes_result {
            Ok(Ok(b)) => b as i64,
            Ok(Err(e)) => {
                warn!(
                    tenant = %handle_clone.tenant_id,
                    "disk sampler: walkdir failed: {}",
                    e
                );
                continue;
            }
            Err(e) => {
                warn!(
                    tenant = %handle_clone.tenant_id,
                    "disk sampler: spawn_blocking panicked: {}",
                    e
                );
                continue;
            }
        };

        // Write estimate into shard database.
        let db = handle.database();
        let update_result = db
            .write(move |conn| {
                conn.execute(
                    "UPDATE tenant_state SET value = ?1, updated_at = datetime('now') \
                     WHERE key = 'disk_bytes_estimate'",
                    rusqlite::params![disk_bytes],
                )
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
                conn.execute(
                    "UPDATE tenant_state SET value = strftime('%s', 'now'), \
                     updated_at = datetime('now') WHERE key = 'disk_sampled_at'",
                    [],
                )
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
                Ok(())
            })
            .await;

        if let Err(e) = update_result {
            warn!(
                tenant = %handle.tenant_id,
                "disk sampler: failed to write estimate: {}",
                e
            );
        }

        // Enforce the disk quota by toggling read_only on the handle.
        let quota = handle.quota();
        if let Some(limit) = quota.disk_bytes {
            let exceeded = disk_bytes > limit;
            let was_read_only = handle.is_read_only();
            if exceeded != was_read_only {
                handle.set_read_only(exceeded);
                if exceeded {
                    warn!(
                        tenant = %handle.tenant_id,
                        disk_bytes,
                        limit,
                        "disk quota exceeded -- tenant set to read-only"
                    );
                } else {
                    tracing::info!(
                        tenant = %handle.tenant_id,
                        disk_bytes,
                        limit,
                        "disk quota cleared -- tenant read-only lifted"
                    );
                }
            }
        }

        // Mark dirty so quota_sync flushes the new disk estimate to the registry.
        handle.mark_dirty();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    /// du_bytes returns correct size for a known file.
    #[test]
    fn test_du_bytes_single_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.db");
        let mut f = std::fs::File::create(&file_path).unwrap();
        f.write_all(&[0u8; 1024]).unwrap();
        drop(f);

        let size = du_bytes(dir.path()).unwrap();
        assert_eq!(size, 1024, "du_bytes must return exact file size");
    }

    /// du_bytes returns 0 for an empty directory.
    #[test]
    fn test_du_bytes_empty_dir() {
        let dir = tempdir().unwrap();
        let size = du_bytes(dir.path()).unwrap();
        assert_eq!(size, 0);
    }

    /// du_bytes sums multiple files recursively.
    #[test]
    fn test_du_bytes_nested() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("a.bin"), vec![0u8; 512]).unwrap();
        std::fs::write(dir.path().join("sub/b.bin"), vec![0u8; 256]).unwrap();

        let size = du_bytes(dir.path()).unwrap();
        assert_eq!(size, 768, "du_bytes must sum all nested files");
    }
}
