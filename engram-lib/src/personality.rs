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

struct EmotionMeta {
    valence: Valence,
    intensity: f64,
}
static EMOTION_KEYWORDS: LazyLock<HashMap<&'static str, EmotionMeta>> = LazyLock::new(|| {
    HashMap::from([
        (
            "happy",
            EmotionMeta {
                valence: Valence::Positive,
                intensity: 0.6,
            },
        ),
        (
            "excited",
            EmotionMeta {
                valence: Valence::Positive,
                intensity: 0.8,
            },
        ),
        (
            "grateful",
            EmotionMeta {
                valence: Valence::Positive,
                intensity: 0.7,
            },
        ),
        (
            "proud",
            EmotionMeta {
                valence: Valence::Positive,
                intensity: 0.7,
            },
        ),
        (
            "relieved",
            EmotionMeta {
                valence: Valence::Positive,
                intensity: 0.5,
            },
        ),
        (
            "thrilled",
            EmotionMeta {
                valence: Valence::Positive,
                intensity: 0.9,
            },
        ),
        (
            "content",
            EmotionMeta {
                valence: Valence::Positive,
                intensity: 0.5,
            },
        ),
        (
            "sad",
            EmotionMeta {
                valence: Valence::Negative,
                intensity: 0.6,
            },
        ),
        (
            "angry",
            EmotionMeta {
                valence: Valence::Negative,
                intensity: 0.8,
            },
        ),
        (
            "frustrated",
            EmotionMeta {
                valence: Valence::Negative,
                intensity: 0.7,
            },
        ),
        (
            "anxious",
            EmotionMeta {
                valence: Valence::Negative,
                intensity: 0.6,
            },
        ),
        (
            "stressed",
            EmotionMeta {
                valence: Valence::Negative,
                intensity: 0.7,
            },
        ),
        (
            "disappointed",
            EmotionMeta {
                valence: Valence::Negative,
                intensity: 0.6,
            },
        ),
        (
            "overwhelmed",
            EmotionMeta {
                valence: Valence::Negative,
                intensity: 0.8,
            },
        ),
        (
            "lonely",
            EmotionMeta {
                valence: Valence::Negative,
                intensity: 0.7,
            },
        ),
        (
            "worried",
            EmotionMeta {
                valence: Valence::Negative,
                intensity: 0.5,
            },
        ),
        (
            "bored",
            EmotionMeta {
                valence: Valence::Negative,
                intensity: 0.4,
            },
        ),
    ])
});

static INTENSIFIERS: LazyLock<HashMap<&'static str, f64>> = LazyLock::new(|| {
    HashMap::from([
        ("very", 1.3),
        ("really", 1.3),
        ("absolutely", 1.4),
        ("extremely", 1.4),
        ("incredibly", 1.4),
        ("super", 1.3),
        ("totally", 1.3),
        ("deeply", 1.3),
        ("strongly", 1.3),
        ("highly", 1.3),
        ("somewhat", 0.7),
        ("slightly", 0.7),
        ("a_bit", 0.7),
        ("kind_of", 0.7),
        ("sort_of", 0.7),
        ("barely", 0.5),
        ("hardly", 0.5),
        ("mildly", 0.6),
    ])
});

// ============================================================================
// Regex patterns for signal extraction
// ============================================================================

macro_rules! lazy_regex {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new($pat).expect(concat!("invalid regex: ", stringify!($name)))
        });
    };
}

