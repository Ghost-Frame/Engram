use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct StatusQuery {
    pub status: Option<String>,
}
