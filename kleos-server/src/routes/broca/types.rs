//! Request/response types for the broca routes.

use serde::Deserialize;

/// Request body for `POST /broca/actions` (authenticated action logging).
#[derive(Debug, Deserialize)]
pub(super) struct LogActionBody {
    /// Identifier of the agent performing the action.
    pub agent: String,
    /// Service name (defaults to `"kleos"` when absent).
    pub service: Option<String>,
    /// Action type string (e.g., `"task.started"`).
    pub action: Option<String>,
    /// Human-readable summary; aliased by `narrative` and `detail`.
    pub summary: Option<String>,
    /// Human-readable detail; falls through to `summary`.
    pub detail: Option<String>,
    /// Pre-computed human-readable narrative; takes precedence over `summary`
    /// and `detail`. When absent, `log_action` auto-generates one via the
    /// template narrator if a matching template exists.
    pub narrative: Option<String>,
    /// Project label to embed in the payload.
    pub project: Option<String>,
    /// Structured event payload.
    pub payload: Option<serde_json::Value>,
    /// Alias for `payload` accepted for backwards compatibility.
    pub metadata: Option<serde_json::Value>,
    /// Upstream Axon event id, set when this action was created by an ingest
    /// webhook rather than a direct API call.
    pub axon_event_id: Option<i64>,
}

/// Query parameters for `GET /broca/actions` and `GET /broca/feed`.
#[derive(Debug, Deserialize)]
pub(super) struct QueryActionsParams {
    /// Filter to actions by this agent identifier.
    pub agent: Option<String>,
    /// Filter to actions from this service name.
    pub service: Option<String>,
    /// Filter to this action type.
    pub action: Option<String>,
    /// Return only actions at or after this ISO-8601 timestamp.
    pub since: Option<String>,
    /// Maximum rows to return (capped at 1000; defaults to 100).
    pub limit: Option<usize>,
    /// Row offset for pagination.
    pub offset: Option<usize>,
}

/// Request body for `POST /broca/narrate` (bulk LLM narration).
#[derive(Debug, serde::Deserialize)]
pub(super) struct NarrateBatchBody {
    /// Action ids to narrate. Must be non-empty and at most 50 elements.
    pub ids: Vec<i64>,
}

/// Request body for `POST /broca/ask` (natural-language query).
///
/// The `question` field is validated by the handler: it must be non-empty and
/// at most 2 000 Unicode characters long.
#[derive(Debug, Deserialize)]
pub(super) struct AskBody {
    /// Natural-language question to answer from the action log.
    pub question: String,
}

/// Inbound Axon webhook payload for `POST /broca/ingest`.
///
/// The endpoint that receives it is intentionally unauthenticated -- protect
/// at the network layer if needed. `source` and `event_type` are required;
/// everything else is optional.
#[derive(Debug, Deserialize)]
pub(super) struct IngestBody {
    /// Axon event id from the upstream event store; stored as `axon_event_id`.
    pub id: Option<i64>,
    /// Axon channel the event was published on (e.g., `"memory"`).
    pub channel: Option<String>,
    /// Service or agent that produced the event.
    pub source: String,
    /// Action type string (e.g., `"memory.store"`).
    #[serde(rename = "type")]
    pub event_type: String,
    /// Structured event payload forwarded from Axon.
    pub payload: Option<serde_json::Value>,
}
