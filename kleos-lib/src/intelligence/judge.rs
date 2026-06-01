//! Session-end evaluation ("Thymus judge"): scores a finished agent session
//! against the rule-compliance and technical-precision rubrics with one LLM
//! call, then records evaluations, session quality, and Soma agent quality.

use serde::Deserialize;

/// The two rubrics judged in v1. gir-personality is intentionally excluded
/// (stale; persona fidelity is deferred to v2).
pub const JUDGED_RUBRICS: [&str; 2] = ["rule-compliance", "technical-precision"];

/// Snapshot of a finished session handed to the judge. Owned data so it can
/// move into a spawned task without borrowing the live SessionManager.
#[derive(Debug, Clone)]
pub struct JudgeInput {
    /// Stable session identifier.
    pub session_id: String,
    /// Agent name that ran the session.
    pub agent: String,
    /// The task the session worked on.
    pub task: String,
    /// Joined transcript of the session output.
    pub transcript: String,
    /// Number of turns/output lines in the session.
    pub turn_count: i32,
    /// Owning user id (tenant).
    pub user_id: i64,
}

/// Structured judgment returned by the LLM. Scores are 0.0-1.0 per criterion,
/// keyed by criterion name across both rubrics.
#[derive(Debug, Clone, Deserialize)]
pub struct JudgeOutput {
    /// Per-criterion scores in 0.0-1.0.
    pub scores: std::collections::HashMap<String, f64>,
    /// Criteria the agent followed.
    #[serde(default)]
    pub rules_followed: Vec<String>,
    /// Criteria the agent drifted from.
    #[serde(default)]
    pub rules_drifted: Vec<String>,
    /// One-line evaluator note.
    #[serde(default)]
    pub notes: String,
}

/// LLM seam so tests inject a stub instead of hitting a live model.
#[async_trait::async_trait]
pub trait JudgeLlm: Send + Sync {
    /// Complete a system+user prompt, returning the raw model text.
    async fn complete(&self, system: &str, user: &str) -> Result<String, String>;
}

/// Production implementation delegating to intelligence::llm::call_llm.
pub struct RealJudgeLlm;

#[async_trait::async_trait]
impl JudgeLlm for RealJudgeLlm {
    async fn complete(&self, system: &str, user: &str) -> Result<String, String> {
        crate::intelligence::llm::call_llm(system, user, None).await
    }
}
