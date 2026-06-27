// ============================================================================
// PERSONALITY ENGINE -- signal extraction, profile synthesis, caching
// Tier 2 (rule-based NLP) and Tier 3 (template) fallback paths.
// ============================================================================

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::db::Database;
use crate::intelligence::sentiment;
use crate::EngError;
use crate::Result;

// ============================================================================
// Types
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalType {
    Preference,
    Value,
    Motivation,
    Decision,
    Emotion,
    Identity,
}

impl SignalType {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Preference => "preference",
            Self::Value => "value",
            Self::Motivation => "motivation",
            Self::Decision => "decision",
            Self::Emotion => "emotion",
            Self::Identity => "identity",
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Preference => "PREFERENCES",
            Self::Value => "CORE VALUES",
            Self::Motivation => "MOTIVATIONS",
            Self::Decision => "DECISIONS",
            Self::Emotion => "EMOTIONS",
            Self::Identity => "IDENTITY",
        }
    }
}

impl std::fmt::Display for SignalType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Valence {
    Positive,
    Negative,
    Neutral,
    Mixed,
}

impl Valence {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Positive => "positive",
            Self::Negative => "negative",
            Self::Neutral => "neutral",
            Self::Mixed => "mixed",
        }
    }
}

impl std::fmt::Display for Valence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonalitySignal {
    pub signal_type: SignalType,
    pub subject: String,
    pub valence: Valence,
    pub intensity: f64,
    pub reasoning: String,
    pub source_text: String,
}

/// Input data for profile synthesis.
#[derive(Debug, Default)]
pub struct SynthesisInput {
    pub signals: Vec<SignalRow>,
    pub preferences: Vec<PreferenceRow>,
    pub facts: Vec<FactRow>,
    pub static_memories: Vec<StaticMemoryRow>,
}

