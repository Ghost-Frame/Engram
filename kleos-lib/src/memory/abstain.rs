//! L2 ABSTAIN gate -- "insufficient evidence" instead of a confidently-wrong memory.
//!
//! Today the search route only abstains when the result set is *empty*: any memory
//! that survives filtering is returned as an answer regardless of how weak the match
//! is. An agent-in-the-loop evaluation showed this is a live fabrication risk -- on
//! unanswerable queries the system returned an irrelevant memory at a middling score
//! with `abstained=false`. That same evaluation found a clean separation on the raw
//! cosine `semantic_score`: answerable top hits sat at ~0.71-0.78, unanswerable at
//! ~0.45-0.62. This gate turns that empirical separation into a decision.
//!
//! ## Design (semantic-primary)
//!
//! - **Primary signal: `semantic_score`** (raw cosine in [0,1]) -- the signal the
//!   evidence actually measured. We gate on the *best* semantic score across the whole
//!   returned pool, never `results[0]`, because the reranker sorts by a *blended* score
//!   so the max-semantic row is not necessarily first.
//! - **Cross-encoder confidence (`ce_confidence`) is a supplementary tightener**: a
//!   strong CE match *vetoes* an abstain (CE rescue). CE never *causes* an abstain on
//!   its own at the defaults -- the CE scale is not a calibrated probability, so an
//!   operator must set a calibrated `KLEOS_ABSTAIN_CE_FLOOR` to enable that path.
//! - **The compound `score` (RRF x decay x pagerank x recency) is never used** as a
//!   confidence -- it is not one.
//! - **Margin is AND-only and opt-in** (`KLEOS_ABSTAIN_MARGIN`, default 0 = off). It
//!   only tightens toward abstain in the borderline band just above the threshold, so
//!   two strong near-duplicate (versioned) memories with a tiny gap never trigger a
//!   false abstain.
//! - **No-signal path** (FTS-only / sqlite-vec fallback, `semantic_score=None` and no
//!   reranker): do not false-abstain -- return `signal="no_signal"` so operators can
//!   see the gate was inert rather than believing it ran.
//!
//! The gate is **default-off** (`KLEOS_ABSTAIN_ENABLED=false`); when disabled it is a
//! pure no-op and the caller's behavior is byte-identical to before this module existed.
//!
//! ## Calibration & provenance
//!
//! The default thresholds are starting points, not laws. They are operating points on a
//! specific embedder + reranker (bge-m3 + Granite); a model swap or a scoring change
//! shifts the score distribution and requires re-running the
//! calibration harness (`tests/abstain_calibration.rs`) and re-freezing the defaults.
//! Every threshold is overridable at runtime via env so re-tuning needs no rebuild.

use crate::memory::types::{QuestionType, SearchResult};
use std::sync::LazyLock;

/// Master switch. Default `false`: the gate is a no-op and behavior is unchanged.
static ENABLED: LazyLock<bool> = LazyLock::new(|| {
    std::env::var("KLEOS_ABSTAIN_ENABLED")
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
});

/// Default primary semantic-cosine threshold. Below this (and absent a CE rescue) the
/// gate abstains. 0.65 sits in the empirically-observed gap between answerable
/// (~0.71-0.78) and unanswerable (~0.45-0.62) top hits. Clamped to [0, 1].
const DEFAULT_SEMANTIC: f64 = 0.65;
static SEMANTIC: LazyLock<f64> =
    LazyLock::new(|| env_f64("KLEOS_ABSTAIN_SEMANTIC", DEFAULT_SEMANTIC, 0.0, 1.0));

/// FactRecall threshold reduction. FactRecall false-abstains are the worst outcome (the
/// user knows the fact is in the store), so its threshold is `semantic - fact_relax`,
/// i.e. slightly more permissive. Default 0.0 (no per-type split until calibrated).
const DEFAULT_FACT_RELAX: f64 = 0.0;
static FACT_RELAX: LazyLock<f64> =
    LazyLock::new(|| env_f64("KLEOS_ABSTAIN_FACT_RELAX", DEFAULT_FACT_RELAX, 0.0, 0.5));

