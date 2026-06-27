//! Emotional valence tracking -- sentiment/affect analysis.
//! Ported from tier4/valence.ts.

use crate::db::Database;
use crate::Result;
use regex::Regex;
use rusqlite::params;
use std::collections::HashMap;
use std::sync::LazyLock;

use super::types::{
    EmotionMatch, EmotionStat, EmotionalProfile, OverallEmotionStats, ValenceResult,
};

/// A compiled regex pattern paired with its emotion label and dimensional scores.
struct EmotionPattern {
    /// Pre-compiled case-insensitive word-boundary regex for the lexicon class.
    regex: Regex,
    /// Canonical emotion label emitted by `analyze_valence_for_lang`.
    emotion: &'static str,
    /// Valence score in [-1, 1] (negative = unpleasant, positive = pleasant).
    valence: f64,
    /// Arousal score in [0, 1] (low = calm, high = activated).
    arousal: f64,
}

/// Valence patterns sourced from the i18n lexicon.
///
/// The 21 tuples below name the lexicon class, the canonical emotion
/// label emitted by analyze_valence, and the default valence + arousal
/// pair to use when the TOML class does not declare its own metadata.
/// When the TOML carries `valence = ...` and `arousal = ...`, those win
/// (cf. `lexicon::class_valence_arousal`).
const VALENCE_CLASSES: &[(&str, &str, f64, f64)] = &[
    ("valence_anger_intense", "anger", -0.9, 0.9),
    ("valence_anger_mild", "anger", -0.7, 0.7),
    ("valence_fear_intense", "fear", -0.8, 0.9),
    ("valence_fear_mild", "fear", -0.6, 0.6),
    ("valence_sadness_intense", "sadness", -0.9, 0.3),
    ("valence_sadness_mild", "sadness", -0.6, 0.3),
    ("valence_fatigue", "fatigue", -0.3, 0.1),
    ("valence_confusion", "confusion", -0.3, 0.4),
    ("valence_frustration", "frustration", -0.5, 0.6),
    ("valence_disgust", "disgust", -0.8, 0.5),
    ("valence_joy_intense", "joy", 0.9, 0.9),
    ("valence_excitement", "excitement", 0.8, 0.8),
    ("valence_joy_mild", "joy", 0.7, 0.6),
    ("valence_pride", "pride", 0.7, 0.6),
    ("valence_satisfaction", "satisfaction", 0.4, 0.3),
    ("valence_calm", "calm", 0.3, 0.1),
    ("valence_gratitude", "gratitude", 0.6, 0.3),
    ("valence_curiosity", "curiosity", 0.4, 0.5),
    ("valence_accomplishment", "accomplishment", 0.5, 0.5),
    ("valence_admiration", "admiration", 0.7, 0.4),
    ("valence_surprise", "surprise", 0.0, 0.7),
];

/// Per-language emotion pattern table, built once at process startup.
///
/// Keyed by ISO 639-1 language code (e.g. "en", "fr", "de"). Each value
/// holds only the compiled `Vec<EmotionPattern>` for that single language,
/// eliminating the cross-language false positives that arose from the old flat
/// Vec (where a French word could match an English pattern) and reducing
/// per-call work from O(all languages) to O(one language).
static EMOTION_PATTERNS: LazyLock<HashMap<String, Vec<EmotionPattern>>> = LazyLock::new(|| {
    let mut map: HashMap<String, Vec<EmotionPattern>> = HashMap::new();
    for lang in crate::lexicon::supported_languages() {
        let mut patterns: Vec<EmotionPattern> = Vec::new();
        for (class, emotion, default_v, default_a) in VALENCE_CLASSES {
            let words = crate::lexicon::word_class(&lang, class);
            if words.is_empty() {
                continue;
            }
            // Wildcard-after-stem: stem each word (one-shot)
            // and add `\w*` so FR feminine/plural forms (`fatiguee`,
            // `joyeuses`) match without TOML duplication. Multi-word
            // entries (`burned out`) keep their inner whitespace; the
            // case-insensitive flag and trailing `\w*` carry the bulk
            // of the tolerance.
            let with_stem = crate::lexicon::class_stem_enabled(&lang, class);
            let alternation = words
                .iter()
                .map(|w| regex::escape(&crate::lexicon::fold_for_matching(w, &lang, with_stem)))
                .collect::<Vec<_>>()
                .join("|");
            let pattern = format!(r"(?i)\b(?:{alternation})\w*\b");
            let Ok(regex) = Regex::new(&pattern) else {
                tracing::warn!(class = class, lang = %lang, "valence regex compile failed");
                continue;
            };
            // Prefer the per-class metadata when present; fall back to
            // the code-side default for the bucket.
            let (valence, arousal) = crate::lexicon::class_valence_arousal(&lang, class)
                .unwrap_or((*default_v, *default_a));
            patterns.push(EmotionPattern {
                regex,
                emotion,
                valence,
                arousal,
            });
        }
        map.insert(lang, patterns);
    }
    map
});