#[derive(Debug, Clone)]
pub struct SignalRow {
    pub signal_type: String,
    pub subject: String,
    pub valence: String,
    pub intensity: f64,
    pub reasoning: Option<String>,
    pub source_text: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PreferenceRow {
    pub domain: String,
    pub preference: String,
    pub strength: f64,
}

#[derive(Debug, Clone)]
pub struct FactRow {
    pub subject: String,
    pub verb: String,
    pub object: String,
}

#[derive(Debug, Clone)]
pub struct StaticMemoryRow {
    pub content: String,
}

// ============================================================================
// Emotion keywords and intensifiers
// ============================================================================

/// Lexicon class identifiers driving the emotion keyword scan.
///
/// The previous hardcoded EMOTION_KEYWORDS HashMap
/// (English-only, 17 entries) is replaced by an iteration over these
/// classes for every supported language. Each class declares its
/// `valence` (signed: positive emotions > 0, negative < 0) and
/// `intensity` in the TOML lexicon. The Rust side only enumerates
/// the class names so the canonical taxonomy stays stable across
/// languages.
const EMOTION_CLASSES: &[&str] = &[
    "emotion_happy",
    "emotion_excited",
    "emotion_grateful",
    "emotion_proud",
    "emotion_relieved",
    "emotion_thrilled",
    "emotion_content",
    "emotion_sad",
    "emotion_angry",
    "emotion_frustrated",
    "emotion_anxious",
    "emotion_stressed",
    "emotion_disappointed",
    "emotion_overwhelmed",
    "emotion_lonely",
    "emotion_worried",
    "emotion_bored",
];

fn valence_from_signed(signed: f64) -> Valence {
    if signed > 0.0 {
        Valence::Positive
    } else if signed < 0.0 {
        Valence::Negative
    } else {
        Valence::Neutral
    }
}

/// Process-lifetime intensifier word -> multiplier map from the i18n lexicon.
///
/// Built ONCE for the process lifetime (was previously rebuilt per signal
/// inside the rule-based scoring loop). Intentionally a UNION across all
/// supported languages: the downstream fold at the call site uses
/// with_stem=false, so the folded keys (lowercase + diacritic strip only)
/// are language-neutral and collisions across languages are benign. Tier
/// multipliers stay in code because they encode a semantic policy about how
/// strongly each tier modulates the signal; the words themselves live in the
/// lexicon and can be extended per language without recompilation.
static INTENSIFIER_MAP: LazyLock<HashMap<String, f64>> = LazyLock::new(|| {
    // Intensifier tiers and their multipliers (semantic policy in code).
    const TIERS: &[(&str, f64)] = &[
        ("intensifier_weakening", 0.5),
        ("intensifier_softening", 0.6),
        ("intensifier_attenuating", 0.7),
        ("intensifier_strong", 1.3),
        ("intensifier_very_strong", 1.4),
    ];
    let mut map = HashMap::new();
    for lang in crate::lexicon::supported_languages() {
        for (class, mult) in TIERS {
            for word in crate::lexicon::word_class(&lang, class) {
                // Fold the lexicon word (lowercase + strip-accents, stem
                // disabled per the intensifier_* TOML metadata). Then
                // normalise whitespace to underscore so the key matches
                // the tokeniser convention applied downstream
                // (split_whitespace + replace).
                let folded = crate::lexicon::fold_word_for_class(&word, &lang, class);
                let key = folded.replace(' ', "_");
                map.entry(key).or_insert(*mult);
            }
        }
    }
    map
});

/// Valence + intensity for an emotion keyword. Used by the upstream env-var
/// emotion extension (`EXTRA_EMOTION_KEYWORDS`); the i18n lexicon
/// classes carry their own metadata via `class_emotion_metadata`.
struct EmotionMeta {
    valence: Valence,
    intensity: f64,
}

/// Additional emotion keywords loaded from env vars at startup.
/// KLEOS_PERSONALITY_POSITIVE_EXTRA and KLEOS_PERSONALITY_NEGATIVE_EXTRA
/// accept comma-separated keyword lists (e.g. "verliebt,selig"). These are
/// checked in addition to the i18n lexicon emotion classes so an
/// operator can extend the taxonomy without a lexicon overlay edit.
static EXTRA_EMOTION_KEYWORDS: LazyLock<HashMap<String, EmotionMeta>> = LazyLock::new(|| {
    let mut map = HashMap::new();
    if let Ok(v) = std::env::var("KLEOS_PERSONALITY_POSITIVE_EXTRA") {
        for kw in v
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
        {
            map.insert(
                kw,
                EmotionMeta {
                    valence: Valence::Positive,
                    intensity: 0.65,
                },
            );
        }
    }
    if let Ok(v) = std::env::var("KLEOS_PERSONALITY_NEGATIVE_EXTRA") {
        for kw in v
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
        {
            map.insert(
                kw,
                EmotionMeta {
                    valence: Valence::Negative,
                    intensity: 0.65,
                },
            );
        }
    }
    map
});

// ============================================================================
// Regex patterns for signal extraction
// ============================================================================

// Per-language regex helpers replacing the prior
// English-only static LIKE_PATTERN / DISLIKE_PATTERN / etc. Each helper
// reads the relevant lexicon class (verb_like, verb_dislike, etc.) and
// interpolates its words into the surrounding pattern template. The
// helpers fall back to None when the class has no words, so a language
// without coverage is silently skipped at the call site.

fn personality_like_pattern_for(lang: &str) -> Option<Regex> {
    // Wildcard-after-stem: stem the TOML words once and
    // add `\w*` so inflected forms (`j'aime`, `aimerais`, `loved`,
    // `enjoys`) match without TOML duplication. Verb/marker groups stay
    // non-capturing -- cap[1] = object.
    let verbs = crate::lexicon::word_class_alternation_stemmed(lang, "verb_like");
    if verbs.is_empty() {
        return None;
    }
    let pronouns = crate::lexicon::word_class_alternation_stemmed(lang, "first_person_pronoun");
    let pronoun_clause = if pronouns.is_empty() {
        String::new()
    } else {
        format!(r"(?:{pronouns})\w*\s+")
    };
    Regex::new(&format!(
        r"(?i)\b(?:{pronoun_clause})?(?:{verbs})\w*\s+(.+?)(?:\.|,|!|\s+(?:and|but|so|because))"
    ))
    .ok()
}

fn personality_dislike_pattern_for(lang: &str) -> Option<Regex> {
    let verbs = crate::lexicon::word_class_alternation_stemmed(lang, "verb_dislike");
    if verbs.is_empty() {
        return None;
    }
    let pronouns = crate::lexicon::word_class_alternation_stemmed(lang, "first_person_pronoun");
    let pronoun_clause = if pronouns.is_empty() {
        String::new()
    } else {
        format!(r"(?:{pronouns})\w*\s+")
    };
    Regex::new(&format!(
        r"(?i)\b(?:{pronoun_clause})?(?:{verbs})\w*\s+(.+?)(?:\.|,|!|\s+(?:and|but|so|because))"
    ))
    .ok()
}

fn personality_fav_pattern_for(lang: &str) -> Option<Regex> {
    let marker = crate::lexicon::word_class_alternation_stemmed(lang, "favorite_marker");
    let copula = crate::lexicon::word_class_alternation_stemmed(lang, "is_or_are");
    if marker.is_empty() || copula.is_empty() {
        return None;
    }
    // Marker placement varies (EN before, FR after). Use non-capturing
    // groups around the optional marker so cap[1] / cap[2] still mean
    // (category, value) at the call site.
    // Regex fix: wrap each alternation in `(?:...)` BEFORE
    // applying the `\w*` wildcard, otherwise regex priority attaches
    // the suffix only to the last alternative.
    Regex::new(&format!(
        r"(?i)\b(?:my|mon|ma)\s+(?:(?:{marker})\w*\s+)?(.+?)\s+(?:(?:{marker})\w*\s+)?(?:(?:{copula})\w*)\s+(.+?)(?:\.|,|$)"
    ))
    .ok()
}

fn personality_decision_pattern_for(lang: &str) -> Option<Regex> {
    let verbs = crate::lexicon::word_class_alternation_stemmed(lang, "decision_verbs");
    if verbs.is_empty() {
        return None;
    }
    let pronouns = crate::lexicon::word_class_alternation_stemmed(lang, "first_person_pronoun");
    let pronoun_clause = if pronouns.is_empty() {
        String::new()
    } else {
        format!(r"(?:{pronouns})\w*\s+")
    };
    Regex::new(&format!(
        r"(?i)\b(?:{pronoun_clause})?(?:{verbs})\w*\s+(.+?)(?:\.|,|!|$)"
    ))
    .ok()
}

fn personality_identity_pattern_for(lang: &str) -> Option<Regex> {
    let markers = crate::lexicon::word_class_alternation_stemmed(lang, "identity_markers");
    if markers.is_empty() {
        return None;
    }
    let pronouns = crate::lexicon::word_class_alternation_stemmed(lang, "first_person_pronoun");
    let pronoun_clause = if pronouns.is_empty() {
        String::new()
    } else {
        format!(r"(?:{pronouns})\w*\s+")
    };
    Regex::new(&format!(
        r"(?i)\b(?:{pronoun_clause})?(?:{markers})\w*\s+(.+?)(?:\.|,|!|$)"
    ))
    .ok()
}

fn personality_value_pattern_for(lang: &str) -> Option<Regex> {
    let markers = crate::lexicon::word_class_alternation_stemmed(lang, "value_markers");
    if markers.is_empty() {
        return None;
    }
    Regex::new(&format!(r"(?i)\b(?:{markers})\w*\s+(.+?)(?:\.|,|!|$)")).ok()
}

fn personality_motivation_pattern_for(lang: &str) -> Option<Regex> {
    let markers = crate::lexicon::word_class_alternation_stemmed(lang, "motivation_markers");
    if markers.is_empty() {
        return None;
    }
    Regex::new(&format!(r"(?i)\b(?:{markers})\w*\s+(.+?)(?:\.|,|!|$)")).ok()
}

struct PersonalityRegexCache {
    like: HashMap<String, Regex>,
    dislike: HashMap<String, Regex>,
    favorite: HashMap<String, Regex>,
    decision: HashMap<String, Regex>,
    identity: HashMap<String, Regex>,
    value: HashMap<String, Regex>,
    motivation: HashMap<String, Regex>,
}

static PERSONALITY_REGEX: LazyLock<PersonalityRegexCache> = LazyLock::new(|| {
    let mut cache = PersonalityRegexCache {
        like: HashMap::new(),
        dislike: HashMap::new(),
        favorite: HashMap::new(),
        decision: HashMap::new(),
        identity: HashMap::new(),
        value: HashMap::new(),
        motivation: HashMap::new(),
    };
    for lang in crate::lexicon::supported_languages() {
        if let Some(r) = personality_like_pattern_for(&lang) {
            cache.like.insert(lang.clone(), r);
        }
        if let Some(r) = personality_dislike_pattern_for(&lang) {
            cache.dislike.insert(lang.clone(), r);
        }
        if let Some(r) = personality_fav_pattern_for(&lang) {
            cache.favorite.insert(lang.clone(), r);
        }
        if let Some(r) = personality_decision_pattern_for(&lang) {
            cache.decision.insert(lang.clone(), r);
        }
        if let Some(r) = personality_identity_pattern_for(&lang) {
            cache.identity.insert(lang.clone(), r);
        }
        if let Some(r) = personality_value_pattern_for(&lang) {
            cache.value.insert(lang.clone(), r);
        }
        if let Some(r) = personality_motivation_pattern_for(&lang) {
            cache.motivation.insert(lang.clone(), r);
        }
    }
    cache
});

// ============================================================================
// Helper functions
// ============================================================================

/// Clean a captured subject string: trim, remove leading articles, truncate.
///
/// Leading articles are sourced from the i18n lexicon (`articles` class) for
/// the single detected language `lang` only -- the language is detected once
/// at the public boundary and threaded down, so a French capture like
/// "le chien" strips via the French article set while an English capture
/// strips via the English one, without a cross-language union.
fn clean_subject(raw: &str, lang: &str) -> String {
    let trimmed = raw.trim();
    let stripped = crate::lexicon::word_class(lang, "articles")
        .into_iter()
        .find_map(|article| {
            trimmed
                .strip_prefix(&format!("{} ", article.to_lowercase()))
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| trimmed.to_string());
    stripped.chars().take(200).collect()
}

/// Split text into sentences.
fn split_sentences(content: &str) -> Vec<&str> {
    content
        .split(['.', '!', '?', '\n'])
        .map(|s: &str| s.trim())
        .filter(|s| s.len() > 5)
        .collect()
}

// ============================================================================
// Error conversion helper
// ============================================================================

// ============================================================================
// Tier 3 -- Template-based signal extraction
// Pattern match explicit signals only. Quality: ~25%% of LLM.
// ============================================================================

/// Extract personality signals using template-based pattern matching (Tier 3).
///
/// `lang` is the language detected once at the public boundary
/// (`extract_personality_signals` / `detect_signals`) and threaded in. Each of
/// the 5 pattern families looks up that single language's compiled regex,
/// falling back to the "en" entry when the detected language has no pattern,
/// instead of brute-forcing the union of every supported language.
pub fn extract_signals_template(content: &str, lang: &str) -> Vec<PersonalitySignal> {
    let mut signals = Vec::new();
    let sentences = split_sentences(content);

    // A HashSet keyed by (signal_type discriminant, subject) dedup-guards
    // against a single pattern matching the same subject more than once.
    let mut seen_signals: std::collections::HashSet<(u8, String)> =
        std::collections::HashSet::new();

    // Preferences: likes
    if let Some(re) = PERSONALITY_REGEX
        .like
        .get(lang)
        .or_else(|| PERSONALITY_REGEX.like.get("en"))
    {
        for caps in re.captures_iter(content) {
            if let Some(m) = caps.get(1) {
                let subject = clean_subject(m.as_str(), lang);
                if subject.len() < 3 || subject.len() > 100 {
                    continue;
                }
                if !seen_signals.insert((1, subject.clone())) {
                    continue;
                }
                signals.push(PersonalitySignal {
                    signal_type: SignalType::Preference,
                    subject: subject.clone(),
                    valence: Valence::Positive,
                    intensity: 0.6,
                    reasoning: format!("Expressed positive preference about {subject}"),
                    source_text: caps
                        .get(0)
                        .map(|m| m.as_str().trim().to_string())
                        .unwrap_or_default(),
                });
            }
        }
    }

    // Preferences: dislikes
    if let Some(re) = PERSONALITY_REGEX
        .dislike
        .get(lang)
        .or_else(|| PERSONALITY_REGEX.dislike.get("en"))
    {
        for caps in re.captures_iter(content) {
            if let Some(m) = caps.get(1) {
                let subject = clean_subject(m.as_str(), lang);
                if subject.len() < 3 || subject.len() > 100 {
                    continue;
                }
                if !seen_signals.insert((2, subject.clone())) {
                    continue;
                }
                signals.push(PersonalitySignal {
                    signal_type: SignalType::Preference,
                    subject: subject.clone(),
                    valence: Valence::Negative,
                    intensity: 0.6,
                    reasoning: format!("Expressed negative preference about {subject}"),
                    source_text: caps
                        .get(0)
                        .map(|m| m.as_str().trim().to_string())
                        .unwrap_or_default(),
                });
            }
        }
    }

    // Preferences: favorites
    if let Some(re) = PERSONALITY_REGEX
        .favorite
        .get(lang)
        .or_else(|| PERSONALITY_REGEX.favorite.get("en"))
    {
        for caps in re.captures_iter(content) {
            if let (Some(cat), Some(val)) = (caps.get(1), caps.get(2)) {
                let cat_clean = clean_subject(cat.as_str(), lang);
                let val_clean = clean_subject(val.as_str(), lang);
                let key = format!("{cat_clean}: {val_clean}");
                if !seen_signals.insert((3, key.clone())) {
                    continue;
                }
                signals.push(PersonalitySignal {
                    signal_type: SignalType::Preference,
                    subject: key,
                    valence: Valence::Positive,
                    intensity: 0.8,
                    reasoning: format!("Named {val_clean} as favorite {cat_clean}"),
                    source_text: caps
                        .get(0)
                        .map(|m| m.as_str().trim().to_string())
                        .unwrap_or_default(),
                });
            }
        }
    }

    // Decisions
    if let Some(re) = PERSONALITY_REGEX
        .decision
        .get(lang)
        .or_else(|| PERSONALITY_REGEX.decision.get("en"))
    {
        for caps in re.captures_iter(content) {
            if let Some(m) = caps.get(1) {
                let subject = clean_subject(m.as_str(), lang);
                if subject.len() < 3 || subject.len() > 100 {
                    continue;
                }
                if !seen_signals.insert((4, subject.clone())) {
                    continue;
                }
                signals.push(PersonalitySignal {
                    signal_type: SignalType::Decision,
                    subject: subject.clone(),
                    valence: Valence::Neutral,
                    intensity: 0.5,
                    reasoning: format!("Made a decision about {subject}"),
                    source_text: caps
                        .get(0)
                        .map(|m| m.as_str().trim().to_string())
                        .unwrap_or_default(),
                });
            }
        }
    }

    // Identity
    if let Some(re) = PERSONALITY_REGEX
        .identity
        .get(lang)
        .or_else(|| PERSONALITY_REGEX.identity.get("en"))
    {
        for caps in re.captures_iter(content) {
            if let Some(m) = caps.get(1) {
                let subject = clean_subject(m.as_str(), lang);
                if subject.len() < 3 || subject.len() > 100 {
                    continue;
                }
                if !seen_signals.insert((5, subject.clone())) {
                    continue;
                }
                signals.push(PersonalitySignal {
                    signal_type: SignalType::Identity,
                    subject: subject.clone(),
                    valence: Valence::Neutral,
                    intensity: 0.7,
                    reasoning: format!("Self-identified as {subject}"),
                    source_text: caps
                        .get(0)
                        .map(|m| m.as_str().trim().to_string())
                        .unwrap_or_default(),
                });
            }
        }
    }

    // Emotions (keyword scan per sentence) -- fold + normalize. Source and
    // lexicon word are both passed through fold_word_for_class so French
    // expressions like "elle est déçue" match the lexicon entry "déçu" even
    // though the user typed without accents, in a different inflection, or
    // both. Sentences are too short to detect reliably, so every sentence
    // reuses the single `lang` detected from the full content.
    for sentence in &sentences {
        let mut matched = false;
        let folded_sentence = crate::lexicon::fold_for_matching(sentence, lang, true);
        for class in EMOTION_CLASSES {
            if matched {
                break;
            }
            let Some((valence_signed, intensity)) =
                crate::lexicon::class_emotion_metadata(lang, class)
            else {
                continue;
            };
            let valence = valence_from_signed(valence_signed);
            for word in crate::lexicon::word_class(lang, class) {
                let folded_word = crate::lexicon::fold_word_for_class(&word, lang, class);
                if folded_sentence.contains(&folded_word) {
                    signals.push(PersonalitySignal {
                        signal_type: SignalType::Emotion,
                        subject: word.clone(),
                        valence,
                        intensity,
                        reasoning: format!("Expressed {valence} emotion: {word}"),
                        source_text: sentence.chars().take(500).collect(),
                    });
                    matched = true;
                    break;
                }
            }
        }

        if !matched {
            // Upstream env-var emotion extensions (KLEOS_PERSONALITY_*_EXTRA),
            // checked in addition to the i18n lexicon classes above.
            let lower = sentence.to_lowercase();
            for (kw, meta) in EXTRA_EMOTION_KEYWORDS.iter() {
                if lower.contains(kw.as_str()) {
                    signals.push(PersonalitySignal {
                        signal_type: SignalType::Emotion,
                        subject: kw.clone(),
                        valence: meta.valence,
                        intensity: meta.intensity,
                        reasoning: format!("Expressed {} emotion: {kw}", meta.valence),
                        source_text: sentence.chars().take(500).collect(),
                    });
                    break;
                }
            }
        }
    }

    signals
}

// ============================================================================
// Tier 2 -- Rule-based NLP signal extraction
// All Tier 3 patterns + sentiment lexicon + intensifiers + values + motivations.
// Quality: ~40%% of LLM.
// ============================================================================

/// Extract personality signals using rule-based NLP (Tier 2).
/// Includes all Tier 3 patterns plus sentiment calibration, intensifiers,
/// value patterns, and motivation patterns.
pub fn extract_signals_rule_based(content: &str, lang: &str) -> Vec<PersonalitySignal> {
    let mut signals = extract_signals_template(content, lang);

    // Single-language lookups for values and motivations (fall back to the
    // "en" entry when the detected language has no pattern), matching the
    // template extractor's detect-once approach.
    let mut seen_signals: std::collections::HashSet<(u8, String)> =
        std::collections::HashSet::new();

    // Values
    if let Some(re) = PERSONALITY_REGEX
        .value
        .get(lang)
        .or_else(|| PERSONALITY_REGEX.value.get("en"))
    {
        for caps in re.captures_iter(content) {
            if let Some(m) = caps.get(1) {
                let subject = clean_subject(m.as_str(), lang);
                if subject.len() < 3 || subject.len() > 100 {
                    continue;
                }
                if !seen_signals.insert((6, subject.clone())) {
                    continue;
                }
                signals.push(PersonalitySignal {
                    signal_type: SignalType::Value,
                    subject: subject.clone(),
                    valence: Valence::Positive,
                    intensity: 0.7,
                    reasoning: format!("Expressed that {subject} is important to them"),
                    source_text: caps
                        .get(0)
                        .map(|m| m.as_str().trim().to_string())
                        .unwrap_or_default(),
                });
            }
        }
    }

    // Motivations
    if let Some(re) = PERSONALITY_REGEX
        .motivation
        .get(lang)
        .or_else(|| PERSONALITY_REGEX.motivation.get("en"))
    {
        for caps in re.captures_iter(content) {
            if let Some(m) = caps.get(1) {
                let subject = clean_subject(m.as_str(), lang);
                if subject.len() < 3 || subject.len() > 100 {
                    continue;
                }
                if !seen_signals.insert((7, subject.clone())) {
                    continue;
                }
                signals.push(PersonalitySignal {
                    signal_type: SignalType::Motivation,
                    subject: subject.clone(),
                    valence: Valence::Positive,
                    intensity: 0.6,
                    reasoning: format!("Expressed aspiration toward {subject}"),
                    source_text: caps
                        .get(0)
                        .map(|m| m.as_str().trim().to_string())
                        .unwrap_or_default(),
                });
            }
        }
    }

    // Sentiment lexicon scoring for intensity calibration
    for sig in &mut signals {
        let (sentiment_sum, sentiment_count) = sentiment::score_text_sum(&sig.source_text);
        if sentiment_count > 0 {
            let avg_sentiment = sentiment_sum as f64 / sentiment_count as f64;
            sig.intensity = (sig.intensity + avg_sentiment * 0.05).clamp(0.0, 1.0);
        }

        // Intensifier detection (words via lexicon, source-text tokens folded
        // the same way as map keys so French intensifiers like "très" match
        // "tres" in the source). The map is the process-lifetime static built
        // once, not rebuilt per signal.
        let intensifiers = &INTENSIFIER_MAP;
        // The intensifier class has stem = false, so we fold without
        // stemming -- lowercase + diacritic strip only. We pick "en" as
        // the fold language since stem is disabled; the result is
        // language-agnostic in that path.
        let folded_text = crate::lexicon::fold_for_matching(&sig.source_text, "en", false);
        let words: Vec<String> = folded_text
            .split_whitespace()
            .map(|w| w.replace(char::is_whitespace, "_"))
            .collect();
        for word in &words {
            if let Some(&mult) = intensifiers.get(word.as_str()) {
                sig.intensity = (sig.intensity * mult).clamp(0.0, 1.0);
            }
        }

        // Punctuation-based intensity
        if sig.source_text.contains('!') {
            sig.intensity = (sig.intensity + 0.1).min(1.0);
        }
        if sig.source_text.len() > 5 && sig.source_text == sig.source_text.to_uppercase() {
            sig.intensity = (sig.intensity + 0.2).min(1.0);
        }
    }

    signals
}

// ============================================================================
// Profile Synthesis
// ============================================================================

const ALL_SIGNAL_TYPES: &[SignalType] = &[
    SignalType::Value,
    SignalType::Preference,
    SignalType::Decision,
    SignalType::Emotion,
    SignalType::Identity,
    SignalType::Motivation,
];

/// Tier 3 -- Template-based profile synthesis.
/// Structured dump of signals grouped by type.
/// Quality: ~20%% narrative, ~70%% informational content.
pub fn synthesize_profile_template(input: &SynthesisInput) -> String {
    if input.signals.is_empty() && input.preferences.is_empty() {
        return "Insufficient data for personality synthesis. No personality signals have been extracted yet.".to_string();
    }

    let mut sections = Vec::new();
    let now = chrono::Utc::now().format("%Y-%m-%d").to_string();
    sections.push(format!(
        "Profile based on {} signals. Updated {}.",
        input.signals.len(),
        now
    ));

    // Group by type
    let mut grouped: HashMap<&str, Vec<&SignalRow>> = HashMap::new();
    for sig in &input.signals {
        grouped
            .entry(sig.signal_type.as_str())
            .or_default()
            .push(sig);
    }

    // Sort each group by intensity desc, take top 5
    for sig_type in ALL_SIGNAL_TYPES {
        if let Some(group) = grouped.get_mut(sig_type.as_str()) {
            group.sort_by(|a, b| {
                b.intensity
                    .partial_cmp(&a.intensity)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let top: Vec<_> = group.iter().take(5).collect();
            if top.is_empty() {
                continue;
            }
            sections.push(format!("\n{}:", sig_type.label()));
            for sig in top {
                let reasoning = sig.reasoning.as_deref().unwrap_or("");
                let suffix = if reasoning.is_empty() {
                    String::new()
                } else {
                    format!(" -- {reasoning}")
                };
                sections.push(format!(
                    "- {} ({}, strength: {:.2}){}",
                    sig.subject, sig.valence, sig.intensity, suffix
                ));
            }
        }
    }

    // Preferences from user_preferences table
    if !input.preferences.is_empty() {
        let likes: Vec<_> = input
            .preferences
            .iter()
            .filter(|p| p.preference.starts_with("likes "))
            .collect();
        let dislikes: Vec<_> = input
            .preferences
            .iter()
            .filter(|p| p.preference.starts_with("dislikes "))
            .collect();
        if !likes.is_empty() {
            sections.push("\nLIKES:".to_string());
            for p in likes.iter().take(10) {
                let pref = p.preference.strip_prefix("likes ").unwrap_or(&p.preference);
                sections.push(format!(
                    "- {} [{}] (strength: {})",
                    pref, p.domain, p.strength
                ));
            }
        }
        if !dislikes.is_empty() {
            sections.push("\nDISLIKES:".to_string());
            for p in dislikes.iter().take(10) {
                let pref = p
                    .preference
                    .strip_prefix("dislikes ")
                    .unwrap_or(&p.preference);
                sections.push(format!(
                    "- {} [{}] (strength: {})",
                    pref, p.domain, p.strength
                ));
            }
        }
    }

    // Static memories
    if !input.static_memories.is_empty() {
        sections.push("\nCORE IDENTITY:".to_string());
        for m in input.static_memories.iter().take(5) {
            let content: String = m.content.chars().take(200).collect();
            sections.push(format!("- {content}"));
        }
    }

    sections.join("\n")
}

/// Tier 2 -- Rule-based NLP profile synthesis.
/// Signal clustering, trend detection, contradiction flagging.
/// Quality: ~45%%.
pub fn synthesize_profile_rule_based(input: &SynthesisInput) -> String {
    if input.signals.is_empty() && input.preferences.is_empty() {
        return "Insufficient data for personality synthesis. No personality signals have been extracted yet.".to_string();
    }

    let mut sections = Vec::new();
    let now = chrono::Utc::now().format("%Y-%m-%d").to_string();

    // 1. Signal clustering: group by subject token overlap >50%%
    fn tokenize(s: &str) -> Vec<String> {
        s.to_lowercase()
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c.is_whitespace() {
                    c
                } else {
                    ' '
                }
            })
            .collect::<String>()
            .split_whitespace()
            .filter(|t| t.len() >= 3)
            .map(|t| t.to_string())
            .collect()
    }

    struct Cluster {
        label: String,
        signal_indices: Vec<usize>,
        avg_intensity: f64,
    }

    let mut clusters: Vec<Cluster> = Vec::new();
    for (i, sig) in input.signals.iter().enumerate() {
        let sig_tokens: std::collections::HashSet<String> =
            tokenize(&sig.subject).into_iter().collect();
        let mut placed = false;
        for cluster in &mut clusters {
            let cluster_tokens: std::collections::HashSet<String> =
                tokenize(&cluster.label).into_iter().collect();
            let intersection = sig_tokens.intersection(&cluster_tokens).count();
            let union = sig_tokens.union(&cluster_tokens).count();
            if union > 0 && (intersection as f64 / union as f64) > 0.5 {
                cluster.signal_indices.push(i);
                let total: f64 = cluster
                    .signal_indices
                    .iter()
                    .map(|&idx| input.signals[idx].intensity)
                    .sum();
                cluster.avg_intensity = total / cluster.signal_indices.len() as f64;
                placed = true;
                break;
            }
        }
        if !placed {
            clusters.push(Cluster {
                label: sig.subject.clone(),
                signal_indices: vec![i],
                avg_intensity: sig.intensity,
            });
        }
    }

    clusters.sort_by_key(|b| std::cmp::Reverse(b.signal_indices.len()));

    sections.push(format!(
        "Personality profile based on {} signals across {} themes. Generated {}.",
        input.signals.len(),
        clusters.len(),
        now
    ));

    // 2. Top themes
    let top_clusters = &clusters[..clusters.len().min(8)];
    if !top_clusters.is_empty() {
        sections.push("\nKEY THEMES:".to_string());
        for cluster in top_clusters {
            let types: Vec<String> = cluster
                .signal_indices
                .iter()
                .map(|&i| input.signals[i].signal_type.clone())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            let valences: Vec<String> = cluster
                .signal_indices
                .iter()
                .map(|&i| input.signals[i].valence.clone())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            let count = cluster.signal_indices.len();
            let intensity = cluster.avg_intensity;

            if count >= 3 {
                sections.push(format!(
                    "- \"{}\" is a strong theme ({} signals, avg intensity: {:.2}). Types: {}. Valences: {}.",
                    cluster.label, count, intensity, types.join(", "), valences.join(", ")
                ));
            } else {
                let plural = if count > 1 { "s" } else { "" };
                sections.push(format!(
                    "- \"{}\" ({} signal{}, intensity: {:.2}, {})",
                    cluster.label,
                    count,
                    plural,
                    intensity,
                    valences.join("/")
                ));
            }
        }
    }

    // 3. Contradiction flagging
    let mut contradictions = Vec::new();
    for cluster in &clusters {
        let has_positive = cluster
            .signal_indices
            .iter()
            .any(|&i| input.signals[i].valence == "positive");
        let has_negative = cluster
            .signal_indices
            .iter()
            .any(|&i| input.signals[i].valence == "negative");
        if has_positive && has_negative {
            contradictions.push(cluster.label.clone());
        }
    }
    if !contradictions.is_empty() {
        sections.push("\nCOMPLEXITIES:".to_string());
        for c in &contradictions {
            sections.push(format!(
                "- \"{c}\" shows mixed signals -- both positive and negative sentiments detected. This suggests nuanced or evolving feelings."
            ));
        }
    }

    // 4. Preferences summary
    if !input.preferences.is_empty() {
        sections.push("\nSTATED PREFERENCES:".to_string());
        for p in input.preferences.iter().take(10) {
            sections.push(format!(
                "- [{}] {} (strength: {})",
                p.domain, p.preference, p.strength
            ));
        }
    }

    // 5. Identity from static memories
    if !input.static_memories.is_empty() {
        sections.push("\nCORE IDENTITY:".to_string());
        for m in input.static_memories.iter().take(5) {
            let content: String = m.content.chars().take(200).collect();
            sections.push(format!("- {content}"));
        }
    }

    // 6. Summary
    let top_types: Vec<String> = input
        .signals
        .iter()
        .map(|s| s.signal_type.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let avg_intensity = if input.signals.is_empty() {
        0.0
    } else {
        input.signals.iter().map(|s| s.intensity).sum::<f64>() / input.signals.len() as f64
    };
    sections.push(format!(
        "\nOverall signal types: {}. Average intensity: {:.2}.",
        top_types.join(", "),
        avg_intensity
    ));

    sections.join("\n")
}

// ============================================================================
// DB-backed functions
// ============================================================================

/// Insert a personality signal into the database.
#[tracing::instrument(skip(db, signal), fields(memory_id, user_id, signal_type = ?signal.signal_type))]
pub async fn insert_signal(
    db: &Database,
    memory_id: i64,
    user_id: i64,
    signal: &PersonalitySignal,
) -> Result<()> {
    let signal_type = signal.signal_type.as_str().to_string();
    let subject: String = signal.subject.chars().take(200).collect();
    let valence = signal.valence.as_str().to_string();
    let intensity = signal.intensity;
    let reasoning: String = signal.reasoning.chars().take(1000).collect();
    let source_text: String = signal.source_text.chars().take(500).collect();

    db.write(move |conn| {
        conn.execute(
            "INSERT INTO personality_signals (memory_id, user_id, signal_type, subject, valence, value, intensity, reasoning, source_text)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                memory_id,
                user_id,
                signal_type,
                subject,
                valence,
                intensity,
                intensity,
                reasoning,
                source_text,
            ],
        )
        ?;
        Ok(())
    })
    .await
}

/// Extract personality signals from content and store them.
/// Uses rule-based extraction (Tier 2) as default fallback.
#[tracing::instrument(skip(db, content), fields(memory_id, user_id, content_len = content.len()))]
pub async fn extract_personality_signals(
    db: &Database,
    content: &str,
    memory_id: i64,
    user_id: i64,
) -> Result<Vec<PersonalitySignal>> {
    if content.len() < 50 {
        return Ok(Vec::new());
    }

    // Detect the content language once at this public boundary and thread it
    // through the private extractors (never re-detected on short spans).
    let lang = crate::lang::detect_lang(content);
    let signals = extract_signals_rule_based(content, &lang);
    if signals.is_empty() {
        return Ok(Vec::new());
    }

    let mut valid_signals = Vec::new();
    for signal in signals {
        match insert_signal(db, memory_id, user_id, &signal).await {
            Ok(()) => valid_signals.push(signal),
            Err(e) => {
                warn!(msg = "personality_signal_insert_failed", memory_id, error = %e);
            }
        }
    }

    if !valid_signals.is_empty() {
        // Invalidate cached profile since we have new signals
        if let Err(e) = invalidate_profile(db, user_id).await {
            tracing::warn!(error = %e, user_id, "failed to invalidate personality profile cache");
        }
        debug!(
            msg = "personality_extracted_fallback",
            memory_id,
            signals = valid_signals.len()
        );
    }

    Ok(valid_signals)
}

/// Synthesize a personality profile from stored signals.
#[tracing::instrument(skip(db), fields(user_id))]
pub async fn synthesize_personality_profile(db: &Database, user_id: i64) -> Result<String> {
    // Gather all personality signals
    let signals = db
        .read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT signal_type, subject, valence, intensity, reasoning, source_text
             FROM personality_signals WHERE user_id = ?1 ORDER BY intensity DESC",
            )?;

            let rows = stmt.query_map(rusqlite::params![user_id], |row| {
                Ok(SignalRow {
                    signal_type: row.get(0)?,
                    subject: row.get(1)?,
                    valence: row.get(2)?,
                    intensity: row.get(3)?,
                    reasoning: row.get(4)?,
                    source_text: row.get(5)?,
                })
            })?;

            let mut signals = Vec::new();
            for row in rows {
                signals.push(row?);
            }
            Ok(signals)
        })
        .await?;

    if signals.is_empty() {
        return Ok("Insufficient data for personality synthesis. No personality signals have been extracted yet.".to_string());
    }

    // Gather preferences.
    // domain and preference are nullable columns (TEXT without NOT NULL); use
    // Option<String> when reading and fall back to empty string so a NULL value
    // in either column does not abort the entire preferences query with an
    // InvalidColumnType error.
    let preferences = db
        .read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT domain, preference, strength FROM user_preferences \
             WHERE user_id = ?1 ORDER BY strength DESC LIMIT 50",
            )?;

            let rows = stmt.query_map(rusqlite::params![user_id], |row| {
                Ok(PreferenceRow {
                    domain: row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    preference: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    strength: row.get(2)?,
                })
            })?;

            let mut preferences = Vec::new();
            for row in rows {
                preferences.push(row?);
            }
            Ok(preferences)
        })
        .await?;

    // Gather facts
    let facts = db
        .read(move |conn| {
            // The user_id predicate is a no-op in a single-owner shard and the
            // tenant boundary in monolith mode; bind the caller's effective id.
            let mut stmt = conn.prepare(
                "SELECT subject, verb, object FROM structured_facts WHERE user_id = ?1 LIMIT 50",
            )?;

            let rows = stmt.query_map(rusqlite::params![user_id], |row| {
                Ok(FactRow {
                    subject: row.get(0)?,
                    verb: row.get(1)?,
                    object: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                })
            })?;

            let mut facts = Vec::new();
            for row in rows {
                facts.push(row?);
            }
            Ok(facts)
        })
        .await?;

    // Gather static memories
    let static_memories = db.read(move |conn| {
        // The user_id predicate is a no-op in a single-owner shard and the
        // tenant boundary in monolith mode; bind the caller's effective id.
        let mut stmt = conn.prepare(
            "SELECT content FROM memories WHERE is_static = 1 AND is_forgotten = 0 AND user_id = ?1 ORDER BY importance DESC LIMIT 20",
        )?;

        let rows = stmt.query_map(rusqlite::params![user_id], |row| {
            Ok(StaticMemoryRow {
                content: row.get(0)?,
            })
        })?;

        let mut static_memories = Vec::new();
        for row in rows {
            static_memories.push(row?);
        }
        Ok(static_memories)
    }).await?;

    let input = SynthesisInput {
        signals,
        preferences,
        facts,
        static_memories,
    };
    let profile = synthesize_profile_rule_based(&input);

    // Cache the profile
    let signal_count = input.signals.len() as i64;
    let profile_clone = profile.clone();
    if let Err(e) = db.write(move |conn| {
        conn.execute(
            "INSERT INTO personality_profiles (user_id, profile, signal_count, is_stale)
             VALUES (?1, ?2, ?3, 0)
             ON CONFLICT(user_id) DO UPDATE SET profile = ?2, signal_count = ?3, is_stale = 0, updated_at = datetime('now')",
            rusqlite::params![user_id, profile_clone, signal_count],
        )?;
        Ok(())
    }).await
    { tracing::warn!(error = %e, user_id, "failed to cache personality profile"); }

    info!(
        msg = "personality_profile_synthesized",
        user_id,
        signals = signal_count
    );
    Ok(profile)
}

