use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct SyncQuery {
    pub since: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Deserialize)]
pub(super) struct SyncReceiveBody {
    pub changes: Vec<engram_lib::sync::SyncReceiveChange>,
}
