// ============================================================================
// CONTEXT DOMAIN -- Type definitions
// ============================================================================

use serde::{Deserialize, Serialize};

// -- Constants ---------------------------------------------------------------

/// Default token budget when none supplied.
pub const DEFAULT_TOKEN_BUDGET: usize = 8000;

/// Absolute cap on token budget.
pub const MAX_TOKEN_BUDGET: usize = 64000;

/// Default max tokens per individual memory block.
pub const DEFAULT_MAX_MEMORY_TOKENS: usize = 2500;

/// Default cosine similarity deduplication threshold.
pub const DEFAULT_DEDUP_THRESHOLD: f64 = 0.88;

/// Default minimum relevance score for semantic results.
pub const DEFAULT_MIN_RELEVANCE: f64 = 0.55;

/// Default semantic ceiling (fraction of total budget) per strategy.
pub const DEFAULT_SEMANTIC_CEILING_BALANCED: f64 = 0.80;
pub const DEFAULT_SEMANTIC_CEILING_PRECISION: f64 = 0.82;
pub const DEFAULT_SEMANTIC_CEILING_BREADTH: f64 = 0.90;

/// Recency boost window: memories within this age (ms) get +10% score.
pub const RECENCY_BOOST_MS: i64 = 48 * 60 * 60 * 1000;

/// Static fact budget fractions per strategy.
pub const STATIC_BUDGET_BALANCED: f64 = 0.3;
pub const STATIC_BUDGET_PRECISION: f64 = 0.2;

// -- Enums -------------------------------------------------------------------

/// Context strategy modes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextStrategy {
    #[default]
    Balanced,
    Precision,
    Breadth,
}

/// Context mode presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextMode {
    Fast,
    Balanced,
    Deep,
    Decision,
}

/// Source of a context block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextBlockSource {
    Static,
    Semantic,
    Evolution,
    Episode,
    Linked,
    Recent,
    Inference,
    WorkingMemory,
}

// -- Structs -----------------------------------------------------------------

/// A single assembled context block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextBlock {
    pub id: i64,
    pub content: String,
    pub category: String,
    pub score: f64,
    pub source: ContextBlockSource,
    pub tokens: usize,
    pub created_at: Option<String>,
    pub model: Option<String>,
    pub origin: Option<String>,
    pub parent_id: Option<i64>,
}

/// Summary of a context block in the result (no content field).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextBlockSummary {
    pub id: i64,
    pub category: String,
    pub source: ContextBlockSource,
    pub model: Option<String>,
    pub origin: Option<String>,
    pub score: f64,
    pub tokens: usize,
}

/// Per-source layer counts in the breakdown.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextBreakdown {
    #[serde(rename = "static")]
    pub static_count: usize,
    pub semantic: usize,
    pub evolution: usize,
    pub episode: usize,
    pub linked: usize,
    pub recent: usize,
    pub inference: usize,
    pub personality: usize,
}

/// Timing info per phase (milliseconds).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextTiming {
    pub embed_ms: Option<u64>,
    pub static_ms: Option<u64>,
    pub search_ms: Option<u64>,
    pub rerank_ms: Option<u64>,
    pub semantic_ms: Option<u64>,
    pub evolution_ms: Option<u64>,
    pub episodes_ms: Option<u64>,
    pub linked_ms: Option<u64>,
    pub recent_ms: Option<u64>,
    pub inference_ms: Option<u64>,
    pub assembly_ms: Option<u64>,
    pub total_ms: Option<u64>,
}

/// Input options for context assembly.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextOptions {
    pub query: String,
    pub max_tokens: Option<usize>,
    pub token_budget: Option<usize>,
    pub budget: Option<usize>,
    pub strategy: Option<ContextStrategy>,
    pub depth: Option<u8>,
    pub mode: Option<ContextMode>,
    pub include_static: Option<bool>,
    pub include_recent: Option<bool>,
    pub include_episodes: Option<bool>,
    pub include_linked: Option<bool>,
    pub include_inference: Option<bool>,
    pub include_current_state: Option<bool>,
    pub include_preferences: Option<bool>,
    pub include_structured_facts: Option<bool>,
    pub include_working_memory: Option<bool>,
    pub max_memory_tokens: Option<usize>,
    pub dedup_threshold: Option<f64>,
    pub min_relevance: Option<f64>,
    pub semantic_ceiling: Option<f64>,
    pub semantic_limit: Option<usize>,
    pub source: Option<String>,
    pub session: Option<String>,
}

/// The assembled context result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextResult {
    pub context: String,
    pub blocks: Vec<ContextBlockSummary>,
    pub token_estimate: usize,
    pub token_budget: usize,
    pub utilization: f64,
    pub strategy: ContextStrategy,
    pub breakdown: ContextBreakdown,
    pub timing: ContextTiming,
}

/// A supplementary section (working memory, current state, personality, etc.)
#[derive(Debug, Clone)]
pub struct SupplementarySection {
    pub label: String,
    pub content: String,
}

/// Layer enable flags resolved from options and depth.
#[derive(Debug, Clone, Copy)]
pub struct LayerFlags {
    pub include_static: bool,
    pub include_recent: bool,
    pub include_episodes: bool,
    pub include_linked: bool,
    pub include_inference: bool,
    pub include_current_state: bool,
    pub include_preferences: bool,
    pub include_structured_facts: bool,
    pub include_working_memory: bool,
    pub include_personality: bool,
}
