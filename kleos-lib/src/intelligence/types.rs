//! Intelligence domain -- shared type definitions.
//! Ported from intelligence/types.ts.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// --- Reflection types ---

/// Valid reflection period values.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReflectionPeriod {
    Day,
    Week,
    Month,
}

/// Format `ReflectionPeriod` as the lowercase label used in API responses.
impl std::fmt::Display for ReflectionPeriod {
    /// Write the lowercase period label ("day", "week", or "month").
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Day => write!(f, "day"),
            Self::Week => write!(f, "week"),
            Self::Month => write!(f, "month"),
        }
    }
}

// --- Contradiction resolution ---

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContradictionResolution {
    KeepA,
    KeepB,
    KeepBoth,
    Merge,
}

// --- Decomposition types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecompositionResult {
    pub facts: Vec<String>,
    pub skip: bool,
}

/// Which decomposition tier produced a result: LLM, rule-based, or template.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DecompositionTier {
    Llm,
    #[serde(rename = "tier2-rules")]
    Tier2Rules,
    #[serde(rename = "tier3-template")]
    Tier3Template,
}

/// Format `DecompositionTier` as the canonical string label used in API responses.
impl std::fmt::Display for DecompositionTier {
    /// Write the tier label ("llm", "tier2-rules", or "tier3-template").
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Llm => write!(f, "llm"),
            Self::Tier2Rules => write!(f, "tier2-rules"),
            Self::Tier3Template => write!(f, "tier3-template"),
        }
    }
}

/// Pairs a `DecompositionResult` with the tier that produced it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecompositionWithTier {
    pub result: DecompositionResult,
    pub tier: DecompositionTier,
}

// --- Fact store metadata ---

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

// --- Valence types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValenceResult {
    pub valence: f64,
    pub arousal: f64,
    pub dominant_emotion: String,
    pub all_emotions: Vec<EmotionMatch>,
}

/// A single emotion label with its valence and arousal scores.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmotionMatch {
    pub emotion: String,
    pub valence: f64,
    pub arousal: f64,
}

/// A memory record augmented with its emotional valence, arousal, and dominant emotion label.
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

/// Aggregate emotional profile for a tenant: per-emotion stats and overall averages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmotionalProfile {
    pub emotions: Vec<EmotionStat>,
    pub overall: OverallEmotionStats,
}

/// Per-emotion aggregate: count and average valence/arousal across all matching memories.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmotionStat {
    pub dominant_emotion: String,
    pub count: i64,
    pub avg_valence: f64,
    pub avg_arousal: f64,
}

/// Overall emotional statistics: averages and positive/negative/neutral polarity counts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverallEmotionStats {
    pub avg_valence: f64,
    pub avg_arousal: f64,
    pub positive_count: i64,
    pub negative_count: i64,
    pub neutral_count: i64,
}

// --- Predictive types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PredictiveContext {
    pub time_context: String,
    pub predicted_categories: Vec<String>,
    pub predicted_project: Option<PredictedProject>,
    pub proactive_memories: Vec<ProactiveMemory>,
    pub suggested_actions: Vec<String>,
}

/// A project predicted to be relevant in the current temporal context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PredictedProject {
    pub id: i64,
    pub name: String,
}

/// A memory surfaced proactively because it is predicted to be relevant now.
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

// --- Reconsolidation types ---

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

/// The action taken during a reconsolidation sweep for a single memory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReconsolidationAction {
    Strengthened,
    Weakened,
    Corrected,
    Unchanged,
}

// --- Growth types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrowthReflectRequest {
    pub service: String,
    pub context: Vec<String>,
    pub existing_growth: Option<String>,
    pub prompt_override: Option<String>,
}

/// Result of a growth reflection: the generated observation and its storage ids.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrowthReflectResult {
    pub observation: Option<String>,
    pub stored_memory_id: Option<i64>,
    pub reflection_id: Option<i64>,
}

// --- Extraction stats ---

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExtractionStats {
    pub facts: i32,
    pub preferences: i32,
    pub state_updates: i32,
}

// --- Consolidation ---

#[derive(Debug, Clone, Serialize)]
pub struct ConsolidationRecord {
    pub id: i64,
    pub summary: String,
}

/// Summary of a consolidation sweep: how many pairs were examined and merged.
#[derive(Debug, Clone, Serialize)]
pub struct SweepResult {
    pub pairs_found: i64,
    pub consolidated: i64,
    /// Groups skipped due to safety guardrails (cluster too large, cap hit).
    pub skipped: i64,
}

// --- Duplicates ---

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

/// Summary of a deduplication run: candidates found, actually merged, and whether it was a dry run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeduplicateResult {
    pub pairs_found: i64,
    pub merged: i64,
    pub dry_run: bool,
}

