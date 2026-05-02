use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct ListSkillsParams {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub agent: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SearchSkillsBody {
    pub query: String,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct RecordExecutionBody {
    pub success: bool,
    pub duration_ms: Option<f64>,
    pub error_type: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct GetExecutionsParams {
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct JudgeBody {
    pub judge_agent: String,
    pub score: f64,
    pub rationale: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct RecordToolQualityBody {
    pub tool_name: String,
    pub agent: String,
    pub success: bool,
    pub latency_ms: Option<f64>,
    pub error_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct StatsParams {
    pub sort_by: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CaptureBody {
    pub description: String,
    pub agent: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct DeriveBody {
    pub parent_ids: Vec<i64>,
    pub direction: String,
    pub agent: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CloudSearchBody {
    pub query: String,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CloudUploadBody {
    pub name: String,
    pub description: String,
    pub content: String,
    pub category: String,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SyncSkillsBody {
    pub dirs: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ExecuteSkillsBody {
    pub task: String,
    #[allow(dead_code)]
    pub skill_dirs: Option<Vec<String>>,
    #[allow(dead_code)]
    pub search_scope: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct UploadSkillBody {
    pub skill_dir: String,
    #[allow(dead_code)]
    pub visibility: Option<String>,
    #[allow(dead_code)]
    pub origin: Option<String>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct EvolutionRecentParams {
    pub hours: Option<u32>,
    pub limit: Option<usize>,
}
