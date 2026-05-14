use serde::Deserialize;

use kleos_lib::cred::ProxyRequest;

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct BootstrapBody {
    #[serde(default)]
    pub secret: Option<String>,
}

// M6: redact the bootstrap secret in any tracing/logging output. The
// derive(Debug) would have printed the literal secret if a tracing layer
// ever recorded the request body.
impl std::fmt::Debug for BootstrapBody {
    /// Format the body for tracing/logging output with the secret field
    /// replaced by a fixed redaction marker so the literal value never
    /// reaches a log sink.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BootstrapBody")
            .field(
                "secret",
                &match &self.secret {
                    Some(_) => "<redacted>",
                    None => "<none>",
                },
            )
            .finish()
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct GcBody {
    pub user_id: Option<i64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct ReembedBody {
    pub user_id: Option<i64>,
}

#[derive(Deserialize)]
pub(super) struct ColdStorageParams {
    #[serde(default = "default_cold_days")]
    pub days: i64,
}

/// serde default: cold-storage threshold in days when the body omits it.
fn default_cold_days() -> i64 {
    90
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct AdminCredResolveBody {
    pub text: Option<String>,
    pub service: Option<String>,
    pub key: Option<String>,
    pub raw: bool,
}

#[derive(Deserialize)]
pub(super) struct AdminCredProxyBody {
    pub service: String,
    pub key: String,
    pub request: ProxyRequest,
}

#[derive(Deserialize)]
pub(super) struct MaintenanceBody {
    pub enabled: bool,
    pub message: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct ProvisionBody {
    pub username: String,
    pub email: Option<String>,
    #[serde(default = "default_role")]
    pub role: String,
}

/// serde default: role label when the request body omits it.
fn default_role() -> String {
    "user".to_string()
}

#[derive(Deserialize)]
pub(super) struct DeprovisionBody {
    pub user_id: i64,
}

#[derive(Deserialize)]
pub(super) struct PitrPrepareBody {
    pub target: String,
    pub dest_path: String,
}

#[derive(Deserialize)]
pub(super) struct AdminPageRankQuery {
    pub user_id: Option<i64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct VectorSyncReplayBody {
    pub limit: Option<usize>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct VectorRebuildIndexBody {
    /// When true, drop any existing vector index before rebuilding.
    /// Defaults to false so repeated calls are cheap.
    pub replace: Option<bool>,
}

/// Body for POST /admin/migrations/down.
#[derive(Deserialize)]
pub(super) struct MigrateDownBody {
    pub target_version: u32,
    #[serde(default)]
    pub dry_run: bool,
}

/// Body for POST /admin/reset.
/// Must contain confirm: "WIPE_ALL_MEMORIES" to prevent accidental data loss.
#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct ResetBody {
    pub confirm: Option<String>,
}

/// Body for POST /admin/entities/backfill.
///
/// `batch_size` controls how many memories are processed per invocation
/// (default 100). `max_memories` caps the total across the whole request;
/// `None` means process all eligible memories up to `batch_size`.
#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct BackfillEntitiesBody {
    /// Number of memories to process in this call (default 100).
    #[serde(default = "default_backfill_batch_size")]
    pub batch_size: i64,
    /// Optional ceiling on total memories processed. None = unlimited.
    pub max_memories: Option<i64>,
}

/// serde default: per-invocation batch size for the entity backfill route.
fn default_backfill_batch_size() -> i64 {
    100
}
