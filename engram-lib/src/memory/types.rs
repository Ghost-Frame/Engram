use serde::{Deserialize, Serialize};

pub const VALID_FEEDBACK_SIGNALS: &[&str] = &["used", "ignored", "corrected", "irrelevant", "helpful"];
pub const DEFAULT_IMPORTANCE: i32 = 5;
pub const MAX_CONTENT_SIZE: usize = 102_400;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionType { #[default] FactRecall, Preference, Reasoning, Generalization, Temporal }
impl std::fmt::Display for QuestionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::FactRecall => "fact_recall", Self::Preference => "preference",
            Self::Reasoning => "reasoning", Self::Generalization => "generalization",
            Self::Temporal => "temporal",
        };
        write!(f, "{}", s)
    }
}
impl std::str::FromStr for QuestionType {
    type Err = crate::EngError;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "fact_recall" => Ok(Self::FactRecall), "preference" => Ok(Self::Preference),
            "reasoning" => Ok(Self::Reasoning), "generalization" => Ok(Self::Generalization),
            "temporal" => Ok(Self::Temporal),
            other => Err(crate::EngError::InvalidInput(["unknown question type: ", other].concat())),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryCategory { Task, Discovery, Decision, State, Issue, #[default] General, Reference }
impl std::fmt::Display for MemoryCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Task => "task", Self::Discovery => "discovery", Self::Decision => "decision",
            Self::State => "state", Self::Issue => "issue", Self::General => "general",
            Self::Reference => "reference",
        };
        write!(f, "{}", s)
    }
}
impl std::str::FromStr for MemoryCategory {
    type Err = crate::EngError;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "task" => Ok(Self::Task), "discovery" => Ok(Self::Discovery),
            "decision" => Ok(Self::Decision), "state" => Ok(Self::State),
            "issue" => Ok(Self::Issue), "general" => Ok(Self::General),
            "reference" => Ok(Self::Reference),
            other => Err(crate::EngError::InvalidInput(["unknown category: ", other].concat())),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryStatus { #[default] Approved, Pending }
impl std::fmt::Display for MemoryStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self { Self::Approved => write!(f, "approved"), Self::Pending => write!(f, "pending") }
    }
}
impl std::str::FromStr for MemoryStatus {
    type Err = crate::EngError;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "approved" => Ok(Self::Approved), "pending" => Ok(Self::Pending),
            other => Err(crate::EngError::InvalidInput(["unknown status: ", other].concat())),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: i64, pub content: String, pub category: String, pub source: String,
    pub session_id: Option<String>, pub importance: i32, pub embedding: Option<Vec<f32>>,
    pub version: i32, pub is_latest: bool, pub parent_memory_id: Option<i64>,
    pub root_memory_id: Option<i64>, pub source_count: i32, pub is_static: bool,
    pub is_forgotten: bool, pub is_archived: bool, pub is_inference: bool,
    pub is_fact: bool, pub is_decomposed: bool, pub forget_after: Option<String>,
    pub forget_reason: Option<String>, pub model: Option<String>,
    pub recall_hits: i32, pub recall_misses: i32, pub adaptive_score: Option<f64>,
    pub pagerank_score: Option<f64>, pub last_accessed_at: Option<String>,
    pub access_count: i32, pub tags: Option<String>, pub episode_id: Option<i64>,
    pub decay_score: Option<f64>, pub confidence: f64, pub sync_id: Option<String>,
    pub status: String, pub user_id: i64, pub space_id: Option<i64>,
    pub fsrs_stability: Option<f64>, pub fsrs_difficulty: Option<f64>,
    pub fsrs_storage_strength: Option<f64>, pub fsrs_retrieval_strength: Option<f64>,
    pub fsrs_learning_state: Option<i32>, pub fsrs_reps: Option<i32>,
    pub fsrs_lapses: Option<i32>, pub fsrs_last_review_at: Option<String>,
    pub valence: Option<f64>, pub arousal: Option<f64>,
    pub dominant_emotion: Option<String>, pub created_at: String, pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct SearchStrategy {
    pub vector_floor: f64, pub vector_weight: f64, pub fts_weight: f64,
    pub candidate_multiplier: usize, pub fts_limit_multiplier: usize,
    pub expand_relationships: bool, pub relationship_seed_limit: usize,
    pub hop1_limit: usize, pub hop2_limit: usize, pub relationship_multiplier: f64,
    pub include_personality_signals: bool, pub personality_limit: usize,
    pub personality_weight: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HybridSearchOptions {
    pub vector_floor: Option<f64>, pub question_type: Option<QuestionType>,
    pub expand_relationships: Option<bool>, pub include_personality_signals: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalDiagnostics {
    pub question_type: QuestionType, pub reranked: bool,
    pub reranker_ms: f64, pub candidate_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkedMemory {
    pub id: i64, pub content: String, pub category: String, pub similarity: f64,
    #[serde(rename = "type")]
    pub link_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionChainEntry {
    pub id: i64, pub content: String, pub version: i32, pub is_latest: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreRequest {
    pub content: String,
    #[serde(default = "default_category")]
    pub category: String,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default = "default_importance")]
    pub importance: i32,
    pub tags: Option<Vec<String>>, pub embedding: Option<Vec<f32>>,
    pub session_id: Option<String>, pub is_static: Option<bool>,
    pub user_id: Option<i64>, pub space_id: Option<i64>,
    pub parent_memory_id: Option<i64>,
}
fn default_category() -> String { "general".to_string() }
fn default_source() -> String { "unknown".to_string() }
fn default_importance() -> i32 { 5 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreResult { pub id: i64, pub created: bool, pub duplicate_of: Option<i64> }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String, pub embedding: Option<Vec<f32>>, pub limit: Option<usize>,
    pub category: Option<String>, pub source: Option<String>,
    pub tags: Option<Vec<String>>, pub threshold: Option<f32>,
    pub user_id: Option<i64>, pub space_id: Option<i64>,
    pub include_forgotten: Option<bool>, pub mode: Option<String>,
    pub question_type: Option<QuestionType>,
    #[serde(default)]
    pub expand_relationships: bool,
    #[serde(default)]
    pub include_links: bool,
    #[serde(default = "default_true")]
    pub latest_only: bool,
    pub source_filter: Option<String>,
}
fn default_true() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub memory: Memory, pub score: f64, pub search_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decay_score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub combined_score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantic_score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fts_score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graph_score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub personality_signal_score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temporal_boost: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub question_type: Option<QuestionType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reranked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reranker_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linked: Option<Vec<LinkedMemory>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_chain: Option<Vec<VersionChainEntry>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListOptions {
    pub limit: usize, pub offset: usize, pub category: Option<String>,
    pub source: Option<String>, pub user_id: Option<i64>, pub space_id: Option<i64>,
    pub include_forgotten: bool, pub include_archived: bool,
}
impl Default for ListOptions {
    fn default() -> Self {
        Self { limit: 50, offset: 0, category: None, source: None,
               user_id: None, space_id: None, include_forgotten: false, include_archived: false }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateRequest {
    pub content: Option<String>, pub category: Option<String>,
    pub importance: Option<i32>, pub tags: Option<Vec<String>>,
    pub is_static: Option<bool>, pub status: Option<String>,
    pub embedding: Option<Vec<f32>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackItem {
    pub query: String, pub memory_id: i64, pub signal: String,
    pub context: Option<String>, pub agent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrectOptions {
    pub correction: String, pub original_claim: Option<String>,
    pub memory_id: Option<i64>, pub category: Option<String>,
    pub source: Option<String>, pub importance: Option<i32>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryHealthParams { pub stale_days: i64, pub dup_threshold: f64, pub limit: usize }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeduplicateOptions { pub threshold: f64, pub dry_run: bool, pub max_merge: usize }
