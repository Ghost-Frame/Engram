use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct DecayScoresQuery {
    pub limit: Option<usize>,
    pub order: Option<String>,
}
