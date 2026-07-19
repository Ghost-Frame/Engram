// ============================================================================
// Lexicon -- i18n core module.
//
// Centralises multilingual word lists and (later) regex overrides used by
// the intelligence pipeline (extraction.rs, personality.rs, valence.rs,
// sentiment.rs, etc.). These sites read their word classes from here
// instead of the hardcoded constants they previously embedded.
//
// Public API:
//   - word_class(lang, class)             -> Vec<String>
//   - word_class_alternation(lang, class) -> String (joined with '|')
//   - supported_languages()               -> Vec<String>
//   - complex_regex(id)                   -> Option<String> (stub for Layer B)
//
// Cascade for the source of truth, in priority order:
//   1. `KLEOS_LEXICON_REPOSITORY` env var (explicit, any directory).
//   2. `KLEOS_DATA_DIR/lexicon` (or `ENGRAM_DATA_DIR/lexicon`) when present.
//   3. Embedded baselines: `lexicon/en.toml`, `lexicon/fr.toml`.
//
// Format of each `<lang>.toml` file (see also `kleos-lib/lexicon/en.toml`
// and `fr.toml` for the embedded reference):
//
//     schema_version = 1
//     language = "en"
//
//     [classes.verb_like]
//     words = ["love", "like", ...]
//
//     [classes.emotion_happy]
//     words = ["happy", "joyful", ...]
//     valence = 0.7
//     intensity = 0.6
// ============================================================================

mod cache;
mod loader;

use std::sync::OnceLock;

use rust_stemmers::{Algorithm, Stemmer};
use unicode_normalization::UnicodeNormalization;

use loader::ParsedLexicon;

/// Embedded EN baseline. Parsed once on first access and panics if the file
/// is malformed (a malformed embedded baseline is a build-time bug that must
/// surface immediately, not a runtime warning).
fn embedded_en() -> &'static ParsedLexicon {
    static EN: OnceLock<ParsedLexicon> = OnceLock::new();
    EN.get_or_init(|| {
        loader::parse(include_str!("../../lexicon/en.toml"))
            .expect("embedded en.toml must parse (build-time guarantee)")
    })
}

/// Embedded FR baseline. Same rules as `embedded_en`.
fn embedded_fr() -> &'static ParsedLexicon {
    static FR: OnceLock<ParsedLexicon> = OnceLock::new();
    FR.get_or_init(|| {
        loader::parse(include_str!("../../lexicon/fr.toml"))
            .expect("embedded fr.toml must parse (build-time guarantee)")
    })
}

/// Embedded DE baseline. Same rules as `embedded_en`.
fn embedded_de() -> &'static ParsedLexicon {
    static DE: OnceLock<ParsedLexicon> = OnceLock::new();
    DE.get_or_init(|| {
        loader::parse(include_str!("../../lexicon/de.toml"))
            .expect("embedded de.toml must parse (build-time guarantee)")
    })
}

/// List of language codes that have an embedded baseline. The override repo
/// can add others on top via files named `<lang>.toml`.
const EMBEDDED_LANGS: &[&str] = &["en", "fr", "de"];

fn embedded(lang: &str) -> Option<&'static ParsedLexicon> {
    match lang {
        "en" => Some(embedded_en()),
        "fr" => Some(embedded_fr()),
        "de" => Some(embedded_de()),
        _ => None,
    }
}

/// Look up the words for a given semantic class in a given language.
///
/// Returns an empty `Vec` (not an error) when the language or class is
/// unknown. The empty result is a deliberate API choice: callers iterate
/// `supported_languages()` and concatenate matches across all languages,
/// so silently-empty buckets are the common case and must not raise.
pub fn word_class(lang: &str, class: &str) -> Vec<String> {
    // 1. Override repo wins when available.
    if let Some(repo) = cache::repo_root() {
        if let Some(parsed) = cache::resolve_override(repo, lang) {
            if let Some(class_entry) = parsed.classes.get(class) {
                return class_entry.words.clone();
            }
            // Override file exists but does not define this class: fall back
            // to the embedded baseline if any. This lets operators write thin
            // override files that only patch specific classes without having
            // to redeclare everything.
        }
    }
    // 2. Embedded baseline.
    embedded(lang)
        .and_then(|p| p.classes.get(class))
        .map(|c| c.words.clone())
        .unwrap_or_default()
}

