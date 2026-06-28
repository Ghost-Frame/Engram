//! Offline calibration harness for the L2 ABSTAIN gate.
//!
//! Drives the *production* decision logic (`kleos_lib::memory::abstain::evaluate_signals`)
//! over a labelled set of answerable/unanswerable queries and their captured retrieval
//! signals (`fixtures/abstain/calibration.json`). For a sweep of semantic thresholds it
//! reports:
//!   - FAR (false-abstain rate): fraction of ANSWERABLE queries the gate wrongly abstained.
//!   - TAR (true-abstain rate):  fraction of UNANSWERABLE queries the gate correctly abstained.
//!
//! It asserts the targets at the shipped default threshold (0.65): FAR < 5%, TAR > 80%.
//!
//! This runs in CI (fast, no models) as a regression guard that the gate logic + default
//! threshold still separate the documented score distribution. It is NOT a substitute for a
//! production threshold freeze: the fixture's `sem_top`/`ce_top` are seeded from the
//! agent-in-the-loop distribution. Before freezing a threshold for a NEW embedder/reranker,
//! re-capture per-query signals from an isolated kleos-server (real bge-m3 + reranker,
//! scratch DB) running each fixture `q` against the fixture `corpus`, update the JSON, and
//! re-run this sweep.

use kleos_lib::memory::abstain::{evaluate_signals, AbstainConfig};
use kleos_lib::memory::types::QuestionType;
use serde_json::Value;

/// Embedded calibration fixture (corpus + labelled queries with captured signals).
const FIXTURE: &str = include_str!("fixtures/abstain/calibration.json");

/// The shipped default semantic threshold; the FAR/TAR targets are asserted here.
const DEFAULT_THRESHOLD: f64 = 0.65;

/// One labelled calibration query, projected from the fixture JSON.
struct CalQuery {
    id: String,
    answerable: bool,
    qt: QuestionType,
    sem_top: Option<f64>,
    ce_top: Option<f64>,
}

/// Parse the `queries` array into typed rows. Panics with context on a malformed fixture so
/// an authoring mistake fails loudly rather than silently skewing the rates.
fn load_queries() -> Vec<CalQuery> {
    let root: Value = serde_json::from_str(FIXTURE).expect("calibration.json parses");
    let arr = root["queries"].as_array().expect("queries array");
    arr.iter()
        .map(|q| {
            let id = q["id"].as_str().expect("query id").to_string();
            let class = q["class"].as_str().expect("query class");
            let answerable = match class {
                "answerable" => true,
                "unanswerable" => false,
                other => panic!("query {id}: unknown class `{other}`"),
            };
            // Reuse QuestionType's snake_case Deserialize so the labels match production.
            let qt: QuestionType = serde_json::from_value(q["question_type"].clone())
                .unwrap_or_else(|_| panic!("query {id}: bad question_type"));
            CalQuery {
                id,
                answerable,
                qt,
                sem_top: q["sem_top"].as_f64(),
                ce_top: q["ce_top"].as_f64(),
            }
        })
        .collect()
}

/// Config for one sweep point: gate enabled, only the semantic threshold varies; the CE
/// rescue and margin sit at their shipped defaults so the sweep reflects real behavior.
fn sweep_config(semantic: f64) -> AbstainConfig {
    AbstainConfig {
        enabled: true,
        semantic,
        fact_relax: 0.0,
        // Recalibrated 2026-06-28 for bge-reranker-v2-m3 (was 0.80, tuned for granite). The new
        // reranker's CE distribution is sharply bimodal (relevant ~0.98-1.0, irrelevant ~0.00),
        // so a legitimately-rescuable answerable boundary case (ans-dahlia-rescued) lands at
        // ce=0.752 -- below the old 0.80 floor, which false-abstained it. 0.70 sits in the wide
        // valley below that case and well above the irrelevant cluster.
        ce_rescue: 0.70,
        ce_floor: 0.0,
        margin: 0.0,
    }
}

