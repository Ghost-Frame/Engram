use std::sync::Arc;

use kleos_lib::llm::{local::LocalModelClient, CallOptions, Priority};
use serde::Deserialize;

const GATE_SYSTEM_PROMPT: &str = "\
You are a memory curator for a developer's AI coding assistant. \
Given a message from a coding session, decide if it contains information worth remembering long-term.

STORE if it contains: decisions made, preferences stated, problems solved with the solution, \
infrastructure facts discovered, deployment outcomes, architecture choices, credentials/access info \
(redacted references only), or lessons learned from debugging.

SKIP if it is: routine troubleshooting steps without a conclusion, generic explanations of how \
things work, conversational filler, raw tool output without interpretation, questions without answers, \
content that only makes sense in the immediate session context, or step-by-step instructions that \
are easily re-derived.

Respond with JSON only, no markdown fencing:
{\"store\": true, \"category\": \"decision|infrastructure|issue|reference|state\", \"importance\": 1-9, \"summary\": \"one-line distillation\"}
or
{\"store\": false}";

/// JSON verdict the gate LLM returns for a single turn. `store=false` skips
/// the memory entirely; otherwise the optional fields shape how it lands in
/// the memory table.
#[derive(Debug, Deserialize)]
pub struct Verdict {
    pub store: bool,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub importance: Option<i32>,
    #[serde(default)]
    pub summary: Option<String>,
}

/// Output of the gate for one input turn: the LLM verdict plus the original
/// text and session/project context needed to construct a memory write.
pub struct GateResult {
    pub verdict: Verdict,
    pub original_text: String,
    pub session_id: String,
    pub project: String,
}

/// LLM-backed quality gate that classifies assistant turns as store-worthy
/// or skippable. Holds a shared LLM client, an optional model override, and a
/// minimum inter-call delay so a long batch cannot pin the local model.
pub struct MemoryGate {
    llm: Arc<LocalModelClient>,
    model: Option<String>,
    pace: std::time::Duration,
}

/// Constructor and batch evaluation entry points for the LLM-backed gate.
impl MemoryGate {
    /// Construct a new gate with the given LLM client, optional model override,
    /// and minimum inter-call pacing in milliseconds (0 disables the sleep).
    pub fn new(llm: Arc<LocalModelClient>, model: Option<String>, pace_ms: u64) -> Self {
        Self {
            llm,
            model,
            pace: std::time::Duration::from_millis(pace_ms),
        }
    }

    /// Run the gate against each turn in order, sleeping `pace` between calls
    /// to space out GPU load. Failed evaluations are logged and skipped; the
    /// returned vec contains only turns the LLM successfully classified.
    pub async fn evaluate_batch(&self, turns: Vec<PendingTurn>) -> Vec<GateResult> {
        let mut results = Vec::new();
        let mut first = true;

        for turn in turns {
            if !first && !self.pace.is_zero() {
                tokio::time::sleep(self.pace).await;
            }
            first = false;
            match self.evaluate_single(&turn.text).await {
                Ok(verdict) => {
                    results.push(GateResult {
                        verdict,
                        original_text: turn.text,
                        session_id: turn.session_id,
                        project: turn.project,
                    });
                }
                Err(e) => {
                    tracing::warn!(error = %e, "gate evaluation failed, skipping turn");
                }
            }
        }

        results
    }

    /// Send one truncated turn to the LLM and parse the JSON verdict it
    /// returns. Strips markdown fencing if the model wraps its output in code
    /// blocks despite the prompt asking for raw JSON.
    async fn evaluate_single(&self, text: &str) -> Result<Verdict, String> {
        let truncated: String = text.chars().take(1500).collect();

        let opts = CallOptions {
            model: self.model.clone(),
            max_tokens: Some(150),
            temperature: Some(0.1),
            priority: Priority::Background,
            timeout_ms: Some(15_000),
        };

        let response = self
            .llm
            .call(GATE_SYSTEM_PROMPT, &truncated, Some(opts))
            .await
            .map_err(|e| format!("llm call failed: {e}"))?;

        let cleaned = response.trim();
        // Strip markdown fencing if the model adds it anyway
        let json_str = if cleaned.starts_with("```") {
            cleaned
                .trim_start_matches("```json")
                .trim_start_matches("```")
                .trim_end_matches("```")
                .trim()
        } else {
            cleaned
        };

        serde_json::from_str::<Verdict>(json_str)
            .map_err(|e| format!("failed to parse verdict: {e} -- raw: {json_str}"))
    }
}

/// One assistant turn queued for gate evaluation. Carries the raw text plus
/// the session and project context that will be attached to the stored memory.
pub struct PendingTurn {
    pub text: String,
    pub session_id: String,
    pub project: String,
}
