//! Deprovision teardown job handler.
//!
//! Registered under the `deprovision_teardown` job type. Payload must deserialize
//! to `DeprovisionJobPayload`. On success the registry row is Tombstoned. On
//! exhausted retries (5 attempts) the row is marked Stuck by the job framework.

use crate::db::Database;
use crate::tenant::registry_db::RegistryDb;
use crate::tenant::teardown::run_teardown_job;
use crate::EngError;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info};

/// Payload stored in the jobs table for a deprovision teardown job.
#[derive(Debug, Serialize, Deserialize)]
pub struct DeprovisionJobPayload {
    /// The unique deprovision operation ID.
    pub deprovision_id: String,
    /// The user being deprovisioned.
    pub user_id: i64,
    /// The registry tenant_id (shard path component).
    pub tenant_id: String,
}

/// Return the per-attempt timeout used by `deprovision_teardown` jobs.
pub(crate) fn job_timeout() -> Duration {
    let timeout_secs: u64 = std::env::var("KLEOS_DEPROVISION_JOB_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1800);
    Duration::from_secs(timeout_secs)
}

/// Register the `deprovision_teardown` job handler with the global job registry.
///
/// Requires the registry_db, monolith_db, and data_root at registration time.
/// These are captured by the closure and reused for every job execution.
///
/// Environment variables:
/// - `KLEOS_DEPROVISION_ARCHIVE`: Set to `true` or `1` to enable shard archiving.
/// - `KLEOS_DEPROVISION_JOB_TIMEOUT_SECS`: Per-attempt timeout (default 1800).
pub async fn register_handler(
    registry_db: Arc<RegistryDb>,
    monolith_db: Arc<Database>,
    data_root: PathBuf,
) {
    let archive = std::env::var("KLEOS_DEPROVISION_ARCHIVE")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    let timeout = job_timeout();
    let timeout_secs = timeout.as_secs();

    crate::jobs::register_job_handler("deprovision_teardown", move |payload_value| {
        let registry_db = Arc::clone(&registry_db);
        let monolith_db = Arc::clone(&monolith_db);
        let data_root = data_root.clone();

        async move {
            let payload: DeprovisionJobPayload = serde_json::from_value(payload_value)
                .map_err(|e| EngError::Internal(format!("invalid deprovision payload: {e}")))?;

            let result = tokio::time::timeout(
                timeout,
                run_teardown_job(
                    &registry_db,
                    &monolith_db,
                    &data_root,
                    &payload.deprovision_id,
                    payload.user_id,
                    &payload.tenant_id,
                    archive,
                ),
            )
            .await;

            match result {
                Ok(Ok(report)) => {
                    info!(
                        deprovision_id = %payload.deprovision_id,
                        steps = report.steps.len(),
                        "deprovision_teardown completed"
                    );
                    Ok(())
                }
                Ok(Err(e)) => {
                    error!(
                        deprovision_id = %payload.deprovision_id,
                        "deprovision_teardown step failed: {e}"
                    );
                    Err(e)
                }
                Err(_elapsed) => {
                    error!(
                        deprovision_id = %payload.deprovision_id,
                        timeout_secs,
                        "deprovision_teardown timed out"
                    );
                    Err(EngError::Internal(format!(
                        "deprovision job timed out after {timeout_secs}s"
                    )))
                }
            }
        }
    })
    .await;
}

/// Unit tests for deprovision job timeout configuration.
#[cfg(test)]
mod tests {
    use super::*;

    /// Verify the default deprovision timeout exceeds the generic job cap.
    #[serial_test::serial(deprovision_timeout_env)]
    #[test]
    fn job_timeout_default_is_longer_than_generic_cap() {
        std::env::remove_var("KLEOS_DEPROVISION_JOB_TIMEOUT_SECS");
        assert!(job_timeout().as_secs() >= 1800);
    }
}
