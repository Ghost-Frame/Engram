use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct ListTasksParams {
    pub agent: Option<String>,
    pub project: Option<String>,
    pub status: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CreateTaskBody {
    pub title: String,
    pub agent: String,
    pub project: String,
    pub status: Option<String>,
    pub summary: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct UpdateTaskBody {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub status: Option<String>,
    pub agent: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct HistoryParams {
    pub limit: Option<usize>,
}
