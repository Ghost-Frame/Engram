// ============================================================================
// Lexicon -- TOML parser.
//
// Parses a lexicon file into a typed `ParsedLexicon`. Format:
//
//     schema_version = 1
//     language = "fr"
//
//     [classes.verb_like]
//     words = ["aimer", "adorer"]
//
//     [classes.emotion_happy]
//     words = ["heureux", "content"]
//     valence = 0.7
//     intensity = 0.6
//
// The nested `[classes.X]` table layout (vs a top-level flat layout) is
// chosen to keep serde / toml deserialisation predictable: a flatten map
// at top-level mixes scalar fields and tables in ways that vary between
// toml backends. The explicit nesting trades 1 keyword (`classes.`) per
// section for parser stability.
// ============================================================================

use std::collections::HashMap;

use serde::Deserialize;

/// Parsed representation of a `<lang>.toml` lexicon file.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct ParsedLexicon {
    /// Forward-compat marker for future schema migrations.
    #[serde(default = "default_schema_version")]
    #[allow(dead_code)]
    pub schema_version: u32,
    /// ISO-639-style language code (e.g. "en", "fr"). Currently informational
    /// only; the consumer-facing key is the file basename.
    #[serde(default)]
    #[allow(dead_code)]
    pub language: String,
    /// Map of class name (e.g. "verb_like") to its content (words + optional
    /// metadata). Missing in a file is fine; the embedded baseline supplies
    /// the defaults.
    #[serde(default)]
    pub classes: HashMap<String, LexiconClass>,
}

fn default_schema_version() -> u32 {
    1
}

/// One semantic class inside a lexicon: a list of canonical words plus
/// optional emotional / sentiment metadata. Only `words` is required;
/// `valence` and `intensity` are reserved for callers that
/// consume `emotion_*` classes (personality.rs, valence.rs).
#[derive(Debug, Clone, Deserialize)]
pub(super) struct LexiconClass {
    #[serde(default)]
    pub words: Vec<String>,
    /// Emotional valence in [-1.0, 1.0]. Only populated for `emotion_*`
    /// classes; other classes leave this as None.
    #[serde(default)]
    #[allow(dead_code)]
    pub valence: Option<f64>,
    /// Emotional intensity in [0.0, 1.0]. Same conventions as `valence`.
    #[serde(default)]
    #[allow(dead_code)]
    pub intensity: Option<f64>,
    /// Arousal in [0.0, 1.0]. Used by valence.rs EMOTION_PATTERNS to
    /// distinguish high-energy vs low-energy emotional signals
    /// (e.g. enraged = 0.9, sad = 0.3, calm = 0.1).
    #[serde(default)]
    #[allow(dead_code)]
    pub arousal: Option<f64>,
    /// Opt-out of morphological stemming during matching. Defaults to
    /// `true` (stem). Set to `false` for grammar-word classes where
    /// stemming would over-collapse semantics (state_verbs: "est" ->
    /// "et"; first_person_pronoun: "je" -> "j"; etc.).
    #[serde(default = "default_stem")]
    pub stem: bool,
}

fn default_stem() -> bool {
    true
}

/// Parse error returned by `parse`. We keep this lightweight (string error)
/// because the consumer only needs to log it and fall back.
#[derive(Debug)]
pub(super) struct ParseError {
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "lexicon parse error: {}", self.message)
    }
}

impl std::error::Error for ParseError {}

/// Parse a TOML lexicon source. On failure, returns a `ParseError` whose
/// message reflects the underlying toml diagnostic.
pub(super) fn parse(src: &str) -> Result<ParsedLexicon, ParseError> {
    toml::from_str::<ParsedLexicon>(src).map_err(|e| ParseError {
        message: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_file() {
        let src = r#"
schema_version = 1
language = "en"

[classes.verb_like]
words = ["love", "like"]
"#;
        let parsed = parse(src).expect("should parse");
        assert_eq!(parsed.language, "en");
        assert_eq!(parsed.classes.len(), 1);
        let verb_like = parsed.classes.get("verb_like").expect("verb_like present");
        assert_eq!(
            verb_like.words,
            vec!["love".to_string(), "like".to_string()]
        );
    }

    #[test]
    fn parse_class_with_metadata() {
        let src = r#"
schema_version = 1
language = "fr"

[classes.emotion_happy]
words = ["heureux", "content"]
valence = 0.7
intensity = 0.6
"#;
        let parsed = parse(src).expect("should parse");
        let happy = parsed
            .classes
            .get("emotion_happy")
            .expect("emotion_happy present");
        assert_eq!(
            happy.words,
            vec!["heureux".to_string(), "content".to_string()]
        );
        assert_eq!(happy.valence, Some(0.7));
        assert_eq!(happy.intensity, Some(0.6));
    }

    #[test]
    fn parse_missing_classes_is_ok() {
        // A file with only header fields and no [classes.*] block is valid;
        // the resulting parsed map is empty.
        let src = r#"
schema_version = 1
language = "xx"
"#;
        let parsed = parse(src).expect("should parse");
        assert_eq!(parsed.language, "xx");
        assert!(parsed.classes.is_empty());
    }

    #[test]
    fn parse_invalid_toml_returns_err() {
        let src = "this is not valid TOML [[[";
        let err = parse(src).expect_err("should fail to parse");
        assert!(err.to_string().contains("lexicon parse error"));
    }

    #[test]
    fn parse_class_with_only_words() {
        // The simplest class (no metadata) is the dominant case for Layer A
        // sites (articles, stopwords, etc.).
        let src = r#"
schema_version = 1
language = "en"

[classes.articles]
words = ["the", "a", "an"]
"#;
        let parsed = parse(src).expect("should parse");
        let articles = parsed.classes.get("articles").expect("articles present");
        assert_eq!(articles.words.len(), 3);
        assert!(articles.valence.is_none());
        assert!(articles.intensity.is_none());
    }
}