/// Production-faithful abstain decision for one calibration query. Mirrors the search route's
/// `abstained = abstain.abstain || results.is_empty()`: a query that retrieved NOTHING (no
/// semantic AND no cross-encoder signal, e.g. un-atlantis) abstains in production via the
/// empty-result path, which `evaluate_signals` alone -- reasoning over a non-empty pool -- never
/// sees. Counting it here keeps the offline rates honest rather than penalizing a correct abstain.
fn decides_abstain(q: &CalQuery, cfg: &AbstainConfig) -> bool {
    if q.sem_top.is_none() && q.ce_top.is_none() {
        return true;
    }
    evaluate_signals(q.sem_top, q.ce_top, None, q.qt, cfg).abstain
}

/// Returns (false_abstain_rate, true_abstain_rate) for the queries at one threshold.
fn rates(queries: &[CalQuery], threshold: f64) -> (f64, f64) {
    let cfg = sweep_config(threshold);
    let (mut answerable, mut unanswerable) = (0u32, 0u32);
    let (mut false_abstain, mut true_abstain) = (0u32, 0u32);
    for q in queries {
        let abstain = decides_abstain(q, &cfg);
        if q.answerable {
            answerable += 1;
            if abstain {
                false_abstain += 1;
            }
        } else {
            unanswerable += 1;
            if abstain {
                true_abstain += 1;
            }
        }
    }
    assert!(
        answerable > 0 && unanswerable > 0,
        "fixture needs both classes"
    );
    (
        false_abstain as f64 / answerable as f64,
        true_abstain as f64 / unanswerable as f64,
    )
}

/// Sweep the production gate logic across thresholds over the labelled fixture and assert the
/// FAR/TAR targets hold at the shipped default. Signals are LIVE captures (2026-06-28) from a
/// bge-m3 + bge-reranker-v2-m3 scratch server, with `ce_rescue` recalibrated to 0.70 for that
/// reranker (see `sweep_config`).
#[test]
fn abstain_threshold_meets_far_tar_targets() {
    let queries = load_queries();

    println!("\n=== ABSTAIN calibration sweep (gate logic = production evaluate_signals) ===");
    println!("{:>8} {:>8} {:>8}", "thresh", "FAR", "TAR");
    for t in [0.55_f64, 0.60, 0.65, 0.70, 0.75] {
        let (far, tar) = rates(&queries, t);
        let mark = if (t - DEFAULT_THRESHOLD).abs() < 1e-9 {
            "  <- default"
        } else {
            ""
        };
        println!("{t:>8.2} {far:>8.3} {tar:>8.3}{mark}");
    }

    let (far, tar) = rates(&queries, DEFAULT_THRESHOLD);
    println!("CAL_SUMMARY {{\"threshold\":{DEFAULT_THRESHOLD},\"far\":{far:.4},\"tar\":{tar:.4}}}");
    println!(
        "NOTE: FAR/TAR are measured against LIVE bge-m3 + bge-reranker-v2-m3 captures -- \
         re-capture per-query sem_top/ce_top before freezing for a different embedder/reranker."
    );

    // Per-query diagnostics at the default threshold: name the offenders so a failed freeze
    // points at exactly which queries to inspect or re-capture.
    let cfg = sweep_config(DEFAULT_THRESHOLD);
    let mut false_abstains: Vec<&str> = Vec::new();
    let mut missed_abstains: Vec<&str> = Vec::new();
    for q in &queries {
        let abstain = decides_abstain(q, &cfg);
        if q.answerable && abstain {
            false_abstains.push(q.id.as_str());
        } else if !q.answerable && !abstain {
            missed_abstains.push(q.id.as_str());
        }
    }
    if !false_abstains.is_empty() {
        println!("false-abstains @{DEFAULT_THRESHOLD}: {false_abstains:?}");
    }
    if !missed_abstains.is_empty() {
        println!("missed-abstains @{DEFAULT_THRESHOLD}: {missed_abstains:?}");
    }

    assert!(
        far < 0.05,
        "false-abstain rate {far:.3} >= 0.05 at default threshold {DEFAULT_THRESHOLD} (answerable queries are being wrongly abstained)"
    );
    assert!(
        tar > 0.80,
        "true-abstain rate {tar:.3} <= 0.80 at default threshold {DEFAULT_THRESHOLD} (unanswerable queries are slipping through as fabrications)"
    );
}
