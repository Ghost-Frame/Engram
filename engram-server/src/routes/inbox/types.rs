use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct PagingQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Deserialize)]
pub(super) struct RejectBody {
    pub reason: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct EditBody {
    pub content: Option<String>,
    pub category: Option<String>,
    pub importance: Option<i64>,
    pub tags: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct BulkBody {
    pub ids: Vec<i64>,
    pub action: String,
}
