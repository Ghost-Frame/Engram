use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct PackBody {
    pub context: Option<String>,
    pub token_budget: Option<usize>,
    pub format: Option<String>,
}