lazy_regex!(
    LIKE_PATTERN,
    r"(?i)\b(?:I\s+)?(?:love|like|enjoy|prefer|adore|am (?:really )?into)\s+(.+?)(?:\.|,|!|\s+(?:and|but|so|because))"
);
lazy_regex!(
    DISLIKE_PATTERN,
    r"(?i)\b(?:I\s+)?(?:hate|dislike|don't like|can't stand|avoid)\s+(.+?)(?:\.|,|!|\s+(?:and|but|so|because))"
);
lazy_regex!(
    FAV_PATTERN,
    r"(?i)\bmy favorite\s+(.+?)\s+(?:is|are)\s+(.+?)(?:\.|,|$)"
);
lazy_regex!(
    DECISION_PATTERN,
    r"(?i)\b(?:I\s+)?(?:decided to|chose to|going to|switched to|opted for|picked|went with)\s+(.+?)(?:\.|,|!|$)"
);
lazy_regex!(
    IDENTITY_PATTERN,
    r"(?i)\b(?:I\s+)?(?:am a|'m a|consider myself|identify as)\s+(.+?)(?:\.|,|!|$)"
);
lazy_regex!(
    VALUE_PATTERN,
    r"(?i)\b(?:important to me|matters to me|I believe in|I value|I care about)\s+(.+?)(?:\.|,|!|$)"
);
lazy_regex!(
    MOTIVATION_PATTERN,
    r"(?i)\b(?:I want to|my goal is|I aspire to|I'm trying to|I hope to|I aim to)\s+(.+?)(?:\.|,|!|$)"
);

// ============================================================================
// Helper functions
// ============================================================================

