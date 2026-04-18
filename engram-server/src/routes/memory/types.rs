use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct SearchBody {
    pub query: String,
    pub limit: Option<usize>,
    pub category: Option<String>,
    pub source: Option<String>,
    pub tags: Option<Vec<String>>,
    pub threshold: Option<f32>,
    pub tag: Option<String>,
    pub space_id: Option<i64>,
    pub include_forgotten: Option<bool>,
    pub mode: Option<String>,
    pub question_type: Option<engram_lib::memory::types::QuestionType>,
    pub expand_relationships: Option<bool>,
    pub include_links: Option<bool>,
    pub latest_only: Option<bool>,
    pub source_filter: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct RecallBody {
    pub context: Option<String>,
    pub query: Option<String>,
    pub limit: Option<usize>,
    pub space_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ListQuery {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub category: Option<String>,
    pub source: Option<String>,
    pub space_id: Option<i64>,
    pub include_forgotten: Option<bool>,
    pub include_archived: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub(super) struct TrashListOptions {
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SearchTagsBody {
    pub tags: Vec<String>,
    pub match_all: Option<bool>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct UpdateTagsBody {
    pub tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ForgetBody {
    pub reason: Option<String>,
}
