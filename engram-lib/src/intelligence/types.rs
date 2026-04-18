//! Intelligence domain -- shared type definitions.
//! Ported from intelligence/types.ts.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Reflection types
// ---------------------------------------------------------------------------

/// Valid reflection period values.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReflectionPeriod {
    Day,
    Week,
    Month,
}

impl std::fmt::Display for ReflectionPeriod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Day => write!(f, "day"),
            Self::Week => write!(f, "week"),
            Self::Month => write!(f, "month"),
        }
    }
}

// ---------------------------------------------------------------------------
// Contradiction resolution
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContradictionResolution {
    KeepA,
    KeepB,
    KeepBoth,
    Merge,
}

// ---------------------------------------------------------------------------
// Decomposition types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecompositionResult {
    pub facts: Vec<String>,
    pub skip: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DecompositionTier {
    Llm,
    #[serde(rename = "tier2-rules")]
    Tier2Rules,
    #[serde(rename = "tier3-template")]
    Tier3Template,
}

impl std::fmt::Display for DecompositionTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Llm => write!(f, "llm"),
            Self::Tier2Rules => write!(f, "tier2-rules"),
            Self::Tier3Template => write!(f, "tier3-template"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecompositionWithTier {
    pub result: DecompositionResult,
    pub tier: DecompositionTier,
}

// ---------------------------------------------------------------------------
// Fact store metadata
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct FactStoreMeta {
    pub category: String,
    pub source: String,
    pub user_id: i64,
    pub space_id: Option<i64>,
    pub importance: i32,
    pub episode_id: Option<i64>,
    pub tags: Option<String>,
    pub session_id: Option<String>,
    pub model: Option<String>,
}

// ---------------------------------------------------------------------------
// Valence types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValenceResult {
    pub valence: f64,
    pub arousal: f64,
    pub dominant_emotion: String,
    pub all_emotions: Vec<EmotionMatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmotionMatch {
    pub emotion: String,
    pub valence: f64,
    pub arousal: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmotionMemory {
    pub id: i64,
    pub content: String,
    pub category: String,
    pub importance: i32,
    pub valence: f64,
    pub arousal: f64,
    pub dominant_emotion: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmotionalProfile {
    pub emotions: Vec<EmotionStat>,
    pub overall: OverallEmotionStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmotionStat {
    pub dominant_emotion: String,
    pub count: i64,
    pub avg_valence: f64,
    pub avg_arousal: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverallEmotionStats {
    pub avg_valence: f64,
    pub avg_arousal: f64,
    pub positive_count: i64,
    pub negative_count: i64,
    pub neutral_count: i64,
}

// ---------------------------------------------------------------------------
// Predictive types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PredictiveContext {
    pub time_context: String,
    pub predicted_categories: Vec<String>,
    pub predicted_project: Option<PredictedProject>,
    pub proactive_memories: Vec<ProactiveMemory>,
    pub suggested_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PredictedProject {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProactiveMemory {
    pub id: i64,
    pub content: String,
    pub category: String,
    pub importance: i32,
    pub reason: String,
    pub score: f64,
}

/// A mined sequence pattern: `antecedent -> consequent` observed `support`
/// times across the user's memory timeline. `confidence` is
/// `support / antecedent_total`, i.e. P(consequent | antecedent).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SequencePattern {
    pub antecedent: String,
    pub consequent: String,
    pub support: i64,
    pub confidence: f64,
}

// ---------------------------------------------------------------------------
// Reconsolidation types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconsolidationResult {
    pub memory_id: i64,
    pub action: ReconsolidationAction,
    pub old_importance: i32,
    pub new_importance: i32,
    pub old_confidence: f64,
    pub new_confidence: f64,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReconsolidationAction {
    Strengthened,
    Weakened,
    Corrected,
    Unchanged,
}

// ---------------------------------------------------------------------------
// Growth types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrowthReflectRequest {
    pub service: String,
    pub context: Vec<String>,
    pub existing_growth: Option<String>,
    pub prompt_override: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrowthReflectResult {
    pub observation: Option<String>,
    pub stored_memory_id: Option<i64>,
    pub reflection_id: Option<i64>,
}

// ---------------------------------------------------------------------------
// Extraction stats
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExtractionStats {
    pub facts: i32,
    pub preferences: i32,
    pub state_updates: i32,
}

// ---------------------------------------------------------------------------
// Consolidation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ConsolidationRecord {
    pub id: i64,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SweepResult {
    pub pairs_found: i64,
    pub consolidated: i64,
}

// ---------------------------------------------------------------------------
// Duplicates
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicatePair {
    pub id_a: i64,
    pub id_b: i64,
    pub content_a: String,
    pub content_b: String,
    pub similarity: f64,
    pub importance_a: i32,
    pub importance_b: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeduplicateResult {
    pub pairs_found: i64,
    pub merged: i64,
    pub dry_run: bool,
}

// ---------------------------------------------------------------------------
// Temporal
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemporalPattern {
    pub label: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeTravelResult {
    pub id: i64,
    pub content: String,
    pub category: String,
    pub importance: i32,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Fact contradiction
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactContradiction {
    pub new_fact_id: i64,
    pub old_fact_id: i64,
    pub old_memory_id: i64,
    pub subject: String,
    pub verb: String,
    pub new_object: Option<String>,
    pub old_object: Option<String>,
}

// ---------------------------------------------------------------------------
// LLM options
// ---------------------------------------------------------------------------

/// Options for LLM calls.
#[derive(Debug, Clone)]
pub struct LlmOptions {
    pub temperature: f64,
    pub max_tokens: u32,
}

impl Default for LlmOptions {
    fn default() -> Self {
        Self {
            temperature: 0.3,
            max_tokens: 1024,
        }
    }
}

// ---------------------------------------------------------------------------
// Scheduler reports
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Ok,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskReport {
    pub name: String,
    pub status: TaskStatus,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PipelineReport {
    pub reports: Vec<TaskReport>,
    pub total_duration_ms: u64,
    pub ok_count: usize,
    pub failed_count: usize,
    pub skipped_count: usize,
}

// ---------------------------------------------------------------------------
// Causal chains
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CausalChain {
    pub id: i64,
    pub root_memory_id: Option<i64>,
    pub description: Option<String>,
    pub confidence: f64,
    pub user_id: i64,
    pub created_at: String,
    pub links: Vec<CausalLink>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CausalLink {
    pub id: i64,
    pub chain_id: i64,
    pub cause_memory_id: i64,
    pub effect_memory_id: i64,
    pub strength: f64,
    pub order_index: i32,
    pub created_at: String,
}

/// A memory discovered while walking backward from an effect.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CausalAncestor {
    /// The cause memory's id.
    pub memory_id: i64,
    /// Number of causal hops from the original effect to this ancestor
    /// (1 = direct cause, 2 = cause-of-cause, etc.).
    pub depth: usize,
    /// Minimum link strength observed along the shortest path from the
    /// effect down to this ancestor; a rough "weakest link" score so
    /// callers can filter low-confidence chains.
    pub strength_min: f64,
}

// ---------------------------------------------------------------------------
// Reflections
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reflection {
    pub id: i64,
    pub content: String,
    pub reflection_type: String,
    pub source_memory_ids: Vec<i64>,
    pub confidence: f64,
    pub user_id: i64,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Contradiction detection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Contradiction {
    pub memory_a: String,
    pub memory_b: String,
    pub confidence: f32,
    pub description: String,
}

// ---------------------------------------------------------------------------
// Memory health
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryHealthReport {
    pub total_memories: i64,
    pub without_embeddings: i64,
    pub archived: i64,
    pub superseded: i64,
    pub with_links: i64,
    pub avg_importance: f64,
    pub oldest_memory: Option<String>,
    pub embedding_coverage_pct: f64,
}

// ---------------------------------------------------------------------------
// Digests
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Digest {
    pub id: i64,
    pub period: String,
    pub content: String,
    pub memory_count: i32,
    pub user_id: i64,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Feedback
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackRequest {
    pub memory_id: i64,
    pub rating: String,
    pub context: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackStats {
    pub helpful: i64,
    pub irrelevant: i64,
    pub off_topic: i64,
    pub outdated: i64,
    pub total: i64,
}

// ---------------------------------------------------------------------------
// Intelligence tier
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntelligenceTier {
    Auto,
    Llm,
    Rules,
    Template,
}

impl IntelligenceTier {
    pub fn from_env() -> Self {
        match std::env::var("ENGRAM_INTELLIGENCE_TIER")
            .unwrap_or_default()
            .to_lowercase()
            .as_str()
        {
            "llm" => Self::Llm,
            "rules" => Self::Rules,
            "template" => Self::Template,
            _ => Self::Auto,
        }
    }
}
