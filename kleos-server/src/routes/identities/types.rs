use serde::Deserialize;

#[derive(Deserialize)]
pub struct ListIdentitiesParams {
    pub host: Option<String>,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub key_id: Option<i64>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Deserialize)]
pub struct AuditParams {
    pub since: Option<String>,
    pub limit: Option<usize>,
}