/// Cross-encoder rescue floor. A best `ce_confidence >= ce_rescue` vetoes any abstain:
/// a strong query x document relevance overrides a middling cosine. Default 0.70,
/// recalibrated 2026-06-28 for the bge-reranker-v2-m3 reranker (was 0.80, frozen against
/// granite). That model's CE distribution is sharply bimodal -- relevant pairs ~0.98-1.0,
/// irrelevant ~0.00 -- so a legitimately-rescuable boundary case captured at ce=0.752 fell
/// below the old 0.80 floor and was false-abstained; 0.70 sits in the valley below it.
const DEFAULT_CE_RESCUE: f64 = 0.70;
static CE_RESCUE: LazyLock<f64> =
    LazyLock::new(|| env_f64("KLEOS_ABSTAIN_CE_RESCUE", DEFAULT_CE_RESCUE, 0.0, 1.0));

/// Optional cross-encoder abstain floor for the semantic-absent path. Default 0.0
/// (disabled) so CE never causes an abstain on its own until an operator calibrates it
/// against their deployed reranker. Clamped to [0, 1].
const DEFAULT_CE_FLOOR: f64 = 0.0;
static CE_FLOOR: LazyLock<f64> =
    LazyLock::new(|| env_f64("KLEOS_ABSTAIN_CE_FLOOR", DEFAULT_CE_FLOOR, 0.0, 1.0));

/// AND-margin tightener band. Default 0.0 (disabled). When > 0, the gate abstains on an
/// above-threshold top hit ONLY when it is within `margin` of the threshold AND the
/// top-1/top-2 semantic gap is below `margin` (ambiguous, no clear winner). Clamped [0,1].
const DEFAULT_MARGIN: f64 = 0.0;
static MARGIN: LazyLock<f64> =
    LazyLock::new(|| env_f64("KLEOS_ABSTAIN_MARGIN", DEFAULT_MARGIN, 0.0, 1.0));

/// Model baseline the default thresholds were frozen against. A different embedder or
/// reranker shifts the score distribution and silently invalidates the thresholds, so the
/// gate warns once when it is enabled against a non-baseline reranker.
const CALIBRATION_BASELINE: &str = "bge-m3 (embed) + bge-reranker-v2-m3 (rerank, onnx)";

/// One-shot provenance guard. When the gate is enabled and a non-baseline reranker is
/// configured (any HTTP/TEI/Cohere backend, or a non-default `KLEOS_RERANKER_MODEL`), warn
/// exactly once that the abstain thresholds must be recalibrated. Initialized lazily and
/// forced from `AbstainConfig::from_env`, so the warning fires at most once per process.
static CALIBRATION_GUARD: LazyLock<()> = LazyLock::new(|| {
    if !*ENABLED {
        return;
    }
    let backend = std::env::var("KLEOS_RERANKER_BACKEND")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let model = std::env::var("KLEOS_RERANKER_MODEL").unwrap_or_default();
    let non_baseline = matches!(backend.as_str(), "http" | "tei" | "cohere") || !model.is_empty();
    if non_baseline {
        tracing::warn!(
            baseline = CALIBRATION_BASELINE,
            reranker_backend = %backend,
            reranker_model = %model,
            "KLEOS_ABSTAIN enabled with a non-baseline reranker; abstain thresholds were \
             calibrated for the baseline and must be recalibrated (tests/abstain_calibration.rs) \
             or abstain decisions are uncalibrated"
        );
    }
});

/// Parse an env var as f64, clamped to [lo, hi]; fall back to `default` on absence/parse error.
fn env_f64(var: &str, default: f64, lo: f64, hi: f64) -> f64 {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .map(|v| v.clamp(lo, hi))
        .unwrap_or(default)
}

/// Resolved abstain configuration. Built from env (`from_env`) in production; constructed
/// directly in tests and in the calibration harness so thresholds can be swept.
#[derive(Debug, Clone, Copy)]
pub struct AbstainConfig {
    /// Master switch; `false` makes `abstain_gate` a no-op.
    pub enabled: bool,
    /// Primary semantic-cosine threshold.
    pub semantic: f64,
    /// FactRecall threshold reduction (more permissive for the highest-value type).
    pub fact_relax: f64,
    /// CE score at/above which an abstain is vetoed.
    pub ce_rescue: f64,
    /// CE score below which the gate abstains when no semantic signal exists (0 = off).
    pub ce_floor: f64,
    /// AND-margin tightener band (0 = off).
    pub margin: f64,
}

/// Env-resolution and construction for the abstain gate's tuned thresholds.
impl AbstainConfig {
    /// Resolve the configuration from `KLEOS_ABSTAIN_*` env vars (read once via LazyLock).
    pub fn from_env() -> Self {
        // Fire the one-shot model-provenance warning if the gate is enabled against a
        // non-baseline reranker. Cheap after first call (LazyLock is an atomic load).
        LazyLock::force(&CALIBRATION_GUARD);
        Self {
            enabled: *ENABLED,
            semantic: *SEMANTIC,
            fact_relax: *FACT_RELAX,
            ce_rescue: *CE_RESCUE,
            ce_floor: *CE_FLOOR,
            margin: *MARGIN,
        }
    }
}

