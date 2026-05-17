//! Request and query-parameter types for the soma route module.
//!
//! All types in this module are `pub(super)` -- they are implementation
//! details of the soma router and are not part of the public API surface.

use serde::Deserialize;

/// Body for `POST /soma/agents` -- creates or upserts an agent registration.
#[derive(Debug, Deserialize)]
pub(super) struct CreateAgentBody {
    pub name: String,
    #[serde(alias = "agent_type", alias = "category")]
    pub r#type: Option<String>,
    pub description: Option<String>,
    pub capabilities: Option<serde_json::Value>,
    pub config: Option<serde_json::Value>,
}

/// Body for `PATCH /soma/agents/{id}` -- partial update of agent fields.
/// Fields absent from the body are left unchanged.
#[derive(Debug, Deserialize)]
pub(super) struct UpdateAgentBody {
    pub status: Option<String>,
    #[serde(alias = "agent_type", alias = "category")]
    pub r#type: Option<String>,
    pub description: Option<String>,
    pub capabilities: Option<serde_json::Value>,
    pub config: Option<serde_json::Value>,
}

/// Query parameters for `GET /soma/agents` -- filters and pagination.
#[derive(Debug, Deserialize)]
pub(super) struct ListAgentsParams {
    #[serde(alias = "type")]
    pub agent_type: Option<String>,
    pub status: Option<String>,
    pub capability: Option<String>,
    pub limit: Option<usize>,
}

/// Body for `POST /soma/groups` -- creates a new agent group.
#[derive(Debug, Deserialize)]
pub(super) struct CreateGroupBody {
    pub name: String,
    pub description: Option<String>,
}

/// Body for `POST /soma/groups/{id}/members` -- adds an agent to a group.
#[derive(Debug, Deserialize)]
pub(super) struct AddMemberBody {
    pub agent_id: i64,
}

/// Body for `POST /soma/agents/{id}/log` -- appends a log entry to an agent.
#[derive(Debug, Deserialize)]
pub(super) struct LogEventBody {
    pub level: String,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

/// Query parameters for `GET /soma/agents/{id}/logs` -- log pagination and
/// optional level filter.
#[derive(Debug, Deserialize)]
pub(super) struct ListLogsParams {
    pub limit: Option<i64>,
    pub level: Option<String>,
}

/// Query parameters for `GET /soma/agents/stale` -- staleness window.
///
/// `minutes` is the age threshold in minutes. Absent values default to 5;
/// non-positive values clamp to 1 and values above 1440 clamp to 1440
/// (24 hours). Clamping is applied in the service layer; this struct carries
/// the raw optional value.
#[derive(Debug, Deserialize)]
pub(super) struct StaleAgentsParams {
    /// Staleness window in minutes. Defaults to 5 when absent.
    pub minutes: Option<i64>,
}

/// Body for `POST /soma/agents/{id}/heartbeat` -- optional status override.
///
/// Both fields are optional; an empty body is a valid heartbeat. When `status`
/// is present, it overrides the default `offline -> online` transition with
/// the supplied value (validated server-side against the allowed status set).
#[derive(Debug, Default, Deserialize)]
pub(super) struct HeartbeatBody {
    pub status: Option<String>,
}

/// Body for `PATCH /soma/agents/{id}/quality` -- update quality_score and/or
/// drift_flags. At least one of the two fields must be supplied. `drift_flags`
/// is stored as a JSON array of strings.
#[derive(Debug, Deserialize)]
pub(super) struct UpdateQualityBody {
    pub quality_score: Option<f64>,
    pub drift_flags: Option<serde_json::Value>,
}
