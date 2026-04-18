//! Intelligence domain -- shared type definitions.
//! Ported from intelligence/types.ts.

use serde::{Deserialize, Serialize};

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
