use serde::Deserialize;

/// Query parameters for the `GET /scratch` (list) endpoint.
#[derive(Deserialize)]
pub(super) struct ScratchQuery {
    pub agent: Option<String>,
    pub model: Option<String>,
    pub session: Option<String>,
}

/// Query parameters for the `GET /scratchpad/get` endpoint used by the `ke`
/// edit-gate: `namespace` maps to the `agent` column; `key` is the `entry_key`.
#[derive(Deserialize)]
pub(super) struct ScratchGetQuery {
    /// Corresponds to the `agent` column -- "spec-task" for forge ledger entries.
    pub namespace: String,
    /// The `entry_key` value: `<session_id>:<absolute_path>` as built by `ke`.
    pub key: String,
}

/// Request body for the `POST /scratch/{session}/promote` endpoint.
#[derive(Deserialize)]
pub(super) struct PromoteBody {
    pub keys: Option<Vec<String>>,
    pub combine: Option<bool>,
    pub category: Option<String>,
}
