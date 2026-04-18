use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct DetectBody {
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct StoreSignalBody {
    pub signal_type: String,
    pub value: f64,
    pub evidence: Option<String>,
    pub agent: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ListSignalsParams {
    pub limit: Option<usize>,
}