/// Clean a captured subject string: trim, remove leading articles, truncate.
fn clean_subject(raw: &str) -> String {
    let trimmed = raw.trim();
    let stripped = trimmed
        .strip_prefix("a ")
        .or_else(|| trimmed.strip_prefix("an "))
        .or_else(|| trimmed.strip_prefix("the "))
        .or_else(|| trimmed.strip_prefix("my "))
        .or_else(|| trimmed.strip_prefix("our "))
        .unwrap_or(trimmed);
    let chars: String = stripped.chars().take(200).collect();
    chars
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

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

// ============================================================================
// Tier 3 -- Template-based signal extraction
// Pattern match explicit signals only. Quality: ~25%% of LLM.
// ============================================================================

/// Extract personality signals using template-based pattern matching (Tier 3).
pub fn extract_signals_template(content: &str) -> Vec<PersonalitySignal> {
    let mut signals = Vec::new();
    let sentences = split_sentences(content);

    // Preferences: likes
    for caps in LIKE_PATTERN.captures_iter(content) {
        if let Some(m) = caps.get(1) {
            let subject = clean_subject(m.as_str());
            if subject.len() < 3 || subject.len() > 100 {
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

    // Preferences: dislikes
    for caps in DISLIKE_PATTERN.captures_iter(content) {
        if let Some(m) = caps.get(1) {
            let subject = clean_subject(m.as_str());
            if subject.len() < 3 || subject.len() > 100 {
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

    // Preferences: favorites
    for caps in FAV_PATTERN.captures_iter(content) {
        if let (Some(cat), Some(val)) = (caps.get(1), caps.get(2)) {
            let cat_clean = clean_subject(cat.as_str());
            let val_clean = clean_subject(val.as_str());
            signals.push(PersonalitySignal {
                signal_type: SignalType::Preference,
                subject: format!("{cat_clean}: {val_clean}"),
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

    // Decisions
    for caps in DECISION_PATTERN.captures_iter(content) {
        if let Some(m) = caps.get(1) {
            let subject = clean_subject(m.as_str());
            if subject.len() < 3 || subject.len() > 100 {
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

    // Identity
    for caps in IDENTITY_PATTERN.captures_iter(content) {
        if let Some(m) = caps.get(1) {
            let subject = clean_subject(m.as_str());
            if subject.len() < 3 || subject.len() > 100 {
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

    // Emotions (keyword scan per sentence)
    for sentence in &sentences {
        let lower = sentence.to_lowercase();
        for (keyword, meta) in EMOTION_KEYWORDS.iter() {
            if lower.contains(keyword) {
                signals.push(PersonalitySignal {
                    signal_type: SignalType::Emotion,
                    subject: keyword.to_string(),
                    valence: meta.valence,
                    intensity: meta.intensity,
                    reasoning: format!("Expressed {} emotion: {keyword}", meta.valence),
                    source_text: sentence.chars().take(500).collect(),
                });
                break; // One emotion per sentence
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
pub fn extract_signals_rule_based(content: &str) -> Vec<PersonalitySignal> {
    let mut signals = extract_signals_template(content);

    // Values
    for caps in VALUE_PATTERN.captures_iter(content) {
        if let Some(m) = caps.get(1) {
            let subject = clean_subject(m.as_str());
            if subject.len() < 3 || subject.len() > 100 {
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

    // Motivations
    for caps in MOTIVATION_PATTERN.captures_iter(content) {
        if let Some(m) = caps.get(1) {
            let subject = clean_subject(m.as_str());
            if subject.len() < 3 || subject.len() > 100 {
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

    // Sentiment lexicon scoring for intensity calibration
    for sig in &mut signals {
        let (sentiment_sum, sentiment_count) = sentiment::score_text_sum(&sig.source_text);
        if sentiment_count > 0 {
            let avg_sentiment = sentiment_sum as f64 / sentiment_count as f64;
            sig.intensity = (sig.intensity + avg_sentiment * 0.05).clamp(0.0, 1.0);
        }

        // Intensifier detection
        let words: Vec<String> = sig
            .source_text
            .to_lowercase()
            .split_whitespace()
            .map(|w| w.replace(char::is_whitespace, "_"))
            .collect();
        for word in &words {
            if let Some(&mult) = INTENSIFIERS.get(word.as_str()) {
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

    clusters.sort_by(|a, b| b.signal_indices.len().cmp(&a.signal_indices.len()));

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
        .map_err(rusqlite_to_eng_error)?;
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

    let signals = extract_signals_rule_based(content);
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
        let _ = invalidate_profile(db, user_id).await;
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
            let mut stmt = conn
                .prepare(
                    "SELECT signal_type, subject, valence, intensity, reasoning, source_text
             FROM personality_signals WHERE user_id = ?1 ORDER BY intensity DESC",
                )
                .map_err(rusqlite_to_eng_error)?;

            let rows = stmt
                .query_map(rusqlite::params![user_id], |row| {
                    Ok(SignalRow {
                        signal_type: row.get(0)?,
                        subject: row.get(1)?,
                        valence: row.get(2)?,
                        intensity: row.get(3)?,
                        reasoning: row.get(4)?,
                        source_text: row.get(5)?,
                    })
                })
                .map_err(rusqlite_to_eng_error)?;

            let mut signals = Vec::new();
            for row in rows {
                signals.push(row.map_err(rusqlite_to_eng_error)?);
            }
            Ok(signals)
        })
        .await?;

    if signals.is_empty() {
        return Ok("Insufficient data for personality synthesis. No personality signals have been extracted yet.".to_string());
    }

    // Gather preferences
    let preferences = db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT domain, preference, strength FROM user_preferences WHERE user_id = ?1 ORDER BY strength DESC LIMIT 50",
        ).map_err(rusqlite_to_eng_error)?;

        let rows = stmt.query_map(rusqlite::params![user_id], |row| {
            Ok(PreferenceRow {
                domain: row.get(0)?,
                preference: row.get(1)?,
                strength: row.get(2)?,
            })
        }).map_err(rusqlite_to_eng_error)?;

        let mut preferences = Vec::new();
        for row in rows {
            preferences.push(row.map_err(rusqlite_to_eng_error)?);
        }
        Ok(preferences)
    }).await?;

    // Gather facts
    let facts = db
        .read(move |conn| {
            let mut stmt = conn.prepare(
            "SELECT subject, verb, object FROM structured_facts WHERE user_id = ?1 LIMIT 50",
        ).map_err(rusqlite_to_eng_error)?;

            let rows = stmt
                .query_map(rusqlite::params![user_id], |row| {
                    Ok(FactRow {
                        subject: row.get(0)?,
                        verb: row.get(1)?,
                        object: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    })
                })
                .map_err(rusqlite_to_eng_error)?;

            let mut facts = Vec::new();
            for row in rows {
                facts.push(row.map_err(rusqlite_to_eng_error)?);
            }
            Ok(facts)
        })
        .await?;

    // Gather static memories
    let static_memories = db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT content FROM memories WHERE user_id = ?1 AND is_static = 1 AND is_forgotten = 0 ORDER BY importance DESC LIMIT 20",
        ).map_err(rusqlite_to_eng_error)?;

        let rows = stmt.query_map(rusqlite::params![user_id], |row| {
            Ok(StaticMemoryRow {
                content: row.get(0)?,
            })
        }).map_err(rusqlite_to_eng_error)?;

        let mut static_memories = Vec::new();
        for row in rows {
            static_memories.push(row.map_err(rusqlite_to_eng_error)?);
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
    let _ = db.write(move |conn| {
        conn.execute(
            "INSERT INTO personality_profiles (user_id, profile, signal_count, is_stale)
             VALUES (?1, ?2, ?3, 0)
             ON CONFLICT(user_id) DO UPDATE SET profile = ?2, signal_count = ?3, is_stale = 0, updated_at = datetime('now')",
            rusqlite::params![user_id, profile_clone, signal_count],
        ).map_err(rusqlite_to_eng_error)?;
        Ok(())
    }).await;

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
        let mut stmt = conn
            .prepare("SELECT profile FROM personality_profiles WHERE user_id = ?1 AND is_stale = 0")
            .map_err(rusqlite_to_eng_error)?;

        let mut rows = stmt
            .query(rusqlite::params![user_id])
            .map_err(rusqlite_to_eng_error)?;

        match rows.next().map_err(rusqlite_to_eng_error)? {
            Some(row) => {
                let profile: String = row.get(0).map_err(rusqlite_to_eng_error)?;
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
        )
        .map_err(rusqlite_to_eng_error)?;
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
        let mut stmt = conn
            .prepare("SELECT profile, is_stale FROM personality_profiles WHERE user_id = ?1")
            .map_err(rusqlite_to_eng_error)?;

        let mut rows = stmt
            .query(rusqlite::params![user_id])
            .map_err(rusqlite_to_eng_error)?;

        match rows.next().map_err(rusqlite_to_eng_error)? {
            Some(row) => {
                let profile: String = row.get(0).map_err(rusqlite_to_eng_error)?;
                let is_stale: i32 = row.get(1).map_err(rusqlite_to_eng_error)?;
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
    extract_signals_rule_based(content)
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
        let mut stmt = conn
            .prepare(
                "INSERT INTO personality_signals (signal_type, value, evidence, user_id, agent)
             VALUES (?1, ?2, ?3, ?4, ?5)
             RETURNING id, signal_type, value, evidence, user_id, agent, created_at",
            )
            .map_err(rusqlite_to_eng_error)?;

        let row = stmt
            .query_row(
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
            )
            .map_err(rusqlite_to_eng_error)?;

        Ok(row)
    })
    .await
}

#[tracing::instrument(skip(db), fields(user_id, limit))]
pub async fn list_signals(db: &Database, user_id: i64, limit: usize) -> Result<Vec<StoredSignal>> {
    let limit = limit as i64;
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, signal_type, value, evidence, user_id, agent, created_at
             FROM personality_signals
             WHERE user_id = ?1
             ORDER BY created_at DESC
             LIMIT ?2",
            )
            .map_err(rusqlite_to_eng_error)?;

        let rows = stmt
            .query_map(rusqlite::params![user_id, limit], |row| {
                Ok(StoredSignal {
                    id: row.get(0)?,
                    signal_type: row.get(1)?,
                    value: row.get(2)?,
                    evidence: row.get(3)?,
                    user_id: row.get(4)?,
                    agent: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })
            .map_err(rusqlite_to_eng_error)?;

        let mut signals = Vec::new();
        for row in rows {
            signals.push(row.map_err(rusqlite_to_eng_error)?);
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
        ).map_err(rusqlite_to_eng_error)?;

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
        let mut stmt = conn
            .prepare(
                "SELECT user_id, traits, last_updated_at, created_at
             FROM personality_profiles
             WHERE user_id = ?1",
            )
            .map_err(rusqlite_to_eng_error)?;

        let mut rows = stmt
            .query(rusqlite::params![user_id])
            .map_err(rusqlite_to_eng_error)?;

        match rows.next().map_err(rusqlite_to_eng_error)? {
            Some(row) => {
                let traits_json: String = row.get(1).map_err(rusqlite_to_eng_error)?;
                Ok(Some(StoredProfile {
                    user_id: row.get(0).map_err(rusqlite_to_eng_error)?,
                    traits: serde_json::from_str(&traits_json).unwrap_or(serde_json::json!({})),
                    last_updated_at: row.get(2).map_err(rusqlite_to_eng_error)?,
                    created_at: row.get(3).map_err(rusqlite_to_eng_error)?,
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
        assert_eq!(clean_subject("  a dog  "), "dog");
        assert_eq!(clean_subject("the quick fox"), "quick fox");
        assert_eq!(clean_subject("my favorite thing"), "favorite thing");
    }

    #[test]
    fn test_split_sentences() {
        let sentences = split_sentences("Hello world. How are you? I am fine!");
        assert_eq!(sentences.len(), 3);
    }

    #[test]
    fn test_extract_like() {
        let signals = extract_signals_template("I love programming and building things.");
        assert!(!signals.is_empty(), "Should extract at least one signal");
        let pref = signals
            .iter()
            .find(|s| s.signal_type == SignalType::Preference);
        assert!(pref.is_some(), "Should find a preference signal");
        assert_eq!(pref.unwrap().valence, Valence::Positive);
    }

    #[test]
    fn test_extract_dislike() {
        let signals = extract_signals_template("I hate waking up early.");
        assert!(!signals.is_empty(), "Should extract at least one signal");
        let pref = signals
            .iter()
            .find(|s| s.signal_type == SignalType::Preference);
        assert!(pref.is_some(), "Should find a preference signal");
        assert_eq!(pref.unwrap().valence, Valence::Negative);
    }

    #[test]
    fn test_extract_decision() {
        let signals = extract_signals_template("I decided to quit my job.");
        let decision = signals
            .iter()
            .find(|s| s.signal_type == SignalType::Decision);
        assert!(decision.is_some(), "Should find a decision signal");
    }

    #[test]
    fn test_extract_identity() {
        let signals = extract_signals_template("I am a software developer.");
        let identity = signals
            .iter()
            .find(|s| s.signal_type == SignalType::Identity);
        assert!(identity.is_some(), "Should find an identity signal");
    }

    #[test]
    fn test_extract_emotion() {
        let signals = extract_signals_template("I feel really excited about this project.");
        let emotion = signals
            .iter()
            .find(|s| s.signal_type == SignalType::Emotion);
        assert!(emotion.is_some(), "Should find an emotion signal");
    }

    #[test]
    fn test_rule_based_values() {
        let signals =
            extract_signals_rule_based("I believe in open source software and community.");
        let value = signals.iter().find(|s| s.signal_type == SignalType::Value);
        assert!(value.is_some(), "Should find a value signal");
    }

    #[test]
    fn test_rule_based_motivation() {
        let signals = extract_signals_rule_based("I want to learn Rust and systems programming.");
        let motivation = signals
            .iter()
            .find(|s| s.signal_type == SignalType::Motivation);
        assert!(motivation.is_some(), "Should find a motivation signal");
    }

    #[test]
    fn test_rule_based_intensifier() {
        let base = extract_signals_template("I love cooking.");
        let intensified = extract_signals_rule_based("I really love cooking!");
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
        let signals = extract_signals_template("Hi.");
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
}
