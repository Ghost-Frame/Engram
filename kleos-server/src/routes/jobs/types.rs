use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct ListJobsQuery {
    pub status: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct RetryBody {
    pub id: i64,
}

#[derive(Debug, Deserialize)]
pub(super) struct PurgeBody {
    pub older_than_days: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct PaginationQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CleanupBody {
    pub older_than_days: Option<i64>,
}