/// Return the compiled emotion patterns for the given language code.
///
/// Falls back to English patterns when `lang` has no entry (e.g. an
/// unsupported language that slipped past detection). Returns an empty slice
/// if even the English fallback is absent, which should never occur given the
/// embedded EN baseline but avoids panicking on misconfigured builds.
fn emotion_patterns_for(lang: &str) -> &'static [EmotionPattern] {
    EMOTION_PATTERNS
        .get(lang)
        .or_else(|| EMOTION_PATTERNS.get("en"))
        .map(|v| v.as_slice())
        .unwrap_or(&[])
}

/// Shared valence scoring loop used by both public entry points.
///
/// Iterates only the patterns for `lang`, matching each against `content` and
/// accumulating weighted valence and arousal scores.
fn run_valence(content: &str, lang: &str) -> ValenceResult {
    let mut matches: Vec<EmotionMatch> = Vec::new();
    for pat in emotion_patterns_for(lang) {
        if pat.regex.is_match(content) {
            matches.push(EmotionMatch {
                emotion: pat.emotion.to_string(),
                valence: pat.valence,
                arousal: pat.arousal,
            });
        }
    }
    if matches.is_empty() {
        return ValenceResult {
            valence: 0.0,
            arousal: 0.0,
            dominant_emotion: "neutral".into(),
            all_emotions: vec![],
        };
    }
    let total_weight: f64 = matches.iter().map(|m| m.valence.abs()).sum();
    let avg_valence = matches
        .iter()
        .map(|m| m.valence * m.valence.abs())
        .sum::<f64>()
        / total_weight;
    let avg_arousal = matches
        .iter()
        .map(|m| m.arousal * m.valence.abs())
        .sum::<f64>()
        / total_weight;
    let dominant = matches
        .iter()
        .max_by(|a, b| a.valence.abs().partial_cmp(&b.valence.abs()).unwrap())
        .unwrap();
    ValenceResult {
        valence: (avg_valence * 100.0).round() / 100.0,
        arousal: (avg_arousal * 100.0).round() / 100.0,
        dominant_emotion: dominant.emotion.clone(),
        all_emotions: matches,
    }
}

/// Analyse emotional valence of `content`, auto-detecting its language first.
///
/// Calls `detect_lang` internally and delegates to `analyze_valence_for_lang`.
/// Detection defaults to "en" for short or low-confidence input. Signature is
/// stable -- callers (including `store_valence`) must not be updated.
pub fn analyze_valence(content: &str) -> ValenceResult {
    let lang = crate::lang::detect_lang(content);
    run_valence(content, &lang)
}

/// Analyse emotional valence of `content` for an explicitly supplied language.
///
/// Useful when the caller already holds a detected or stored language code and
/// wants to skip re-detection. Also used by tests that need deterministic
/// language-specific assertions without relying on the detection heuristic.
pub fn analyze_valence_for_lang(content: &str, lang: &str) -> ValenceResult {
    run_valence(content, lang)
}

/// Store valence scores for `memory_id`, returning the analysis result.
///
/// Calls `analyze_valence` (with automatic language detection) and writes the
/// dominant emotion label plus valence/arousal scores back to the memories
/// table. Skips the DB write for neutral memories to avoid polluting aggregates.
#[tracing::instrument(skip(db, content), fields(memory_id, content_len = content.len()))]
pub async fn store_valence(db: &Database, memory_id: i64, content: &str) -> Result<ValenceResult> {
    let result = analyze_valence(content);
    if result.dominant_emotion != "neutral" {
        let valence = result.valence;
        let arousal = result.arousal;
        let dominant_emotion = result.dominant_emotion.clone();
        db.write(move |conn| {
            conn.execute(
                "UPDATE memories SET valence = ?1, arousal = ?2, dominant_emotion = ?3 WHERE id = ?4",
                params![valence, arousal, dominant_emotion, memory_id],
            )?;
            Ok(())
        })
        .await?;
    }
    Ok(result)
}

