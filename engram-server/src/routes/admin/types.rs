use serde::Deserialize;

use engram_lib::cred::ProxyRequest;

#[derive(Debug, Deserialize, Default)]
pub(super) struct BootstrapBody {
    #[serde(default)]
    pub secret: Option<String>,
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