/// The gate's verdict for one query, plus the signals it reasoned over (for the response
/// envelope, calibration analytics, and operator debugging).
#[derive(Debug, Clone)]
pub struct AbstainDecision {
    /// Whether the caller should withhold an answer ("insufficient evidence").
    pub abstain: bool,
    /// Human-readable reason; `None` when `abstain == false`.
    pub reason: Option<String>,
    /// Which path produced the verdict: `disabled`, `empty`, `no_signal`, `ce_rescue`,
    /// `semantic`, or `ce`.
    pub signal: &'static str,
    /// Best semantic_score across the returned pool (the primary signal), if any.
    pub sem_top: Option<f64>,
    /// Best ce_confidence across the returned pool, if a reranker ran.
    pub ce_top: Option<f64>,
    /// Top-1 minus top-2 semantic_score gap, if at least two semantic-scored results.
    pub margin: Option<f64>,
}

/// Per-question-type semantic threshold. FactRecall gets a `fact_relax` discount because a
/// false abstain there (the user knows the fact exists) is the costliest error.
fn semantic_threshold(qt: QuestionType, cfg: &AbstainConfig) -> f64 {
    match qt {
        QuestionType::FactRecall => (cfg.semantic - cfg.fact_relax).max(0.0),
        _ => cfg.semantic,
    }
}

/// Max of a stream of scores, or `None` if empty. NaN-tolerant (scores are never NaN here).
fn max_opt(it: impl Iterator<Item = f64>) -> Option<f64> {
    it.fold(None, |acc, x| match acc {
        Some(m) if m >= x => Some(m),
        _ => Some(x),
    })
}

/// Top-1 minus top-2 of a score set, or `None` when fewer than two scores exist.
fn top_two_margin(mut vals: Vec<f64>) -> Option<f64> {
    if vals.len() < 2 {
        return None;
    }
    vals.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    Some(vals[0] - vals[1])
}

/// The pure decision core: given the best signals already extracted from the result pool,
/// decide whether to abstain. Separated from [`abstain_gate`] so every branch is unit-tested
/// without constructing `SearchResult`/`Memory`, and exposed publicly so the calibration
/// harness can sweep thresholds over captured `(semantic, ce)` pairs using the exact
/// production decision logic rather than a reimplementation.
///
/// - `best_sem`: best `semantic_score` across the pool (the primary signal).
/// - `best_ce`: best `ce_confidence` across the pool (supplementary; `None` if no reranker).
/// - `margin`: top-1 minus top-2 `semantic_score`, or `None` with fewer than two scores.
pub fn evaluate_signals(
    best_sem: Option<f64>,
    best_ce: Option<f64>,
    margin: Option<f64>,
    qt: QuestionType,
    cfg: &AbstainConfig,
) -> AbstainDecision {
    let make = |abstain: bool, reason: Option<&str>, signal: &'static str| AbstainDecision {
        abstain,
        reason: reason.map(str::to_string),
        signal,
        sem_top: best_sem,
        ce_top: best_ce,
        margin,
    };

    // 1. No signal at all (FTS-only, or sqlite-vec fallback with distance=None, and no
    //    reranker). We cannot judge confidence -- a false non-abstain (showing a possibly
    //    weak FTS hit) is far less harmful than a false abstain. Surface that the gate
    //    was inert rather than passing silently.
    if best_sem.is_none() && best_ce.is_none() {
        return make(false, None, "no_signal");
    }

    // 2. CE rescue: a strong cross-encoder match vetoes any abstain. CE only RESCUES at
    //    the defaults; it never causes an abstain on its own (its scale is not a
    //    calibrated probability) unless step 4's calibrated floor is configured.
    if let Some(ce) = best_ce {
        if ce >= cfg.ce_rescue {
            return make(false, None, "ce_rescue");
        }
    }

    // 3. Semantic-primary absolute threshold -- the empirically-grounded signal.
    if let Some(sem) = best_sem {
        let thresh = semantic_threshold(qt, cfg);
        if sem < thresh {
            return make(true, Some("semantic_below_threshold"), "semantic");
        }
        // AND-margin tightener (opt-in). Fires only in the borderline band just above the
        // threshold AND when the runner-up is nearly tied. A clearly strong top hit
        // (sem >= thresh + margin) never abstains, so near-duplicate/versioned correct
        // memories do not produce a false abstain.
        if cfg.margin > 0.0 {
            if let Some(m) = margin {
                if sem < thresh + cfg.margin && m < cfg.margin {
                    return make(true, Some("ambiguous_narrow_margin"), "semantic");
                }
            }
        }
        return make(false, None, "semantic");
    }

    // 4. Only a (below-rescue) CE signal exists, no semantic. Abstain only when an
    //    operator has set a calibrated floor; otherwise answer.
    if let Some(ce) = best_ce {
        if cfg.ce_floor > 0.0 && ce < cfg.ce_floor {
            return make(true, Some("ce_below_floor"), "ce");
        }
        return make(false, None, "ce");
    }

    // Unreachable: step 1 covers the both-None case.
    make(false, None, "no_signal")
}