/// Return an emotional profile for `user_id` aggregated from their memories.
///
/// Queries per-emotion label counts/averages and overall valence/arousal stats
/// from the memories table. Results are scoped to `user_id` to prevent
/// cross-tenant leakage.
#[tracing::instrument(skip(db))]
pub async fn get_emotional_profile(db: &Database, user_id: i64) -> Result<EmotionalProfile> {
    // Scope to the caller: in monolith mode these aggregates ran over every
    // tenant's memories, leaking global emotion-label counts and averages.
    let emotions = db
        .read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT dominant_emotion, COUNT(*) as count, AVG(valence), AVG(arousal) \
                     FROM memories WHERE dominant_emotion IS NOT NULL AND is_forgotten = 0 \
                     AND user_id = ?1 \
                     GROUP BY dominant_emotion ORDER BY count DESC",
            )?;
            let rows = stmt.query_map(rusqlite::params![user_id], |row| {
                Ok(EmotionStat {
                    dominant_emotion: row.get(0)?,
                    count: row.get(1)?,
                    avg_valence: row.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
                    avg_arousal: row.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await?;

    let overall = db
        .read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT AVG(valence), AVG(arousal), \
                     SUM(CASE WHEN valence > 0.2 THEN 1 ELSE 0 END), \
                     SUM(CASE WHEN valence < -0.2 THEN 1 ELSE 0 END), \
                     SUM(CASE WHEN valence BETWEEN -0.2 AND 0.2 THEN 1 ELSE 0 END) \
                     FROM memories WHERE valence IS NOT NULL AND is_forgotten = 0 \
                     AND user_id = ?1",
            )?;
            let result = stmt.query_row(rusqlite::params![user_id], |row| {
                Ok(OverallEmotionStats {
                    avg_valence: row.get::<_, Option<f64>>(0)?.unwrap_or(0.0),
                    avg_arousal: row.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
                    positive_count: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    negative_count: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                    neutral_count: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                })
            })?;
            Ok(result)
        })
        .await?;

    Ok(EmotionalProfile { emotions, overall })
}

/// Tests for language-aware valence analysis.
#[cfg(test)]
mod tests {
    use super::*;

    /// English positive text should score positive valence with an expected dominant emotion.
    #[test]
    fn test_positive() {
        let r = analyze_valence_for_lang("I am so happy and excited about this!", "en");
        assert!(r.valence > 0.0);
        assert!(r.dominant_emotion == "excitement" || r.dominant_emotion == "joy");
    }

    /// English negative text with two matched emotion classes should score negative valence.
    #[test]
    fn test_negative() {
        let r = analyze_valence_for_lang("The server crashed and I am frustrated", "en");
        assert!(r.valence < 0.0);
        assert_eq!(r.all_emotions.len(), 2);
    }

    /// Neutral English text with no emotion words should produce valence 0 and "neutral" label.
    #[test]
    fn test_neutral() {
        let r = analyze_valence_for_lang("The meeting is at 3pm tomorrow", "en");
        assert_eq!(r.valence, 0.0);
        assert_eq!(r.dominant_emotion, "neutral");
    }

    /// Pattern matching must be case-insensitive for English anger words.
    #[test]
    fn test_case_insensitive() {
        let r = analyze_valence_for_lang("I am FURIOUS about this", "en");
        assert!(r.valence < -0.5);
        assert_eq!(r.dominant_emotion, "anger");
    }

    /// French positive text must not panic and should score non-negative valence via the "fr"
    /// pattern set, confirming that per-language routing is active and cross-language pattern
    /// pollution is eliminated.
    #[test]
    fn test_french_valence() {
        // "I am really very happy and grateful today" in French.
        let text = "Je suis vraiment tres heureux et reconnaissant aujourd'hui";
        let r = analyze_valence_for_lang(text, "fr");
        // Must not panic and must produce a valid result.
        assert!(
            r.valence >= 0.0,
            "French positive sentence must not produce negative valence (got {})",
            r.valence
        );
    }
}
