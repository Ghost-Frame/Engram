use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct ReviewBody {
    pub id: Option<i64>,
    pub memory_id: Option<i64>,
    pub grade: Option<u8>,
}

#[derive(Debug, Deserialize)]
pub(super) struct StateQuery {
    pub id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct RecallDueQuery {
    pub topic: String,
    #[serde(default = "default_recall_limit")]
    pub limit: usize,
    pub session: Option<String>,
}

fn default_recall_limit() -> usize {
    5
}