/// Evaluate whether a retrieval result set justifies an answer.
///
/// `results` is the post-rerank, post-truncate slice exactly as a caller receives it.
/// Returns an [`AbstainDecision`]; pure and O(k). When `cfg.enabled` is false this is a
/// no-op returning `abstain=false`.
pub fn abstain_gate(
    results: &[SearchResult],
    qt: QuestionType,
    cfg: &AbstainConfig,
) -> AbstainDecision {
    if !cfg.enabled {
        return AbstainDecision {
            abstain: false,
            reason: None,
            signal: "disabled",
            sem_top: None,
            ce_top: None,
            margin: None,
        };
    }
    if results.is_empty() {
        return AbstainDecision {
            abstain: true,
            reason: Some("empty_result_set".to_string()),
            signal: "empty",
            sem_top: None,
            ce_top: None,
            margin: None,
        };
    }

    // Best signals over the WHOLE pool, not results[0]: the post-rerank sort is by the
    // blended score, so the max-semantic / max-CE row is not necessarily first.
    let sems: Vec<f64> = results.iter().filter_map(|r| r.semantic_score).collect();
    let best_sem = max_opt(sems.iter().copied());
    let best_ce = max_opt(results.iter().filter_map(|r| r.ce_confidence));
    let margin = top_two_margin(sems);

    evaluate_signals(best_sem, best_ce, margin, qt, cfg)
}

#[cfg(test)]
/// Unit tests for the abstain decision core and the gate envelope.
mod tests {
    use super::*;

    /// Config with the gate enabled and the given knobs; per-type relax and CE floor off.
    fn cfg(semantic: f64, ce_rescue: f64, margin: f64) -> AbstainConfig {
        AbstainConfig {
            enabled: true,
            semantic,
            fact_relax: 0.0,
            ce_rescue,
            ce_floor: 0.0,
            margin,
        }
    }

    /// True when two floats are within a tight epsilon -- for threshold assertions.
    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    // ---- helper functions -------------------------------------------------

    #[test]
    fn max_opt_empty_is_none() {
        assert_eq!(max_opt(std::iter::empty()), None);
    }

    /// max_opt returns the maximum regardless of position -- this is what lets the gate
    /// read the best signal across the pool rather than the (blended-sorted) results[0].
    #[test]
    fn max_opt_returns_pool_max_not_first() {
        assert_eq!(max_opt([0.2, 0.9, 0.5].into_iter()), Some(0.9));
    }

    /// `top_two_margin` needs two scores: a single score yields None, three give the top gap.
    #[test]
    fn margin_needs_two_scores() {
        assert_eq!(top_two_margin(vec![0.8]), None);
        let m = top_two_margin(vec![0.75, 0.9, 0.8]).unwrap();
        assert!(approx(m, 0.1), "got {m}");
    }

    // ---- gate envelope ----------------------------------------------------

    #[test]
    fn disabled_is_noop() {
        let c = AbstainConfig {
            enabled: false,
            ..cfg(0.65, 0.8, 0.0)
        };
        let d = abstain_gate(&[], QuestionType::FactRecall, &c);
        assert!(!d.abstain);
        assert_eq!(d.signal, "disabled");
    }

    /// An empty result pool always abstains -- the historical empty-set rule.
    #[test]
    fn empty_results_abstain() {
        let d = abstain_gate(&[], QuestionType::FactRecall, &cfg(0.65, 0.8, 0.0));
        assert!(d.abstain);
        assert_eq!(d.signal, "empty");
    }

    // ---- decision core ----------------------------------------------------

