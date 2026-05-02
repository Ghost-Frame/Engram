use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct FetchBody {
    pub url: String,
    pub cache: Option<bool>,
}
