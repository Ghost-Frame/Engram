use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct ListSessionsParams {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Deserialize)]
pub(super) struct AppendBody {
    pub line: String,
}
