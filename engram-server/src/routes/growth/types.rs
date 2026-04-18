use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct ObservationsQuery {
    pub limit: Option<usize>,
}

#[derive(Deserialize)]
pub(super) struct MaterializeBody {
    pub observation_id: i64,
}
