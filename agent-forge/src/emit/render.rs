//! Markdown renderers. These are pure string functions so the emitted shape is
//! testable without touching the filesystem or the database.

use crate::emit::model::{ApproachRow, SpecRecord};
use crate::emit::trust::Trust;

/// The irreducible prose a model supplies for one slice. Everything else in a
/// slice document is assembled from stored rows.
pub struct SliceContent {
    /// One-line statement of what this slice set out to do.
    pub intent: String,
    /// One entry per component touched: what it does and under what conditions.
    pub components: Vec<String>,
    /// Non-obvious conditions: root causes, gotchas, and documented limitations.
    pub conditions: Vec<String>,
}

/// Render a bullet list, or a placeholder line when the list is empty, so a
/// section never renders as a bare heading.
fn bullets(items: &[String]) -> String {
    if items.is_empty() {
        return "_None recorded._\n".to_string();
    }
    items
        .iter()
        .map(|i| format!("- {}\n", i))
        .collect::<Vec<_>>()
        .join("")
}

/// Display a zero-based criterion index as the one-based label used in emitted
/// documents.
fn requirement_label(index: usize) -> String {
    format!("R{}", index + 1)
}

/// Normalize prose for a Markdown table cell without changing its meaning.
fn table_cell(value: &str) -> String {
    value.replace('|', "\\|").replace(['\r', '\n'], " ")
}

/// Return the successful verification commands linked to one criterion in
/// chronological order, with duplicate commands collapsed.
fn passing_evidence(record: &SpecRecord, criteria_index: usize) -> Vec<&str> {
    let Ok(criteria_index) = i64::try_from(criteria_index) else {
        return Vec::new();
    };
    let mut commands = Vec::new();
    for verification in &record.verifications {
        if verification.success
            && verification.criteria_index == Some(criteria_index)
            && !commands.contains(&verification.command.as_str())
        {
            commands.push(verification.command.as_str());
        }
    }
    commands
}