// --- Temporal ---

/// A detected recurring pattern across memory timestamps.
///
/// Round-trips the `temporal_patterns` DB table. The `memory_ids` column is
/// stored as a JSON array in SQLite and deserialized here into a typed Vec.
/// `user_id` is intentionally absent -- the tenant DB is already scoped.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemporalPattern {
    /// Row id assigned by the database on INSERT; `None` before persistence.
    pub id: Option<i64>,
    /// Recurrence category: "daily" | "weekly" | "monthly" | "burst" | "interval".
    pub pattern_type: String,
    /// Human-readable description, e.g. "Recurring 'morning_routine' memories ~every 24.1h".
    pub description: String,
    /// Source memory ids that contributed to this pattern (capped at 50).
    pub memory_ids: Vec<i64>,
    /// Confidence in [0.0, 1.0]: `1.0 - (stddev / mean)`.
    pub confidence: f32,
    /// ISO-8601 duration string ("P1D", "P1W", "P30D"), or `None` if not applicable.
    pub recurrence: Option<String>,
    /// Wall-clock timestamp when this row was created; `None` before persistence.
    pub created_at: Option<String>,
}

/// A memory returned by a time-travel query (as it existed at or before a given timestamp).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeTravelResult {
    pub id: i64,
    pub content: String,
    pub category: String,
    pub importance: i32,
    pub created_at: String,
}

// --- Fact contradiction ---

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

// --- Growth observations ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrowthObservation {
    pub id: i64,
    pub content: String,
    pub source: String,
    pub importance: i64,
    pub created_at: String,
}

/// A growth observation ranked by relevance to a query, for prompt injection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredGrowthObservation {
    pub id: i64,
    pub content: String,
    pub source: String,
    pub score: f64,
    pub created_at: String,
}

// --- LLM options ---

/// Options for LLM calls.
#[derive(Debug, Clone)]
pub struct LlmOptions {
    pub temperature: f64,
    pub max_tokens: u32,
}

/// Provide conservative defaults: low temperature for determinism, modest token budget.
impl Default for LlmOptions {
    /// Return defaults: temperature 0.3 and max_tokens 1024.
    fn default() -> Self {
        Self {
            temperature: 0.3,
            max_tokens: 1024,
        }
    }
}

// --- Scheduler reports ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Ok,
    Failed,
    Skipped,
}

/// Report for a single scheduled intelligence task: outcome, duration, and optional output.
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

/// Aggregate report for a full intelligence pipeline run: all task reports and summary counts.
#[derive(Debug, Clone, Serialize)]
pub struct PipelineReport {
    pub reports: Vec<TaskReport>,
    pub total_duration_ms: u64,
    pub ok_count: usize,
    pub failed_count: usize,
    pub skipped_count: usize,
}

// --- Causal chains ---

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

/// A single directed edge in a causal chain: cause memory -> effect memory with a strength score.
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

// --- Reflections ---

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

// --- Contradiction detection ---

#[derive(Debug, Clone, Serialize)]
pub struct Contradiction {
    pub memory_a: String,
    pub memory_b: String,
    pub confidence: f32,
    pub description: String,
}

// --- Memory health ---

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

// --- Digests ---

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

// --- Feedback ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackRequest {
    pub memory_id: i64,
    pub rating: String,
    pub context: Option<String>,
}

/// Aggregate feedback statistics for a set of memories: counts by rating category.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackStats {
    pub helpful: i64,
    pub irrelevant: i64,
    pub off_topic: i64,
    pub outdated: i64,
    pub total: i64,
}

// --- Intelligence tier ---

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntelligenceTier {
    Auto,
    Llm,
    Rules,
    Template,
}

/// Construct and query `IntelligenceTier` from runtime environment.
impl IntelligenceTier {
    /// Read the intelligence-tier environment variable; default to `Auto`.
    ///
    /// Checks `KLEOS_INTELLIGENCE_TIER` first. Falls back to the legacy
    /// `ENGRAM_INTELLIGENCE_TIER` for backwards compatibility with existing
    /// deployments; this fallback may be removed in a future release.
    ///
    /// Accepted values (case-insensitive): `"llm"`, `"rules"`, `"template"`.
    /// Any other value (including unset) resolves to `Auto`.
    pub fn from_env() -> Self {
        let raw = crate::kleos_env("INTELLIGENCE_TIER").unwrap_or_default();
        match raw.to_lowercase().as_str() {
            "llm" => Self::Llm,
            "rules" => Self::Rules,
            "template" => Self::Template,
            _ => Self::Auto,
        }
    }
}
