use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct ListEpisodesParams {
    pub limit: Option<usize>,
    pub query: Option<String>,
    pub after: Option<String>,
    pub before: Option<String>,
}