/// Convenience: pipe-joined alternation suitable for direct interpolation
/// into a regex template. Each word is `regex::escape`-ed so a lexicon entry
/// containing regex metacharacters (`.`, `*`, `(`, and the like) cannot
/// produce a broken or injectable pattern. For inflected matching that also
/// stems each word, use [`word_class_alternation_stemmed`].
pub fn word_class_alternation(lang: &str, class: &str) -> String {
    word_class(lang, class)
        .iter()
        .map(|w| regex::escape(w))
        .collect::<Vec<_>>()
        .join("|")
}

/// Pipe-joined alternation of the **stemmed** words for a class. The
/// stemming respects `stem = false` on the class (grammar / particle
/// classes pass through untouched). Each word is `regex::escape`-ed so
/// special characters in the stemmed form do not break the surrounding
/// regex template.
///
/// Helper. Use this when the regex compiled from the
/// alternation will be matched against **raw source text** AND you want
/// inflected forms to match: pair this with a `\w*` wildcard after the
/// non-capturing group in the template. Example:
///
/// ```ignore
/// let alt = word_class_alternation_stemmed("fr", "verb_like"); // "aim|ador|appreci|prefer"
/// let pattern = format!(r"(?i)\b(?:{alt})\w*\b\s+(.+?)(?:\.|,|$)");
/// // matches "j'aime", "j'aimais", "j'aimerais", "j'adore", ...
/// ```
///
/// Multi-word entries (`burned out`, `en colere`) are split-stemmed-rejoined
/// by `fold_for_matching`, so the resulting alternation contains
/// space-separated stems (`burn out`, `en coler`) which still match the
/// raw source via case-insensitive flag and the trailing wildcard.
pub fn word_class_alternation_stemmed(lang: &str, class: &str) -> String {
    let with_stem = class_stem_enabled(lang, class);
    word_class(lang, class)
        .iter()
        .map(|w| regex::escape(&fold_for_matching(w, lang, with_stem)))
        .collect::<Vec<_>>()
        .join("|")
}

/// Enumerate every language that the lexicon module can serve. Includes the
/// embedded baselines plus any `<lang>.toml` file present in the override
/// repo (if configured). Result is sorted and de-duplicated.
pub fn supported_languages() -> Vec<String> {
    let mut langs: Vec<String> = EMBEDDED_LANGS.iter().map(|s| s.to_string()).collect();

    if let Some(repo) = cache::repo_root() {
        if let Ok(entries) = std::fs::read_dir(repo) {
            for entry in entries.flatten() {
                if entry.path().extension().and_then(|e| e.to_str()) != Some("toml") {
                    continue;
                }
                if let Some(stem) = entry.path().file_stem().and_then(|s| s.to_str()) {
                    langs.push(stem.to_string());
                }
            }
        }
    }

    langs.sort();
    langs.dedup();
    langs
}

/// Map a language code to its Snowball stemmer algorithm, when one exists.
/// Returns `None` for languages we do not have a stemmer for (the caller
/// falls back to lowercase + strip-accents only).
fn stemmer_for(lang: &str) -> Option<Stemmer> {
    let algo = match lang {
        "en" => Algorithm::English,
        "fr" => Algorithm::French,
        "de" => Algorithm::German,
        "es" => Algorithm::Spanish,
        "it" => Algorithm::Italian,
        "pt" => Algorithm::Portuguese,
        "nl" => Algorithm::Dutch,
        "ru" => Algorithm::Russian,
        "ar" => Algorithm::Arabic,
        "ro" => Algorithm::Romanian,
        "sv" => Algorithm::Swedish,
        "no" => Algorithm::Norwegian,
        "fi" => Algorithm::Finnish,
        "hu" => Algorithm::Hungarian,
        "da" => Algorithm::Danish,
        "tr" => Algorithm::Turkish,
        _ => return None,
    };
    Some(Stemmer::create(algo))
}

/// Strip diacritical marks from a string via Unicode NFD decomposition,
/// then filter out combining marks (category Mn), then re-compose to NFC.
/// `école` becomes `ecole`, `déçu` becomes `decu`, `naïve` becomes `naive`.
fn strip_diacritics(s: &str) -> String {
    use unicode_normalization::char::is_combining_mark;
    s.nfd().filter(|c| !is_combining_mark(*c)).nfc().collect()
}

