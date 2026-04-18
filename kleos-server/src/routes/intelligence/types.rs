use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct ConsolidateBody {
    pub memory_ids: Vec<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CandidatesBody {
    pub threshold: Option<f32>,
}

#[derive(Debug, Deserialize)]
pub(super) struct LimitQuery {
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct DigestBody {
    pub period: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CreateReflectionBody {
    pub content: String,
    pub reflection_type: Option<String>,
    pub source_memory_ids: Vec<i64>,
    pub confidence: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CreateChainBody {
    pub root_memory_id: Option<i64>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AddLinkBody {
    pub chain_id: i64,
    pub cause_memory_id: i64,
    pub effect_memory_id: i64,
    pub strength: Option<f64>,
    pub order_index: Option<i32>,
}

#[derive(Debug, Deserialize, Default)]
pub(super) struct BackwardBody {
    pub max_depth: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SentimentAnalyzeBody {
    pub content: Option<String>,
    pub memory_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SentimentHistoryQuery {
    pub limit: Option<i64>,
    pub since: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ValenceScoreBody {
    pub memory_id: Option<i64>,
    pub content: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SequencesBody {
    pub window_mins: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ReconsolidationCandidatesQuery {
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ExtractBody {
    pub content: Option<String>,
    pub memory_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct TimeTravelBody {
    pub query: Option<String>,
    pub timestamp: String,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SweepBody {
    pub threshold: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CorrectBody {
    pub memory_id: i64,
    pub content: String,
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct DuplicatesQuery {
    pub threshold: Option<f64>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct DeduplicateBody {
    pub threshold: Option<f64>,
    pub dry_run: Option<bool>,
}
