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
