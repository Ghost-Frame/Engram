use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct AuditQueryParams {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}
