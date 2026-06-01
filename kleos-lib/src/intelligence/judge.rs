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

/// Parse the LLM response into a JudgeOutput, tolerating surrounding prose.
/// Rejects responses with no scores.
fn parse_judgment(raw: &str) -> Option<JudgeOutput> {
    crate::intelligence::llm::repair_and_parse_json::<JudgeOutput>(raw)
        .filter(|o| !o.scores.is_empty())
}

/// True if the session is worth an LLM call: enough turns and a non-empty
/// transcript.
fn should_judge(turn_count: i32, transcript: &str, min_turns: i32) -> bool {
    turn_count >= min_turns && !transcript.trim().is_empty()
}

/// Public gate used by the session-end hook: whether a session is worth an LLM call.
pub fn judge_gate(turn_count: i32, transcript: &str, min_turns: i32) -> bool {
    should_judge(turn_count, transcript, min_turns)
}

/// Score a finished session and persist evaluations + session quality + Soma
/// quality. All-or-nothing: an LLM/parse failure aborts with no writes.
pub async fn judge_session<L: JudgeLlm>(
    db: &crate::db::Database,
    llm: &L,
    input: JudgeInput,
) -> crate::Result<()> {
    let rubrics = crate::services::thymus::list_rubrics(db, input.user_id).await?;
    let judged: Vec<_> = rubrics
        .into_iter()
        .filter(|r| JUDGED_RUBRICS.contains(&r.name.as_str()))
        .collect();
    if judged.is_empty() {
        tracing::warn!(
            user_id = input.user_id,
            "judge: no rubrics to score, skipping"
        );
        return Ok(());
    }

    let mut criteria: Vec<(String, String)> = Vec::new();
    for r in &judged {
        if let Some(arr) = r.criteria.as_array() {
            for c in arr {
                let name = c
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let desc = c
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                if !name.is_empty() {
                    criteria.push((name, desc));
                }
            }
        }
    }

    let (system, user) = build_prompt(&criteria, &input);
    let raw = llm
        .complete(&system, &user)
        .await
        .map_err(|e| crate::EngError::InvalidInput(format!("judge llm: {e}")))?;
    let Some(out) = parse_judgment(&raw) else {
        return Err(crate::EngError::InvalidInput(
            "judge: unparseable LLM response".into(),
        ));
    };

    let mut overall_sum = 0.0_f64;
    let mut overall_n = 0.0_f64;
    for r in &judged {
        let mut rubric_scores = serde_json::Map::new();
        if let Some(arr) = r.criteria.as_array() {
            for c in arr {
                if let Some(name) = c.get("name").and_then(|v| v.as_str()) {
                    if let Some(score) = out.scores.get(name) {
                        rubric_scores.insert(name.to_string(), serde_json::json!(score));
                    }
                }
            }
        }
        if rubric_scores.is_empty() {
            continue;
        }
        let eval = crate::services::thymus::evaluate(
            db,
            crate::services::thymus::EvaluateRequest {
                rubric_id: r.id,
                agent: input.agent.clone(),
                subject: input.session_id.clone(),
                input: None,
                output: None,
                scores: serde_json::Value::Object(rubric_scores),
                notes: Some(out.notes.clone()),
                evaluator: "thymus-judge".into(),
                user_id: Some(input.user_id),
            },
        )
        .await?;
        overall_sum += eval.overall_score;
        overall_n += 1.0;
    }

    let session_overall = if overall_n > 0.0 {
        overall_sum / overall_n
    } else {
        0.0
    };
    let rule_rate = judged
        .iter()
        .find(|r| r.name == "rule-compliance")
        .map(|_| session_overall);

    crate::services::thymus::record_session_quality(
        db,
        crate::services::thymus::RecordSessionQualityRequest {
            session_id: input.session_id.clone(),
            agent: input.agent.clone(),
            turn_count: Some(input.turn_count),
            rules_followed: Some(out.rules_followed.clone()),
            rules_drifted: Some(out.rules_drifted.clone()),
            personality_score: None,
            rule_compliance_rate: rule_rate,
            user_id: Some(input.user_id),
        },
    )
    .await?;

    if let Ok(agent) =
        crate::services::soma::get_agent_by_name(db, input.user_id, &input.agent).await
    {
        let rolled = match agent.quality_score {
            Some(p) => (p + session_overall) / 2.0,
            None => session_overall,
        };
        let _ = crate::services::soma::update_agent_quality(
            db,
            agent.id,
            input.user_id,
            Some(rolled),
            None,
        )
        .await;
    }

    tracing::info!(
        session_id = %input.session_id,
        agent = %input.agent,
        overall = session_overall,
        "thymus judge recorded"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    #[test]
    fn parses_valid_judgment_with_surrounding_prose() {
        let raw = "Here is the result:\n{\"scores\": {\"zero_waste\": 0.8, \
\"verify_results\": 1.0}, \"rules_followed\": [\"verify_results\"], \
\"rules_drifted\": [\"zero_waste\"], \"notes\": \"mostly solid\"}\nDone.";
        let out = parse_judgment(raw).expect("should parse");
        assert_eq!(out.scores.get("verify_results"), Some(&1.0));
        assert_eq!(out.rules_drifted, vec!["zero_waste".to_string()]);
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse_judgment("not json at all").is_none());
    }

    #[test]
    fn gate_skips_short_sessions() {
        assert!(!should_judge(2, "some text", 3));
        assert!(!should_judge(5, "   ", 3));
        assert!(should_judge(3, "real transcript", 3));
    }

    #[test]
    fn prompt_includes_criteria_and_transcript() {
        let criteria = vec![
            ("zero_waste".to_string(), "No wasted tool calls".to_string()),
            (
                "verify_results".to_string(),
                "Verifies before claiming done".to_string(),
            ),
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

    /// Stub LLM that always returns the given string unchanged.
    struct StubLlm(String);

    #[async_trait::async_trait]
    impl JudgeLlm for StubLlm {
        async fn complete(&self, _system: &str, _user: &str) -> Result<String, String> {
            Ok(self.0.clone())
        }
    }

    /// End-to-end: seed rubrics + agent, run judge_session, verify evaluations
    /// are written and Soma quality is updated.
    #[tokio::test]
    async fn judge_session_writes_quality_and_evaluations() {
        let db = Database::connect_memory().await.expect("db");

        // Seed rule-compliance rubric for user 1.
        crate::services::thymus::create_rubric(
            &db,
            crate::services::thymus::CreateRubricRequest {
                name: "rule-compliance".into(),
                description: None,
                criteria: serde_json::json!([
                    {"name": "engram_first", "weight": 1.0, "scale_min": 0.0, "scale_max": 1.0,
                     "description": "uses memory first"}
                ]),
                user_id: Some(1),
            },
        )
        .await
        .expect("rubric1");

        // Seed technical-precision rubric for user 1.
        crate::services::thymus::create_rubric(
            &db,
            crate::services::thymus::CreateRubricRequest {
                name: "technical-precision".into(),
                description: None,
                criteria: serde_json::json!([
                    {"name": "verify_results", "weight": 1.0, "scale_min": 0.0, "scale_max": 1.0,
                     "description": "verifies"}
                ]),
                user_id: Some(1),
            },
        )
        .await
        .expect("rubric2");

        // Register the agent. `RegisterAgentRequest` uses `type_` (not `agent_type`).
        crate::services::soma::register_agent(
            &db,
            crate::services::soma::RegisterAgentRequest {
                name: "claude-code".into(),
                type_: "cli".into(),
                description: None,
                capabilities: None,
                config: None,
                user_id: Some(1),
            },
        )
        .await
        .expect("agent");

        let stub = StubLlm(
            "{\"scores\": {\"engram_first\": 0.5, \"verify_results\": 1.0}, \
             \"rules_followed\": [\"verify_results\"], \
             \"rules_drifted\": [\"engram_first\"], \"notes\": \"ok\"}"
                .into(),
        );
        let input = JudgeInput {
            session_id: "s-test".into(),
            agent: "claude-code".into(),
            task: "t".into(),
            transcript: "did stuff".into(),
            turn_count: 5,
            user_id: 1,
        };

        judge_session(&db, &stub, input).await.expect("judge ok");

        // list_evaluations: (db, user_id, agent, rubric_id, limit) -- no offset.
        let evals = crate::services::thymus::list_evaluations(&db, 1, None, None, 50)
            .await
            .expect("list evals");
        assert_eq!(evals.len(), 2, "one evaluation per rubric");

        let agent = crate::services::soma::get_agent_by_name(&db, 1, "claude-code")
            .await
            .expect("agent");
        assert!(agent.quality_score.is_some(), "soma quality set");
    }
}
