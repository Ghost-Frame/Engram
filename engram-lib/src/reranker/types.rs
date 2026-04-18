use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub(super) struct HttpRerankRequest<'a> {
    pub model: &'a str,
    pub query: &'a str,
    pub documents: Vec<&'a str>,
    pub top_n: usize,
}

#[derive(Deserialize)]
pub(super) struct HttpRerankResponse {
    pub results: Vec<HttpRerankResult>,
}

#[derive(Deserialize)]
pub(super) struct HttpRerankResult {
    pub index: usize,
    pub relevance_score: f64,
}
