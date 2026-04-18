use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct ScratchQuery {
    pub agent: Option<String>,
    pub model: Option<String>,
    pub session: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct PromoteBody {
    pub keys: Option<Vec<String>>,
    pub combine: Option<bool>,
    pub category: Option<String>,
}
