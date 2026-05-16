use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct ListEvaluationsParams {
    pub agent: Option<String>,
    pub rubric_id: Option<i64>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct GetMetricsParams {
    pub agent: Option<String>,
    pub metric: Option<String>,
    pub since: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MetricSummaryParams {
    pub agent: Option<String>,
    pub metric: Option<String>,
    pub since: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SessionQualityParams {
    pub agent: Option<String>,
    pub since: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct DriftEventsParams {
    pub agent: Option<String>,
    pub limit: Option<usize>,
}

/// Query parameters for `GET /thymus/drift-summary`. `agent` is required;
/// the handler returns 400 when absent.
#[derive(Debug, Deserialize)]
pub(super) struct DriftSummaryParams {
    pub agent: Option<String>,
}

/// Query parameters for the agent-scores aggregate endpoint.
///
/// Both fields are optional. `rubric_id` restricts the aggregate to one rubric;
/// `since` is an ISO 8601 lower bound on `created_at`.
#[derive(Debug, Deserialize)]
pub(super) struct AgentScoresParams {
    /// When present, limits the aggregate to evaluations from this rubric.
    pub rubric_id: Option<i64>,
    /// When present, limits the aggregate to evaluations on or after this
    /// ISO 8601 timestamp. Passed verbatim to SQLite as a string comparison.
    pub since: Option<String>,
}
