use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct RespondBody {
    pub gate_id: i64,
    pub approved: bool,
    pub reason: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct CompleteBody {
    pub gate_id: i64,
    pub output: String,
    #[serde(default)]
    pub known_secrets: Vec<String>,
}

#[derive(Deserialize)]
pub(super) struct GuardBody {
    pub action: String,
}