/// Get a cached personality profile.
#[tracing::instrument(skip(db), fields(user_id))]
pub async fn get_cached_profile(db: &Database, user_id: i64) -> Result<Option<String>> {
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT profile FROM personality_profiles WHERE user_id = ?1 AND is_stale = 0",
        )?;

        let mut rows = stmt.query(rusqlite::params![user_id])?;

        match rows.next()? {
            Some(row) => {
                let profile: String = row.get(0)?;
                Ok(Some(profile))
            }
            None => Ok(None),
        }
    })
    .await
}

/// Invalidate (mark stale) the cached personality profile.
#[tracing::instrument(skip(db), fields(user_id))]
pub async fn invalidate_profile(db: &Database, user_id: i64) -> Result<()> {
    db.write(move |conn| {
        conn.execute(
            "UPDATE personality_profiles SET is_stale = 1 WHERE user_id = ?1",
            rusqlite::params![user_id],
        )?;
        Ok(())
    })
    .await
}

/// Get profile for context injection. Returns profile and staleness flag.
#[tracing::instrument(skip(db), fields(user_id))]
pub async fn get_profile_for_injection(
    db: &Database,
    user_id: i64,
) -> Result<Option<(String, bool)>> {
    db.read(move |conn| {
        let mut stmt =
            conn.prepare("SELECT profile, is_stale FROM personality_profiles WHERE user_id = ?1")?;

        let mut rows = stmt.query(rusqlite::params![user_id])?;

        match rows.next()? {
            Some(row) => {
                let profile: String = row.get(0)?;
                let is_stale: i32 = row.get(1)?;
                Ok(Some((profile, is_stale != 0)))
            }
            None => Ok(None),
        }
    })
    .await
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSignal {
    pub id: i64,
    pub signal_type: String,
    pub value: f64,
    pub evidence: Option<String>,
    pub user_id: i64,
    pub agent: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredProfile {
    pub user_id: i64,
    pub traits: serde_json::Value,
    pub last_updated_at: String,
    pub created_at: String,
}

pub fn detect_signals(content: &str) -> Vec<(String, f64)> {
    // Detect language once here (the public boundary), then thread it inward.
    let lang = crate::lang::detect_lang(content);
    extract_signals_rule_based(content, &lang)
        .into_iter()
        .map(|signal| (signal.signal_type.as_str().to_string(), signal.intensity))
        .collect()
}

#[tracing::instrument(skip(db, evidence), fields(signal_type = %signal_type, value, user_id, agent = ?agent))]
pub async fn store_signal(
    db: &Database,
    signal_type: &str,
    value: f64,
    evidence: Option<&str>,
    user_id: i64,
    agent: Option<&str>,
) -> Result<StoredSignal> {
    let signal_type = signal_type.to_string();
    let evidence = evidence.map(|v| v.to_string());
    let agent = agent.map(|v| v.to_string());

    db.write(move |conn| {
        let mut stmt = conn.prepare(
            "INSERT INTO personality_signals (signal_type, value, evidence, user_id, agent)
             VALUES (?1, ?2, ?3, ?4, ?5)
             RETURNING id, signal_type, value, evidence, user_id, agent, created_at",
        )?;

        let row = stmt.query_row(
            rusqlite::params![signal_type, value, evidence, user_id, agent],
            |row| {
                Ok(StoredSignal {
                    id: row.get(0)?,
                    signal_type: row.get(1)?,
                    value: row.get(2)?,
                    evidence: row.get(3)?,
                    user_id: row.get(4)?,
                    agent: row.get(5)?,
                    created_at: row.get(6)?,
                })
            },
        )?;

        Ok(row)
    })
    .await
}

#[tracing::instrument(skip(db), fields(user_id, limit))]
pub async fn list_signals(db: &Database, user_id: i64, limit: usize) -> Result<Vec<StoredSignal>> {
    let limit = limit as i64;
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT id, signal_type, value, evidence, user_id, agent, created_at
             FROM personality_signals
             WHERE user_id = ?1
             ORDER BY created_at DESC
             LIMIT ?2",
        )?;

        let rows = stmt.query_map(rusqlite::params![user_id, limit], |row| {
            Ok(StoredSignal {
                id: row.get(0)?,
                signal_type: row.get(1)?,
                value: row.get(2)?,
                evidence: row.get(3)?,
                user_id: row.get(4)?,
                agent: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;

        let mut signals = Vec::new();
        for row in rows {
            signals.push(row?);
        }
        Ok(signals)
    })
    .await
}

#[tracing::instrument(skip(db), fields(user_id))]
pub async fn get_profile(db: &Database, user_id: i64) -> Result<StoredProfile> {
    if let Some(profile) = get_existing_profile(db, user_id).await? {
        return Ok(profile);
    }
    update_profile(db, user_id).await
}

#[tracing::instrument(skip(db), fields(user_id))]
pub async fn update_profile(db: &Database, user_id: i64) -> Result<StoredProfile> {
    let signals = list_signals(db, user_id, 200).await?;
    let mut traits = serde_json::Map::new();
    for signal in &signals {
        traits.insert(signal.signal_type.clone(), serde_json::json!(signal.value));
    }

    let traits_value = serde_json::Value::Object(traits);
    let traits_str = traits_value.to_string();

    db.write(move |conn| {
        let mut stmt = conn.prepare(
            "INSERT INTO personality_profiles (user_id, traits, last_updated_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(user_id) DO UPDATE SET traits = excluded.traits, last_updated_at = excluded.last_updated_at
             RETURNING user_id, traits, last_updated_at, created_at",
        )?;

        let row = stmt.query_row(
            rusqlite::params![user_id, traits_str],
            |row| {
                let traits_json: String = row.get(1)?;
                Ok(StoredProfile {
                    user_id: row.get(0)?,
                    traits: serde_json::from_str(&traits_json).unwrap_or(serde_json::json!({})),
                    last_updated_at: row.get(2)?,
                    created_at: row.get(3)?,
                })
            },
        ).map_err(|e| EngError::Internal(format!("upsert personality profile failed: {e}")))?;

        Ok(row)
    }).await
}

async fn get_existing_profile(db: &Database, user_id: i64) -> Result<Option<StoredProfile>> {
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT user_id, traits, last_updated_at, created_at
             FROM personality_profiles
             WHERE user_id = ?1",
        )?;

        let mut rows = stmt.query(rusqlite::params![user_id])?;

        match rows.next()? {
            Some(row) => {
                let traits_json: String = row.get(1)?;
                Ok(Some(StoredProfile {
                    user_id: row.get(0)?,
                    traits: serde_json::from_str(&traits_json).unwrap_or(serde_json::json!({})),
                    last_updated_at: row.get(2)?,
                    created_at: row.get(3)?,
                }))
            }
            None => Ok(None),
        }
    })
    .await
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_subject() {
        assert_eq!(clean_subject("  a dog  ", "en"), "dog");
        assert_eq!(clean_subject("the quick fox", "en"), "quick fox");
        assert_eq!(clean_subject("my favorite thing", "en"), "favorite thing");
    }

    #[test]
    fn clean_subject_strips_french_article() {
        // The French article "le" is stripped via the fr lexicon's `articles`
        // class, proving per-language (not unioned) article handling.
        assert_eq!(clean_subject("le serveur", "fr"), "serveur");
    }

    #[test]
    fn extract_french_signal() {
        // Best-effort: a French preference sentence should travel the fr regex
        // path without panicking. If the fr lexicon lacks the pattern the
        // result is simply empty -- either way, no panic.
        let signals = extract_signals_template("J'aime la programmation et la cuisine.", "fr");
        if let Some(pref) = signals
            .iter()
            .find(|s| s.signal_type == SignalType::Preference)
        {
            assert_eq!(pref.valence, Valence::Positive);
        }
    }

    #[test]
    fn test_split_sentences() {
        let sentences = split_sentences("Hello world. How are you? I am fine!");
        assert_eq!(sentences.len(), 3);
    }

    #[test]
    fn test_extract_like() {
        let signals = extract_signals_template("I love programming and building things.", "en");
        assert!(!signals.is_empty(), "Should extract at least one signal");
        let pref = signals
            .iter()
            .find(|s| s.signal_type == SignalType::Preference);
        assert!(pref.is_some(), "Should find a preference signal");
        assert_eq!(pref.unwrap().valence, Valence::Positive);
    }

    #[test]
    fn test_extract_dislike() {
        let signals = extract_signals_template("I hate waking up early.", "en");
        assert!(!signals.is_empty(), "Should extract at least one signal");
        let pref = signals
            .iter()
            .find(|s| s.signal_type == SignalType::Preference);
        assert!(pref.is_some(), "Should find a preference signal");
        assert_eq!(pref.unwrap().valence, Valence::Negative);
    }

    #[test]
    fn test_extract_decision() {
        let signals = extract_signals_template("I decided to quit my job.", "en");
        let decision = signals
            .iter()
            .find(|s| s.signal_type == SignalType::Decision);
        assert!(decision.is_some(), "Should find a decision signal");
    }

    #[test]
    fn test_extract_identity() {
        let signals = extract_signals_template("I am a software developer.", "en");
        let identity = signals
            .iter()
            .find(|s| s.signal_type == SignalType::Identity);
        assert!(identity.is_some(), "Should find an identity signal");
    }

    #[test]
    fn test_extract_emotion() {
        let signals = extract_signals_template("I feel really excited about this project.", "en");
        let emotion = signals
            .iter()
            .find(|s| s.signal_type == SignalType::Emotion);
        assert!(emotion.is_some(), "Should find an emotion signal");
    }

    #[test]
    fn test_rule_based_values() {
        let signals =
            extract_signals_rule_based("I believe in open source software and community.", "en");
        let value = signals.iter().find(|s| s.signal_type == SignalType::Value);
        assert!(value.is_some(), "Should find a value signal");
    }

    #[test]
    fn test_rule_based_motivation() {
        let signals =
            extract_signals_rule_based("I want to learn Rust and systems programming.", "en");
        let motivation = signals
            .iter()
            .find(|s| s.signal_type == SignalType::Motivation);
        assert!(motivation.is_some(), "Should find a motivation signal");
    }

    #[test]
    fn test_rule_based_intensifier() {
        let base = extract_signals_template("I love cooking.", "en");
        let intensified = extract_signals_rule_based("I really love cooking!", "en");
        if let (Some(b), Some(i)) = (
            base.iter()
                .find(|s| s.signal_type == SignalType::Preference),
            intensified
                .iter()
                .find(|s| s.signal_type == SignalType::Preference),
        ) {
            assert!(
                i.intensity >= b.intensity,
                "Intensified signal should be >= base"
            );
        }
    }

    #[test]
    fn test_short_content_no_extraction() {
        let signals = extract_signals_template("Hi.", "en");
        assert!(signals.is_empty());
    }

    #[test]
    fn test_synthesize_template_empty() {
        let input = SynthesisInput::default();
        let profile = synthesize_profile_template(&input);
        assert!(profile.contains("Insufficient data"));
    }

    #[test]
    fn test_synthesize_template_with_signals() {
        let input = SynthesisInput {
            signals: vec![SignalRow {
                signal_type: "preference".to_string(),
                subject: "programming".to_string(),
                valence: "positive".to_string(),
                intensity: 0.8,
                reasoning: Some("Enjoys coding".to_string()),
                source_text: Some("I love programming".to_string()),
            }],
            ..Default::default()
        };
        let profile = synthesize_profile_template(&input);
        assert!(
            profile.contains("PREFERENCES"),
            "Should contain PREFERENCES section"
        );
        assert!(
            profile.contains("programming"),
            "Should mention programming"
        );
    }

    #[test]
    fn test_synthesize_rule_based_with_signals() {
        let input = SynthesisInput {
            signals: vec![
                SignalRow {
                    signal_type: "preference".to_string(),
                    subject: "rust programming".to_string(),
                    valence: "positive".to_string(),
                    intensity: 0.9,
                    reasoning: Some("Strong preference".to_string()),
                    source_text: None,
                },
                SignalRow {
                    signal_type: "value".to_string(),
                    subject: "open source".to_string(),
                    valence: "positive".to_string(),
                    intensity: 0.7,
                    reasoning: None,
                    source_text: None,
                },
            ],
            ..Default::default()
        };
        let profile = synthesize_profile_rule_based(&input);
        assert!(
            profile.contains("KEY THEMES"),
            "Should contain KEY THEMES section"
        );
        assert!(profile.contains("2 signals"), "Should mention signal count");
    }

    #[test]
    fn detect_signals_returns_scores_for_emotional_content() {
        let signals =
            detect_signals("I feel really excited about this project. I love building things.");
        assert!(
            !signals.is_empty(),
            "Should detect signals in emotional content"
        );
        for (_, intensity) in &signals {
            assert!(
                *intensity >= 0.0 && *intensity <= 1.0,
                "intensity must be in [0, 1]"
            );
        }
    }

    #[test]
    fn detect_signals_empty_for_neutral_content() {
        let signals = detect_signals("The server started on port 4200.");
        assert!(
            signals.is_empty(),
            "Should not detect signals in neutral technical content"
        );
    }

    #[test]
    fn extra_emotion_keywords_env_var_is_additive() {
        // Built-in keywords must always work regardless of env-var state.
        // (LazyLock-based env-var loading cannot be reliably tested in parallel
        // unit tests — covered by integration tests instead.)
        let signals = detect_signals("I feel happy today.");
        assert!(
            !signals.is_empty(),
            "Built-in emotion keywords must always be active"
        );
    }
}
