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

/// Build the (system, user) prompt. `criteria` is the flattened list of
/// (criterion_name, description) across the judged rubrics.
fn build_prompt(criteria: &[(String, String)], input: &JudgeInput) -> (String, String) {
    let system = "You are a strict evaluator of AI agent work sessions. \
Score each listed criterion from 0.0 (total failure) to 1.0 (perfect) based ONLY \
on the transcript. Respond with a single JSON object and nothing else: \
{\"scores\": {\"<criterion>\": <0.0-1.0>, ...}, \"rules_followed\": [\"<criterion>\", ...], \
\"rules_drifted\": [\"<criterion>\", ...], \"notes\": \"<one short sentence>\"}. \
Include every criterion in \"scores\"."
        .to_string();

    let mut user = String::new();
    user.push_str("CRITERIA:\n");
    for (name, desc) in criteria {
        user.push_str(&format!("- {name}: {desc}\n"));
    }
    user.push_str(&format!("\nAGENT: {}\n", input.agent));
    user.push_str(&format!("TASK: {}\n", input.task));
    user.push_str(&format!("TURNS: {}\n", input.turn_count));
    user.push_str("\nTRANSCRIPT:\n");
    user.push_str(&input.transcript);
    (system, user)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_includes_criteria_and_transcript() {
        let criteria = vec![
            ("zero_waste".to_string(), "No wasted tool calls".to_string()),
            ("verify_results".to_string(), "Verifies before claiming done".to_string()),
        ];
        let input = JudgeInput {
            session_id: "s1".into(),
            agent: "claude-code".into(),
            task: "fix the build".into(),
            transcript: "ran cargo build, it passed".into(),
            turn_count: 5,
            user_id: 1,
        };
        let (system, user) = build_prompt(&criteria, &input);
        assert!(system.contains("0.0"));
        assert!(user.contains("zero_waste"));
        assert!(user.contains("No wasted tool calls"));
        assert!(user.contains("ran cargo build"));
        assert!(user.contains("fix the build"));
    }
}