/// Fold a single token or multi-word phrase for matching.
///
/// Pipeline: lowercase -> optional Snowball stem (per token if the input has
/// spaces) -> strip diacritics. `with_stem = false` skips the morphological
/// step (used for grammar-word classes) and just lowercases + strips.
///
/// The stem step runs BEFORE diacritic stripping on purpose: the Snowball
/// French (and Romance/Nordic) algorithms use diacritics to identify endings,
/// so stripping first would mis-stem inflected forms (for example the French
/// past participle "aimee" would not reduce to the same stem as "aimer").
/// Stripping the diacritics from the resulting stem keeps the final key
/// accent-invariant either way.
///
/// The function is invoked at both ends of a comparison so that the
/// match is invariant to diacritics, casing, and (when stemming
/// applies) inflection.
pub fn fold_for_matching(s: &str, lang: &str, with_stem: bool) -> String {
    let lower = s.to_lowercase();
    if !with_stem {
        return strip_diacritics(&lower);
    }
    let Some(stemmer) = stemmer_for(lang) else {
        return strip_diacritics(&lower);
    };
    // Snowball operates on a single word. For multi-word entries (causal
    // phrases like "a cause de"), split on whitespace, stem each token on its
    // accented form, then rejoin with spaces to preserve the phrase structure.
    let stemmed = lower
        .split_whitespace()
        .map(|tok| stemmer.stem(tok).into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    strip_diacritics(&stemmed)
}

/// Look up whether `class` has `stem = true` (default) or `stem = false`
/// in the lexicon, then call `fold_for_matching` with the appropriate
/// stemming behavior. Use this when the class identity is known to the
/// caller (which is the typical pattern for Layer A sites).
pub fn fold_word_for_class(word: &str, lang: &str, class: &str) -> String {
    let with_stem = class_stem_enabled(lang, class);
    fold_for_matching(word, lang, with_stem)
}

/// Returns whether the given class allows morphological stemming.
/// Defaults to `true` when the class is unknown. Override lookups
/// follow the same cascade as `word_class`.
pub fn class_stem_enabled(lang: &str, class: &str) -> bool {
    if let Some(repo) = cache::repo_root() {
        if let Some(parsed) = cache::resolve_override(repo, lang) {
            if let Some(class_entry) = parsed.classes.get(class) {
                return class_entry.stem;
            }
        }
    }
    embedded(lang)
        .and_then(|p| p.classes.get(class))
        .map(|c| c.stem)
        .unwrap_or(true)
}

/// Stub for Layer B (): retrieve a complex regex
/// override by its dot-free id (e.g. `"intelligence.extraction.facts.fr.
/// negation_discontinuous"`). Always returns `None` for now; the
/// real implementation reads from `<repo>/patterns/<id>.toml`.
///
/// Exposed so consumer code can be written against the
/// final signature without churn.
pub fn complex_regex(_id: &str) -> Option<String> {
    None
}

/// Return the optional `(valence, arousal)` metadata for a valence
/// class. Used by intelligence/valence.rs::analyze_valence. Returns
/// `None` when the class is missing the metadata or when the language
/// is unknown.
pub fn class_valence_arousal(lang: &str, class: &str) -> Option<(f64, f64)> {
    let from_override = cache::repo_root()
        .and_then(|repo| cache::resolve_override(repo, lang))
        .and_then(|p| {
            p.classes
                .get(class)
                .and_then(|c| match (c.valence, c.arousal) {
                    (Some(v), Some(a)) => Some((v, a)),
                    _ => None,
                })
        });
    if from_override.is_some() {
        return from_override;
    }
    embedded(lang).and_then(|p| {
        p.classes
            .get(class)
            .and_then(|c| match (c.valence, c.arousal) {
                (Some(v), Some(a)) => Some((v, a)),
                _ => None,
            })
    })
}

/// Return the optional `(valence, intensity)` metadata for an emotion class.
/// Used by the personality.rs / valence.rs refactor. Returns `None`
/// when the class has no metadata or the language is unknown.
pub fn class_emotion_metadata(lang: &str, class: &str) -> Option<(f64, f64)> {
    let from_override = cache::repo_root()
        .and_then(|repo| cache::resolve_override(repo, lang))
        .and_then(|p| {
            p.classes
                .get(class)
                .and_then(|c| match (c.valence, c.intensity) {
                    (Some(v), Some(i)) => Some((v, i)),
                    _ => None,
                })
        });
    if from_override.is_some() {
        return from_override;
    }
    embedded(lang).and_then(|p| {
        p.classes
            .get(class)
            .and_then(|c| match (c.valence, c.intensity) {
                (Some(v), Some(i)) => Some((v, i)),
                _ => None,
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_en_loads_verb_like() {
        let words = word_class("en", "verb_like");
        assert!(words.contains(&"love".to_string()));
        assert!(words.contains(&"like".to_string()));
        assert!(words.contains(&"enjoy".to_string()));
        assert!(words.contains(&"adore".to_string()));
        assert!(words.contains(&"prefer".to_string()));
    }

    #[test]
    fn embedded_fr_loads_verb_like() {
        let words = word_class("fr", "verb_like");
        assert!(words.contains(&"aimer".to_string()));
        assert!(words.contains(&"adorer".to_string()));
        assert!(words.contains(&"apprécier".to_string()));
        assert!(words.contains(&"préférer".to_string()));
    }

    #[test]
    fn unknown_language_returns_empty() {
        let words = word_class("xx", "verb_like");
        assert!(words.is_empty());
    }

    #[test]
    fn unknown_class_returns_empty() {
        let words = word_class("en", "this_class_does_not_exist");
        assert!(words.is_empty());
    }

    #[test]
    fn supported_languages_includes_embedded_baselines() {
        let langs = supported_languages();
        assert!(langs.contains(&"en".to_string()));
        assert!(langs.contains(&"fr".to_string()));
        assert!(langs.contains(&"de".to_string()));
    }

    #[test]
    fn supported_languages_is_sorted_and_deduplicated() {
        let langs = supported_languages();
        let mut sorted = langs.clone();
        sorted.sort();
        assert_eq!(langs, sorted, "supported_languages must be sorted");
        let mut deduped = langs.clone();
        deduped.dedup();
        assert_eq!(langs, deduped, "supported_languages must be deduplicated");
    }

    #[test]
    fn word_class_alternation_pipes_words() {
        let alt = word_class_alternation("en", "verb_like");
        // Order is whatever the embedded TOML defined; check we see at
        // least the canonical entries separated by '|'.
        assert!(alt.contains("love"));
        assert!(alt.contains("|"));
    }

    #[test]
    fn complex_regex_returns_none_in_livrable_1() {
        assert!(complex_regex("any.id").is_none());
    }

    #[test]
    fn fold_lowercases() {
        let folded = fold_for_matching("AbC", "xx", false);
        assert_eq!(folded, "abc");
    }

    #[test]
    fn fold_strips_french_diacritics() {
        assert_eq!(fold_for_matching("e\u{0301}cole", "xx", false), "ecole");
        assert_eq!(fold_for_matching("de\u{0327}cu", "xx", false), "decu");
        assert_eq!(fold_for_matching("nai\u{0308}ve", "xx", false), "naive");
        // Precomposed forms also fold to the same shape.
        assert_eq!(fold_for_matching("\u{00e9}cole", "xx", false), "ecole");
    }

    #[test]
    fn fold_stems_french_conjugations() {
        // All four French forms should fold to the same stem.
        let infinitive = fold_for_matching("aimer", "fr", true);
        let past_participle = fold_for_matching("aimée", "fr", true);
        let plural = fold_for_matching("aimées", "fr", true);
        let imperfect = fold_for_matching("aimait", "fr", true);
        assert_eq!(infinitive, past_participle);
        assert_eq!(infinitive, plural);
        assert_eq!(infinitive, imperfect);
    }

    #[test]
    fn fold_stems_english_plurals_and_tenses() {
        let base = fold_for_matching("love", "en", true);
        assert_eq!(fold_for_matching("loves", "en", true), base);
        assert_eq!(fold_for_matching("loved", "en", true), base);
        assert_eq!(fold_for_matching("loving", "en", true), base);
    }

    #[test]
    fn fold_skips_stemming_when_disabled() {
        let stemmed = fold_for_matching("aimerions", "fr", true);
        let raw = fold_for_matching("aimerions", "fr", false);
        assert_eq!(raw, "aimerions");
        assert_ne!(stemmed, raw, "stemming must change the long form");
    }

    #[test]
    fn fold_unknown_language_falls_back_to_strip_only() {
        // No stemmer for "zz" -> the with_stem flag is ignored.
        assert_eq!(
            fold_for_matching("De\u{0301}cu", "zz", true),
            fold_for_matching("De\u{0301}cu", "zz", false),
        );
    }

    #[test]
    fn fold_multi_word_phrase_stems_each_token() {
        let folded = fold_for_matching("a cause de", "fr", true);
        // The output should still be a single phrase, with tokens joined
        // by single spaces. Snowball will leave the short tokens mostly
        // unchanged but the structure must be preserved.
        assert_eq!(folded.split_whitespace().count(), 3);
    }

    #[test]
    fn class_stem_default_is_true() {
        // Verb_like has no explicit `stem` field in the embedded TOML and
        // should default to true.
        assert!(class_stem_enabled("en", "verb_like"));
        assert!(class_stem_enabled("fr", "verb_like"));
    }

    #[test]
    fn fold_word_for_class_respects_stem_metadata() {
        // Verb_like stems by default: "aimerions" should change.
        let stemmed = fold_word_for_class("aimerions", "fr", "verb_like");
        assert_ne!(stemmed, "aimerions");
    }

    #[test]
    fn class_emotion_metadata_returns_some_for_emotion_classes() {
        let meta = class_emotion_metadata("en", "emotion_happy");
        assert!(
            meta.is_some(),
            "emotion_happy must expose (valence, intensity)"
        );
        let (valence, intensity) = meta.unwrap();
        assert!(valence > 0.0, "happy is positive valence");
        assert!(intensity > 0.0 && intensity <= 1.0);
    }

    #[test]
    fn class_emotion_metadata_returns_none_for_layer_a_classes() {
        // Articles / stopwords have no valence/intensity, so metadata must
        // be None even though the class exists.
        assert!(class_emotion_metadata("en", "articles").is_none());
        assert!(class_emotion_metadata("en", "stopwords").is_none());
    }

    #[test]
    fn embedded_en_has_full_class_set() {
        // Smoke test that the embedded EN baseline declares every class the
        // the refactored sites need. Any class missing here will cause
        // an empty result downstream, so we check upfront.
        for class in [
            "verb_like",
            "verb_dislike",
            "verb_buy",
            "state_verbs",
            "articles",
            "stopwords",
            "first_person_pronoun",
            "emotion_happy",
            "emotion_sad",
            "intensifier_strong",
            "negation_marker",
            "causal_strong",
            "causal_context",
            "causal_weak",
            "filler_prefixes",
            "meta_stoplist",
            "credential_keywords",
            "prohibition_marker",
        ] {
            assert!(
                !word_class("en", class).is_empty(),
                "embedded en.toml is missing class {class}",
            );
        }
    }

    #[test]
    fn embedded_fr_has_full_class_set() {
        // Same coverage check for the FR baseline.
        for class in [
            "verb_like",
            "verb_dislike",
            "verb_buy",
            "state_verbs",
            "articles",
            "stopwords",
            "first_person_pronoun",
            "emotion_happy",
            "emotion_sad",
            "intensifier_strong",
            "negation_marker",
            "causal_strong",
            "causal_context",
            "causal_weak",
            "filler_prefixes",
            "meta_stoplist",
            "credential_keywords",
            "prohibition_marker",
        ] {
            assert!(
                !word_class("fr", class).is_empty(),
                "embedded fr.toml is missing class {class}",
            );
        }
    }

    #[test]
    fn embedded_de_loads_verb_like() {
        let words = word_class("de", "verb_like");
        assert!(words.contains(&"lieben".to_string()));
        assert!(words.contains(&"mögen".to_string()));
        assert!(words.contains(&"genießen".to_string()));
        assert!(words.contains(&"bevorzugen".to_string()));
    }

    #[test]
    fn embedded_de_has_full_class_set() {
        // Same coverage check for the DE baseline.
        for class in [
            "verb_like",
            "verb_dislike",
            "verb_buy",
            "state_verbs",
            "articles",
            "stopwords",
            "first_person_pronoun",
            "emotion_happy",
            "emotion_sad",
            "intensifier_strong",
            "negation_marker",
            "causal_strong",
            "causal_context",
            "causal_weak",
            "filler_prefixes",
            "meta_stoplist",
            "credential_keywords",
            "prohibition_marker",
        ] {
            assert!(
                !word_class("de", class).is_empty(),
                "embedded de.toml is missing class {class}",
            );
        }
    }

    #[test]
    fn fold_stems_german_noun_plurals() {
        // German Snowball handles noun declension well: singular and plural
        // forms of the same noun should collapse to the same stem root.
        // "Stunde" (hour, singular) and "Stunden" (plural) both -> "stund".
        let singular = fold_for_matching("Stunde", "de", true);
        let plural = fold_for_matching("Stunden", "de", true);
        assert_eq!(
            singular, plural,
            "Stunde and Stunden should stem to the same root"
        );
        // Also verify the DE stemmer is active at all: the stem must differ
        // from the raw-folded plural (i.e., the -en suffix is actually stripped).
        let raw = fold_for_matching("Stunden", "de", false);
        assert_ne!(
            plural, raw,
            "Snowball-German stemmer must change 'Stunden' (stem != raw fold)"
        );
    }

    // Helper tests.

    #[test]
    fn word_class_alternation_stemmed_fr_verbs_match_inflected_forms() {
        // The FR `verb_like` class lists infinitives (aimer, adorer, ...).
        // After stemming, the alternation collapses to the Snowball roots,
        // and the trailing `\w*` wildcard in the consumer template makes
        // every inflected form match (aime, aimait, aimerions, adorera).
        let alt = word_class_alternation_stemmed("fr", "verb_like");
        let pattern = format!(r"(?i)\b(?:{alt})\w*\b");
        let re = regex::Regex::new(&pattern).expect("compiled");
        for inflected in &[
            "j'aime",
            "j'aimais",
            "j'aimerions",
            "j'adore",
            "j'adorerais",
        ] {
            assert!(
                re.is_match(inflected),
                "FR inflected `{inflected}` must match"
            );
        }
    }

    #[test]
    fn word_class_alternation_stemmed_en_baseline_preserved() {
        // The EN `verb_like` class lists base forms (like, love, ...).
        // Stem roots stay close to the base form for English (like -> like,
        // loved -> love), and the trailing wildcard adds `s`/`d`/`ing`.
        let alt = word_class_alternation_stemmed("en", "verb_like");
        let pattern = format!(r"(?i)\b(?:{alt})\w*\b");
        let re = regex::Regex::new(&pattern).expect("compiled");
        for variant in &["I like", "I likes", "she loved", "they enjoy", "we adore"] {
            assert!(re.is_match(variant), "EN variant `{variant}` must match");
        }
    }

    #[test]
    fn word_class_alternation_stemmed_respects_stem_false() {
        // Grammar classes (articles, stopwords, particles) are declared
        // `stem = false` in the TOMLs. The stemmed helper must return the
        // raw words unchanged for those classes, otherwise short tokens
        // like `le` would be munged.
        let stemmed = word_class_alternation_stemmed("fr", "articles");
        let raw = word_class_alternation("fr", "articles");
        // After fold_for_matching with stem=false we still lowercase +
        // strip diacritics, so the comparison is against a folded raw.
        let raw_folded: String = raw
            .split('|')
            .map(|w| regex::escape(&fold_for_matching(w, "fr", false)))
            .collect::<Vec<_>>()
            .join("|");
        assert_eq!(stemmed, raw_folded);
    }

    #[test]
    fn word_class_alternation_stemmed_unknown_class_empty() {
        assert!(word_class_alternation_stemmed("en", "this_class_does_not_exist").is_empty());
        assert!(word_class_alternation_stemmed("xx", "verb_like").is_empty());
    }

    #[test]
    fn word_class_alternation_stemmed_multiword_entries_handled() {
        // `valence_anger_intense` for EN typically contains multi-word
        // forms like `burned out`. fold_for_matching splits, stems each
        // token, and rejoins -- the alternation entry remains a single
        // pipe-delimited segment with an internal space.
        let alt = word_class_alternation_stemmed("en", "valence_anger_intense");
        if alt.is_empty() {
            // Some embedded baselines may not carry this exact class;
            // skip silently to keep the test resilient to TOML edits.
            return;
        }
        // Compile must not fail; the escaped pipe-joined string is a
        // valid regex body whether or not it contains spaces.
        let pattern = format!(r"(?i)\b(?:{alt})\w*\b");
        regex::Regex::new(&pattern).expect("multi-word alternation compiles");
    }
}
