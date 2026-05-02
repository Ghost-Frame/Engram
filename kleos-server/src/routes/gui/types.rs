use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct LoginForm {
    pub api_key: String,
}

#[derive(Deserialize)]
pub(super) struct CreateMemoryBody {
    pub content: String,
    pub category: Option<String>,
    pub importance: Option<i32>,
    pub tags: Option<Vec<String>>,
    pub is_static: Option<bool>,
}

#[derive(Deserialize)]
pub(super) struct UpdateMemoryBody {
    pub content: Option<String>,
    pub category: Option<String>,
    pub importance: Option<i32>,
    pub is_static: Option<bool>,
}

#[derive(Deserialize)]
pub(super) struct BulkArchiveBody {
    pub ids: Vec<i64>,
}
