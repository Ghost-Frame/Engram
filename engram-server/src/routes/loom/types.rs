use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct ListRunsParams {
    pub workflow_id: Option<i64>,
    pub status: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct GetLogsParams {
    pub step_id: Option<i64>,
    pub level: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CompleteStepBody {
    pub output: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub(super) struct FailStepBody {
    pub error: String,
}