/// Choose a code-fence long enough that the content cannot close it early.
/// CommonMark ends a fenced block at the first line whose backtick run is at
/// least as long as the opening fence, so an interface contract containing its
/// own fence would otherwise terminate the block and swallow everything after
/// it into a code span. The fence is therefore one backtick longer than the
/// longest run the content contains, with three as the floor.
fn fence_for(content: &str) -> String {
    let mut longest = 0usize;
    let mut current = 0usize;
    for ch in content.chars() {
        if ch == '`' {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    "`".repeat(longest.max(2) + 1)
}

/// Render the chosen approach as a decision block naming the strongest rejected
/// alternative and the reason it lost. Returns an empty string when no approach
/// was marked chosen, because a decision with no chosen path is not a decision.
fn render_decision(approaches: &[ApproachRow], trust: Trust) -> String {
    // Known gap against the design's decision-block schema: `where:` (a file or
    // area anchoring the decision) is specified as non-optional, but nothing in
    // the data model can source it -- `approaches` has no such column. The field
    // is therefore absent rather than fabricated. Phase 2 computes the changed
    // file set per slice, which is where an anchor would come from.
    let Some(chosen) = approaches.iter().find(|a| a.chosen) else {
        return String::new();
    };

    // The strongest rejected alternative is the highest-scoring unchosen
    // approach. Unscored approaches compare as f64::MIN so they sort last, and
    // max_by yields the LAST of several equal maxima, so a tie (including a tie
    // between two unscored approaches) resolves to the most recently recorded.
    let alternative = approaches.iter().filter(|a| !a.chosen).max_by(|a, b| {
        a.score
            .unwrap_or(f64::MIN)
            .partial_cmp(&b.score.unwrap_or(f64::MIN))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut out = format!("\n## Decision: {}\n\n", chosen.name);
    out.push_str(&format!("- **why:** {}\n", chosen.description));
    if let Some(alt) = alternative {
        let reason = if alt.cons.is_empty() {
            "no reason recorded".to_string()
        } else {
            alt.cons.join("; ")
        };
        out.push_str(&format!(
            "- **alternative:** {} -- rejected: {}\n",
            alt.name, reason
        ));
    }
    out.push_str(&format!("- **trust:** {}\n", trust.label()));
    out
}

/// Render a spec's top-level record document.
pub fn render_record(record: &SpecRecord, trust: Trust) -> String {
    let mut out = format!("# Record: {}\n\n", record.task_description);
    out.push_str(&format!(
        "- **spec:** `{}`\n- **type:** {}\n\n",
        record.id, record.task_type
    ));

    out.push_str("## Acceptance criteria\n\n");
    out.push_str(&bullets(&record.acceptance_criteria));

    out.push_str("\n## Edge cases\n\n");
    out.push_str(&bullets(&record.edge_cases));

    if let Some(contract) = &record.interface_contract {
        out.push_str("\n## Interface contract\n\n");
        let fence = fence_for(contract);
        out.push_str(&format!("{}text\n{}\n{}\n", fence, contract, fence));
    }

    out.push_str(&render_decision(&record.approaches, trust));

    out.push_str("\n## Verification evidence\n\n");
    if record.verifications.is_empty() {
        out.push_str("_No verification runs recorded._\n");
    } else {
        for v in &record.verifications {
            out.push_str(&format!(
                "- `{}` -- {}\n",
                v.command,
                if v.success { "passed" } else { "failed" }
            ));
        }
    }

    out
}

/// Render the requirements artifact from authoritative spec fields.
pub fn render_requirements(record: &SpecRecord) -> String {
    let mut out = format!("# Requirements: {}\n\n", record.task_description);
    out.push_str(&format!(
        "- **spec:** `{}`\n- **type:** {}\n\n",
        record.id, record.task_type
    ));

    out.push_str("## Acceptance requirements\n\n");
    if record.acceptance_criteria.is_empty() {
        out.push_str("_None recorded._\n");
    } else {
        for (index, criterion) in record.acceptance_criteria.iter().enumerate() {
            out.push_str(&format!(
                "### {}\n\n{}\n\n",
                requirement_label(index),
                criterion
            ));
        }
    }

    out.push_str("## Edge cases\n\n");
    out.push_str(&bullets(&record.edge_cases));

    out.push_str("\n## Behavior to preserve\n\n");
    if record.unchanged_behaviors.is_empty() && record.task_type == "bugfix" {
        out.push_str(
            "> **Specification gap:** No unchanged behavior was recorded for this bug fix.\n",
        );
    } else {
        out.push_str(&bullets(&record.unchanged_behaviors));
    }

    out.push_str("\n## Property-test candidates\n\n");
    if record.test_properties.is_empty() {
        out.push_str("_None recorded. Candidates are never inferred._\n");
    } else {
        for (index, property) in record.test_properties.iter().enumerate() {
            out.push_str(&format!(
                "- **P{} ({})**: {}\n",
                index + 1,
                requirement_label(property.criteria_index),
                property.description
            ));
        }
    }

    out
}

/// Render the design artifact from the interface contract, expected change
/// surface, dependencies, and recorded approach comparison.
pub fn render_design(record: &SpecRecord) -> String {
    let mut out = format!("# Design: {}\n\n", record.task_description);
    out.push_str(&format!("- **spec:** `{}`\n\n", record.id));

    out.push_str("## Interface contract\n\n");
    if let Some(contract) = &record.interface_contract {
        let fence = fence_for(contract);
        out.push_str(&format!("{}text\n{}\n{}\n", fence, contract, fence));
    } else {
        out.push_str("_None recorded._\n");
    }

    out.push_str("\n## Dependencies\n\n");
    match &record.dependencies {
        Some(dependencies) => out.push_str(&format!("{}\n", dependencies)),
        None => out.push_str("_None recorded._\n"),
    }

    out.push_str("\n## Expected files\n\n");
    out.push_str(&bullets(&record.files_to_touch));

    out.push_str("\n## Chosen approach\n\n");
    if let Some(chosen) = record.approaches.iter().find(|approach| approach.chosen) {
        out.push_str(&format!(
            "### {}\n\n{}\n\n",
            chosen.name, chosen.description
        ));
        out.push_str("**Advantages**\n\n");
        out.push_str(&bullets(&chosen.pros));
        out.push_str("\n**Tradeoffs**\n\n");
        out.push_str(&bullets(&chosen.cons));
    } else {
        out.push_str("_No approach chosen._\n");
    }

    out.push_str("\n## Rejected alternatives\n\n");
    let rejected: Vec<&ApproachRow> = record
        .approaches
        .iter()
        .filter(|approach| !approach.chosen)
        .collect();
    if rejected.is_empty() {
        out.push_str("_None recorded._\n");
    } else {
        for approach in rejected {
            out.push_str(&format!(
                "### {}\n\n{}\n\n",
                approach.name, approach.description
            ));
            out.push_str("**Why it was not chosen**\n\n");
            out.push_str(&bullets(&approach.cons));
            out.push('\n');
        }
    }

    out
}

/// Render implementation tasks and a criterion-to-task-to-verification table.
/// Task checkboxes are computed from evidence and are never stored as mutable
/// state.
pub fn render_tasks(record: &SpecRecord) -> String {
    let mut out = format!("# Tasks: {}\n\n", record.task_description);
    out.push_str(&format!("- **spec:** `{}`\n\n", record.id));
    out.push_str(
        "> Checkboxes are derived from successful verification evidence for every linked requirement.\n\n",
    );

    out.push_str("## Implementation tasks\n\n");
    if record.implementation_tasks.is_empty() {
        out.push_str("_No implementation tasks recorded._\n");
    } else {
        for (index, task) in record.implementation_tasks.iter().enumerate() {
            let complete = !task.criteria_indices.is_empty()
                && task
                    .criteria_indices
                    .iter()
                    .all(|criteria_index| !passing_evidence(record, *criteria_index).is_empty());
            let labels = task
                .criteria_indices
                .iter()
                .map(|criteria_index| requirement_label(*criteria_index))
                .collect::<Vec<_>>()
                .join(", ");
            let evidence = task
                .criteria_indices
                .iter()
                .flat_map(|criteria_index| passing_evidence(record, *criteria_index))
                .fold(Vec::new(), |mut commands, command| {
                    if !commands.contains(&command) {
                        commands.push(command);
                    }
                    commands
                });

            out.push_str(&format!(
                "- [{}] **T{}:** {}\n  - Requirements: {}\n",
                if complete { "x" } else { " " },
                index + 1,
                task.description,
                labels
            ));
            if evidence.is_empty() {
                out.push_str("  - Passing evidence: _None recorded._\n");
            } else {
                out.push_str(&format!("  - Passing evidence: {}\n", evidence.join("; ")));
            }
        }
    }

    out.push_str("\n## Traceability\n\n");
    out.push_str("| Requirement | Tasks | Passing verification |\n");
    out.push_str("| --- | --- | --- |\n");
    for criteria_index in 0..record.acceptance_criteria.len() {
        let tasks = record
            .implementation_tasks
            .iter()
            .enumerate()
            .filter(|(_, task)| task.criteria_indices.contains(&criteria_index))
            .map(|(index, _)| format!("T{}", index + 1))
            .collect::<Vec<_>>();
        let evidence = passing_evidence(record, criteria_index)
            .into_iter()
            .map(table_cell)
            .collect::<Vec<_>>();
        out.push_str(&format!(
            "| {} | {} | {} |\n",
            requirement_label(criteria_index),
            if tasks.is_empty() {
                "**Unassigned**".to_string()
            } else {
                tasks.join(", ")
            },
            if evidence.is_empty() {
                "_None_".to_string()
            } else {
                evidence.join("<br>")
            }
        ));
    }

    out
}

/// Render one slice document: the model's knowledge-transfer prose plus the
/// decisions and discoveries stored for the spec.
pub fn render_slice(
    index: i64,
    content: &SliceContent,
    record: &SpecRecord,
    trust: Trust,
) -> String {
    let mut out = format!("# Slice {:03}: {}\n\n", index, content.intent);
    out.push_str(&format!("- **spec:** `{}`\n\n", record.id));

    out.push_str("## Components\n\n");
    out.push_str(&bullets(&content.components));

    out.push_str("\n## Hard-won conditions\n\n");
    let mut conditions = content.conditions.clone();
    // Phase 1 has no time window, so every slice repeats the spec's entire
    // discovery history rather than only what is new since the previous
    // checkpoint. Slice 003 therefore restates what 001 and 002 already showed.
    // Phase 2's slice diffing supplies the scoping that fixes this.
    for learn in &record.learns {
        conditions.push(match &learn.context {
            Some(ctx) => format!("{} ({})", learn.discovery, ctx),
            None => learn.discovery.clone(),
        });
    }
    out.push_str(&bullets(&conditions));

    out.push_str(&render_decision(&record.approaches, trust));

    out
}

#[cfg(test)]
/// Tests for the record and slice renderers.
mod tests {
    use super::*;
    use crate::emit::model::{ApproachRow, LearnRow, SpecRecord, VerificationRow};
    use crate::emit::trust::Trust;

    /// Build a record with one chosen and one rejected approach plus a learning.
    fn record() -> SpecRecord {
        SpecRecord {
            id: "spec_1".into(),
            task_description: "Add a thing".into(),
            task_type: "feature".into(),
            acceptance_criteria: vec!["it works".into()],
            edge_cases: vec!["empty input".into()],
            interface_contract: Some("fn thing() -> u8".into()),
            files_to_touch: vec!["src/thing.rs".into()],
            dependencies: Some("serde".into()),
            unchanged_behaviors: vec!["old behavior remains".into()],
            implementation_tasks: vec![crate::spec_types::ImplementationTask {
                description: "Build the thing".into(),
                criteria_indices: vec![0],
            }],
            test_properties: vec![crate::spec_types::TestProperty {
                description: "All valid inputs round-trip".into(),
                criteria_index: 0,
            }],
            approaches: vec![
                ApproachRow {
                    name: "Direct".into(),
                    description: "Do it directly".into(),
                    pros: vec!["simple".into()],
                    cons: vec![],
                    score: Some(8.0),
                    chosen: true,
                },
                ApproachRow {
                    name: "Indirect".into(),
                    description: "Add a layer".into(),
                    pros: vec![],
                    cons: vec!["slower".into()],
                    score: Some(5.0),
                    chosen: false,
                },
            ],
            learns: vec![LearnRow {
                discovery: "The cache lies on cold start".into(),
                context: Some("found while tracing".into()),
                tags: vec!["cache".into()],
            }],
            verifications: vec![VerificationRow {
                command: "cargo test".into(),
                success: true,
                criteria_index: Some(0),
            }],
        }
    }

    /// The record renders its intent, criteria, edge cases, and contract.
    #[test]
    fn record_renders_spec_fields() {
        let md = render_record(&record(), Trust::SpecVerified);
        assert!(md.contains("# Record: Add a thing"));
        assert!(md.contains("it works"));
        assert!(md.contains("empty input"));
        assert!(md.contains("fn thing() -> u8"));
    }

    /// The record renders the chosen approach as a decision naming the rejected
    /// alternative and its rejection reason.
    #[test]
    fn record_renders_decision_with_alternative() {
        let md = render_record(&record(), Trust::SpecVerified);
        assert!(md.contains("## Decision: Direct"));
        assert!(md.contains("**alternative:**"));
        assert!(md.contains("Indirect"));
        assert!(md.contains("slower"));
    }

    /// The trust label appears verbatim so its caveat cannot be lost.
    #[test]
    fn record_renders_trust_label() {
        let md = render_record(&record(), Trust::SpecVerified);
        assert!(md.contains("not separately proved"));
    }

    /// A record with no approaches renders without a decisions section rather
    /// than emitting an empty heading.
    #[test]
    fn record_without_approaches_omits_decisions() {
        let mut r = record();
        r.approaches.clear();
        let md = render_record(&r, Trust::Unverified);
        assert!(!md.contains("## Decision:"));
    }

    /// A slice renders its components and its hard-won conditions under distinct
    /// headings, and carries the learnings captured for the spec.
    #[test]
    fn slice_renders_knowledge_transfer() {
        let content = SliceContent {
            intent: "wire it up".into(),
            components: vec!["Renderer -- turns rows into markdown".into()],
            conditions: vec!["Empty specs still render".into()],
        };
        let md = render_slice(2, &content, &record(), Trust::SpecVerified);
        assert!(md.contains("# Slice 002: wire it up"));
        assert!(md.contains("## Components"));
        assert!(md.contains("Renderer -- turns rows into markdown"));
        assert!(md.contains("## Hard-won conditions"));
        assert!(md.contains("Empty specs still render"));
        assert!(md.contains("The cache lies on cold start"));
    }

    /// An interface contract containing its own code fence does not terminate the
    /// block early. Without a longer outer fence the contract's closing backticks
    /// would end the section and swallow the Decision and Verification headings
    /// into a code span -- silent corruption of a committed document.
    #[test]
    fn interface_contract_containing_a_fence_stays_fenced() {
        let mut r = record();
        r.interface_contract = Some("example:\n```\nfn thing() {}\n```".into());
        let md = render_record(&r, Trust::SpecVerified);
        assert!(md.contains("````text"));
        // The opening and closing fences must PAIR. Asserting that the later
        // headings are merely present proves nothing, because they are emitted
        // unconditionally and `contains` does not parse markdown -- a mismatched
        // closing fence would leave the block open and still satisfy that check.
        // Counting the four-backtick runs catches exactly that regression: the
        // contract's own three-backtick fences cannot match, so a correct render
        // has precisely two.
        assert_eq!(md.matches("````").count(), 2);
    }

    /// Content with no backticks gets the minimum three-backtick fence, not a
    /// wider one. Without this, an off-by-one in `fence_for` would go unnoticed
    /// because every other test only checks that the content survives.
    #[test]
    fn fence_floor_is_three_backticks() {
        let mut r = record();
        r.interface_contract = Some("fn thing() -> u8".into());
        let md = render_record(&r, Trust::SpecVerified);
        assert!(md.contains("```text"));
        assert!(!md.contains("````"));
    }

    /// Requirements preserve stable labels, bugfix gaps, and explicit property
    /// candidates without inventing additional properties.
    #[test]
    fn requirements_render_structured_spec_fields() {
        let mut r = record();
        r.task_type = "bugfix".into();
        r.unchanged_behaviors.clear();
        let md = render_requirements(&r);

        assert!(md.contains("### R1"));
        assert!(md.contains("No unchanged behavior was recorded"));
        assert!(md.contains("**P1 (R1)**"));
        assert!(md.contains("All valid inputs round-trip"));
    }

    /// Design output contains the contract, expected change surface, chosen
    /// approach, and rejected alternative.
    #[test]
    fn design_renders_recorded_decisions() {
        let md = render_design(&record());

        assert!(md.contains("fn thing() -> u8"));
        assert!(md.contains("src/thing.rs"));
        assert!(md.contains("## Chosen approach"));
        assert!(md.contains("### Direct"));
        assert!(md.contains("### Indirect"));
    }

    /// A task remains pending until all linked criteria have successful
    /// evidence, then becomes checked without any task-state mutation.
    #[test]
    fn task_completion_is_derived_from_passing_evidence() {
        let mut r = record();
        r.acceptance_criteria.push("second condition".into());
        r.implementation_tasks[0].criteria_indices.push(1);
        let pending = render_tasks(&r);
        assert!(pending.contains("- [ ] **T1:**"));

        r.verifications.push(VerificationRow {
            command: "cargo test second".into(),
            success: true,
            criteria_index: Some(1),
        });
        let complete = render_tasks(&r);
        assert!(complete.contains("- [x] **T1:**"));
        assert!(complete.contains("| R2 | T1 | cargo test second |"));
    }

    /// The traceability table makes uncovered requirements and absent evidence
    /// visible instead of silently treating them as complete.
    #[test]
    fn tasks_render_traceability_gaps() {
        let mut r = record();
        r.acceptance_criteria.push("unassigned condition".into());
        let md = render_tasks(&r);

        assert!(md.contains("| R2 | **Unassigned** | _None_ |"));
    }

    /// The rendered alternative is the highest-SCORING rejected approach, and the
    /// score is what drives the choice -- not the position in the list.
    ///
    /// The ordering here is deliberate. `Strong` is neither the first nor the
    /// last rejected approach, so this arrangement discriminates between all
    /// three plausible selection rules at once: "first unchosen" would surface
    /// `Weak`, "last unchosen" would surface `Mid`, and only a score-driven rule
    /// surfaces `Strong`. Putting the highest scorer at either end, as an earlier
    /// version of this test did, cannot tell those rules apart.
    #[test]
    fn alternative_is_the_highest_scoring_rejection() {
        let mut r = record();
        r.approaches = vec![
            ApproachRow {
                name: "Chosen".into(),
                description: "the taken path".into(),
                pros: vec![],
                cons: vec![],
                score: Some(9.0),
                chosen: true,
            },
            ApproachRow {
                name: "Weak".into(),
                description: "a poor option".into(),
                pros: vec![],
                cons: vec!["barely works".into()],
                score: Some(2.0),
                chosen: false,
            },
            ApproachRow {
                name: "Strong".into(),
                description: "the real contender".into(),
                pros: vec![],
                cons: vec!["slower".into()],
                score: Some(7.0),
                chosen: false,
            },
            ApproachRow {
                name: "Mid".into(),
                description: "a middling option".into(),
                pros: vec![],
                cons: vec!["awkward".into()],
                score: Some(5.0),
                chosen: false,
            },
        ];
        let md = render_record(&r, Trust::SpecVerified);
        assert!(md.contains("**alternative:** Strong -- rejected: slower"));
        assert!(!md.contains("Weak"));
        assert!(!md.contains("Mid"));
    }
}
