use serde::{Deserialize, Serialize};

// -- Enums ---

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryCategory {
    Task,
    Discovery,
    Decision,
    State,
    Issue,
    General,
    Reference,
}

impl Default for MemoryCategory {
    fn default() -> Self {
        Self::General
    }
}

impl std::fmt::Display for MemoryCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Task => write!(f, "task"),
            Self::Discovery => write!(f, "discovery"),
            Self::Decision => write!(f, "decision"),
            Self::State => write!(f, "state"),
            Self::Issue => write!(f, "issue"),
            Self::General => write!(f, "general"),
            Self::Reference => write!(f, "reference"),
        }
    }
}

impl std::str::FromStr for MemoryCategory {
    type Err = crate::EngError;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "task" => Ok(Self::Task),
            "discovery" => Ok(Self::Discovery),
            "decision" => Ok(Self::Decision),
            "state" => Ok(Self::State),
            "issue" => Ok(Self::Issue),
            "general" => Ok(Self::General),
            "reference" => Ok(Self::Reference),
            _ => Err(crate::EngError::InvalidInput(format!(
                "unknown category: {}",
                s
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryStatus {
    Approved,
    Pending,
}

impl Default for MemoryStatus {
    fn default() -> Self {
        Self::Approved
    }
}

impl std::fmt::Display for MemoryStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Approved => write!(f, "approved"),
            Self::Pending => write!(f, "pending"),
        }
    }
}

impl std::str::FromStr for MemoryStatus {
    type Err = crate::EngError;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "approved" => Ok(Self::Approved),
            "pending" => Ok(Self::Pending),
            _ => Err(crate::EngError::InvalidInput(format!(
                "unknown status: {}",
                s
            ))),
        }
    }
}

// -- Core memory struct ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: i64,
    pub content: String,
    pub category: String,          // stored as text in DB
    pub source: String,
    pub session_id: Option<String>,
    pub importance: i32,           // 1-10, default 5
    pub embedding: Option<Vec<f32>>,
    pub version: i32,
    pub is_latest: bool,
    pub parent_memory_id: Option<i64>,
    pub root_memory_id: Option<i64>,
    pub source_count: i32,
    pub is_static: bool,
    pub is_forgotten: bool,
    pub is_archived: bool,
    pub is_inference: bool,
    pub is_fact: bool,
    pub is_decomposed: bool,
    pub forget_after: Option<String>,
    pub forget_reason: Option<String>,
    pub model: Option<String>,
    pub recall_hits: i32,
    pub recall_misses: i32,
    pub adaptive_score: Option<f64>,
    pub pagerank_score: Option<f64>,
    pub last_accessed_at: Option<String>,
    pub access_count: i32,
    pub tags: Option<String>,      // JSON array string
    pub episode_id: Option<i64>,
    pub decay_score: Option<f64>,
    pub confidence: f64,
    pub sync_id: Option<String>,
    pub status: String,            // "approved" or "pending"
    pub user_id: i64,
    pub space_id: Option<i64>,
    // FSRS columns
    pub fsrs_stability: Option<f64>,
    pub fsrs_difficulty: Option<f64>,
    pub fsrs_storage_strength: Option<f64>,
    pub fsrs_retrieval_strength: Option<f64>,
    pub fsrs_learning_state: Option<i32>,
    pub fsrs_reps: Option<i32>,
    pub fsrs_lapses: Option<i32>,
    pub fsrs_last_review_at: Option<String>,
    // Emotional
    pub valence: Option<f64>,
    pub arousal: Option<f64>,
    pub dominant_emotion: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

// -- Request/Response types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreRequest {
    pub content: String,
    #[serde(default = "default_category")]
    pub category: String,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default = "default_importance")]
    pub importance: i32,
    pub tags: Option<Vec<String>>,
    pub embedding: Option<Vec<f32>>,
    pub session_id: Option<String>,
    pub is_static: Option<bool>,
    pub user_id: Option<i64>,
    pub space_id: Option<i64>,
    pub parent_memory_id: Option<i64>,
}

fn default_category() -> String {
    "general".to_string()
}
fn default_source() -> String {
    "unknown".to_string()
}
fn default_importance() -> i32 {
    5
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreResult {
    pub id: i64,
    pub created: bool,
    pub duplicate_of: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    pub embedding: Option<Vec<f32>>,
    pub limit: Option<usize>,
    pub category: Option<String>,
    pub source: Option<String>,
    pub tags: Option<Vec<String>>,
    pub threshold: Option<f32>,
    pub user_id: Option<i64>,
    pub space_id: Option<i64>,
    pub include_forgotten: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub memory: Memory,
    pub score: f64,
    pub search_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListOptions {
    pub limit: usize,
    pub offset: usize,
    pub category: Option<String>,
    pub source: Option<String>,
    pub user_id: Option<i64>,
    pub space_id: Option<i64>,
    pub include_forgotten: bool,
    pub include_archived: bool,
}

impl Default for ListOptions {
    fn default() -> Self {
        Self {
            limit: 50,
            offset: 0,
            category: None,
            source: None,
            user_id: None,
            space_id: None,
            include_forgotten: false,
            include_archived: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateRequest {
    pub content: Option<String>,
    pub category: Option<String>,
    pub importance: Option<i32>,
    pub tags: Option<Vec<String>>,
    pub is_static: Option<bool>,
    pub status: Option<String>,
    pub embedding: Option<Vec<f32>>,
}
