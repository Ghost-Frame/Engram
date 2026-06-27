use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct ObservationsQuery {
    pub limit: Option<usize>,
}

#[derive(Deserialize)]
pub(super) struct MaterializeBody {
    pub observation_id: i64,
}

#[derive(Deserialize)]
pub(super) struct ContextQuery {
    /// Keywords or current session topic to score observations against.
    pub q: Option<String>,
    /// Maximum number of scored observations to return (default 5, max 20).
    pub limit: Option<usize>,
}
