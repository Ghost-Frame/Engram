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