    #[test]
    fn no_signal_does_not_abstain() {
        let d = evaluate_signals(
            None,
            None,
            None,
            QuestionType::FactRecall,
            &cfg(0.65, 0.8, 0.0),
        );
        assert!(!d.abstain, "no signal must not false-abstain");
        assert_eq!(d.signal, "no_signal");
    }

    /// A sub-threshold semantic score with no CE rescue abstains on `semantic`.
    #[test]
    fn semantic_below_threshold_abstains() {
        let d = evaluate_signals(
            Some(0.60),
            None,
            None,
            QuestionType::FactRecall,
            &cfg(0.65, 0.8, 0.0),
        );
        assert!(d.abstain);
        assert_eq!(d.signal, "semantic");
        assert_eq!(d.reason.as_deref(), Some("semantic_below_threshold"));
    }

    /// An above-threshold semantic score answers -- no abstain.
    #[test]
    fn semantic_above_threshold_answers() {
        let d = evaluate_signals(
            Some(0.72),
            None,
            None,
            QuestionType::FactRecall,
            &cfg(0.65, 0.8, 0.0),
        );
        assert!(!d.abstain);
        assert_eq!(d.signal, "semantic");
    }

    /// A strong cross-encoder match vetoes an abstain even when the cosine is sub-threshold.
    #[test]
    fn ce_rescue_overrides_low_semantic() {
        let d = evaluate_signals(
            Some(0.50),
            Some(0.92),
            None,
            QuestionType::Preference,
            &cfg(0.65, 0.8, 0.0),
        );
        assert!(!d.abstain);
        assert_eq!(d.signal, "ce_rescue");
    }

    /// Two strong near-duplicate (versioned) memories: tiny margin but high absolute score.
    /// Must NOT abstain -- this is the H-3 false-abstain regression guard.
    #[test]
    fn strong_near_duplicates_do_not_abstain_on_margin() {
        let d = evaluate_signals(
            Some(0.90),
            None,
            Some(0.01),
            QuestionType::FactRecall,
            &cfg(0.65, 0.8, 0.05),
        );
        assert!(
            !d.abstain,
            "strong top hit must answer despite a tiny runner-up gap"
        );
    }

    /// Borderline top hit just above threshold with a near-tied runner-up: margin tightener fires.
    #[test]
    fn borderline_ambiguous_pair_abstains_when_margin_enabled() {
        let d = evaluate_signals(
            Some(0.66),
            None,
            Some(0.01),
            QuestionType::FactRecall,
            &cfg(0.65, 0.8, 0.05),
        );
        assert!(d.abstain);
        assert_eq!(d.reason.as_deref(), Some("ambiguous_narrow_margin"));
    }

    /// With margin disabled (default), a borderline ambiguous pair still answers.
    #[test]
    fn margin_disabled_answers_borderline() {
        let d = evaluate_signals(
            Some(0.66),
            None,
            Some(0.01),
            QuestionType::FactRecall,
            &cfg(0.65, 0.8, 0.0),
        );
        assert!(!d.abstain);
    }

    /// CE never causes an abstain on its own at the defaults (ce_floor = 0).
    #[test]
    fn ce_only_below_rescue_answers_without_floor() {
        let d = evaluate_signals(
            None,
            Some(0.40),
            None,
            QuestionType::Reasoning,
            &cfg(0.65, 0.8, 0.0),
        );
        assert!(!d.abstain);
        assert_eq!(d.signal, "ce");
    }

    /// With a calibrated CE floor configured, a low CE-only result abstains.
    #[test]
    fn ce_floor_abstains_when_configured() {
        let c = AbstainConfig {
            ce_floor: 0.30,
            ..cfg(0.65, 0.8, 0.0)
        };
        let d = evaluate_signals(None, Some(0.20), None, QuestionType::Reasoning, &c);
        assert!(d.abstain);
        assert_eq!(d.reason.as_deref(), Some("ce_below_floor"));
    }

    /// FactRecall's relax makes it more permissive than other types at the same cosine.
    #[test]
    fn fact_relax_is_more_permissive_for_factrecall() {
        let c = AbstainConfig {
            fact_relax: 0.10,
            ..cfg(0.65, 0.8, 0.0)
        };
        // sem 0.58: below the 0.65 default, but >= FactRecall's relaxed 0.55.
        let fact = evaluate_signals(Some(0.58), None, None, QuestionType::FactRecall, &c);
        let pref = evaluate_signals(Some(0.58), None, None, QuestionType::Preference, &c);
        assert!(!fact.abstain, "FactRecall relaxed threshold should answer");
        assert!(pref.abstain, "Preference keeps the stricter threshold");
    }
}
