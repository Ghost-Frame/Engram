// ============================================================================
// SENTIMENT LEXICON -- AFINN-style word map for rule-based personality analysis
// Values range from -5 (most negative) to +5 (most positive).
//
// Words are sourced from the i18n lexicon (sentiment_pos5 through sentiment_neg5
// classes per language). Words are folded with fold_word_for_class so inflected
// forms match at query time. Language detection routes each text to the correct
// per-language map, eliminating cross-language homograph collisions.
// ============================================================================

use std::collections::HashMap;
use std::sync::LazyLock;

/// Sentiment bucket names mapped to their AFINN-style integer score.
const SENTIMENT_BUCKETS: &[(&str, i32)] = &[
    ("sentiment_pos5", 5),
    ("sentiment_pos4", 4),
    ("sentiment_pos3", 3),
    ("sentiment_pos2", 2),
    ("sentiment_pos1", 1),
    ("sentiment_neg1", -1),
    ("sentiment_neg2", -2),
    ("sentiment_neg3", -3),
    ("sentiment_neg4", -4),
    ("sentiment_neg5", -5),
];

/// Per-language sentiment lookup tables built once at first access.
///
/// Outer key: ISO 639-1 language code (e.g. `"en"`, `"fr"`, `"de"`).
/// Inner key: folded word (lowercased, Snowball-stemmed when the class has
/// `stem = true`, diacritics stripped). Inner value: AFINN integer score.
/// Each language is built independently so a word that appears in two language
/// lexicons maps to its own-language score without the arbitrary ordering
/// collision produced by a single flat union map.
// Frozen for the process lifetime: built once from kleos_lib::lexicon and
// never refreshed, so lexicon override edits on disk do not reach this cache
// without a restart. Any future lexicon hot-reload feature must invalidate
// this cache too, not just the lexicon module's own TTL cache.
static SENTIMENT_MAP: LazyLock<HashMap<String, HashMap<String, i32>>> = LazyLock::new(|| {
    let mut outer: HashMap<String, HashMap<String, i32>> = HashMap::new();
    for lang in crate::lexicon::supported_languages() {
        let inner = outer.entry(lang.clone()).or_default();
        for (class, score) in SENTIMENT_BUCKETS {
            for word in crate::lexicon::word_class(&lang, class) {
                let folded = crate::lexicon::fold_word_for_class(&word, &lang, class);
                // or_insert preserves the first (highest-intensity bucket) score
                // because SENTIMENT_BUCKETS is ordered pos5..neg5.
                inner.entry(folded).or_insert(*score);
            }
        }
    }
    outer
});

/// Fallback returned by `sentiment_map_for` when neither the requested language
/// nor the "en" baseline are present. Should not occur in production since "en"
/// is always embedded, but prevents a panic in stripped-down test builds.
static EMPTY_SENTIMENT_MAP: LazyLock<HashMap<String, i32>> = LazyLock::new(HashMap::new);

/// Return the folded-word-to-score map for `lang`, falling back to "en" when
/// the requested language is absent, and to an empty map when "en" is absent.
fn sentiment_map_for(lang: &str) -> &'static HashMap<String, i32> {
    if let Some(map) = SENTIMENT_MAP.get(lang) {
        return map;
    }
    if let Some(map) = SENTIMENT_MAP.get("en") {
        return map;
    }
    &EMPTY_SENTIMENT_MAP
}

// Embedded fallback list -- kept as a reference for tests that hardcode
// specific (word, score) pairs. SENTIMENT_MAP is the authoritative source.
// Wrapped in #[cfg(test)] so it does not contribute to the production binary.
#[cfg(test)]
#[allow(dead_code)]
static _SENTIMENT_LEXICON_REFERENCE: LazyLock<HashMap<&'static str, i32>> = LazyLock::new(|| {
    let entries: &[(&str, i32)] = &[
        // Strong positive (4-5)
        ("love", 5),
        ("adore", 5),
        ("excellent", 5),
        ("amazing", 5),
        ("fantastic", 5),
        ("brilliant", 5),
        ("wonderful", 5),
        ("outstanding", 5),
        ("superb", 5),
        ("perfect", 5),
        ("incredible", 5),
        ("magnificent", 5),
        ("phenomenal", 5),
        ("exceptional", 5),
        ("flawless", 5),
        ("spectacular", 5),
        ("breathtaking", 5),
        ("glorious", 5),
        ("marvelous", 5),
        ("supreme", 5),
        ("bliss", 4),
        ("blissful", 4),
        ("thrilled", 4),
        ("overjoyed", 4),
        ("ecstatic", 4),
        ("elated", 4),
        ("exhilarating", 4),
        ("exquisite", 4),
        ("fascinating", 4),
        ("inspiring", 4),
        ("joyful", 4),
        ("passionate", 4),
        ("radiant", 4),
        ("triumph", 4),
        ("triumphant", 4),
        ("victorious", 4),
        ("delightful", 4),
        ("euphoric", 4),
        ("masterful", 4),
        ("treasured", 4),
        // Moderate positive (2-3)
        ("good", 3),
        ("great", 3),
        ("nice", 3),
        ("enjoy", 3),
        ("happy", 3),
        ("helpful", 3),
        ("pleased", 3),
        ("glad", 3),
        ("like", 2),
        ("fine", 2),
        ("comfortable", 2),
        ("positive", 2),
        ("pleasant", 3),
        ("cheerful", 3),
        ("confident", 3),
        ("creative", 2),
        ("delighted", 3),
        ("eager", 3),
        ("enthusiastic", 3),
        ("friendly", 2),
        ("fun", 3),
        ("generous", 3),
        ("grateful", 3),
        ("honest", 2),
        ("hopeful", 2),
        ("kind", 2),
        ("lovely", 3),
        ("motivated", 3),
        ("optimistic", 3),
        ("proud", 3),
        ("refreshing", 3),
        ("reliable", 2),
        ("rewarding", 3),
        ("satisfied", 3),
        ("smile", 2),
        ("successful", 3),
        ("supportive", 2),
        ("trusted", 2),
        ("upbeat", 3),
        ("valued", 2),
        ("welcoming", 2),
        ("worthwhile", 3),
        // Mild positive (1)
        ("ok", 1),
        ("okay", 1),
        ("decent", 1),
        ("fair", 1),
        ("acceptable", 1),
        ("adequate", 1),
        ("alright", 1),
        ("reasonable", 1),
        ("solid", 1),
        ("calm", 1),
        ("steady", 1),
        ("workable", 1),
        ("useful", 1),
        ("functional", 1),
        ("stable", 1),
        ("manageable", 1),
        ("tolerable", 1),
        ("passable", 1),
        ("suitable", 1),
        ("serviceable", 1),
        ("bearable", 1),
        ("capable", 1),
        ("clear", 1),
        ("consistent", 1),
        ("constructive", 1),
        ("interesting", 1),
        ("neat", 1),
        ("polite", 1),
        ("practical", 1),
        ("professional", 1),
        // Mild negative (-1)
        ("boring", -1),
        ("mediocre", -1),
        ("bland", -1),
        ("dull", -1),
        ("average", -1),
        ("forgettable", -1),
        ("plain", -1),
        ("flat", -1),
        ("meh", -1),
        ("uninspiring", -1),
        ("uninteresting", -1),
        ("lackluster", -1),
        ("generic", -1),
        ("basic", -1),
        ("stale", -1),
        ("mundane", -1),
        ("tedious", -1),
        ("tiresome", -1),
        ("uninspired", -1),
        ("unremarkable", -1),
        ("vague", -1),
        ("weak", -1),
        ("overrated", -1),
        ("hollow", -1),
        ("dry", -1),
        ("repetitive", -1),
        ("redundant", -1),
        ("routine", -1),
        ("predictable", -1),
        ("ordinary", -1),
        // Moderate negative (-2 to -3)
        ("bad", -2),
        ("annoying", -2),
        ("frustrating", -3),
        ("disappointing", -2),
        ("difficult", -2),
        ("ugly", -2),
        ("poor", -2),
        ("slow", -2),
        ("confusing", -2),
        ("awkward", -2),
        ("broken", -3),
        ("buggy", -2),
        ("clunky", -2),
        ("complicated", -2),
        ("corrupt", -3),
        ("cruel", -3),
        ("dangerous", -3),
        ("dirty", -2),
        ("failed", -2),
        ("failure", -3),
        ("flawed", -2),
        ("harmful", -3),
        ("harsh", -2),
        ("hostile", -3),
        ("ignorant", -2),
        ("inefficient", -2),
        ("inferior", -2),
        ("insulting", -3),
        ("irritating", -2),
        ("lazy", -2),
        ("limited", -2),
        ("messy", -2),
        ("misleading", -3),
        ("offensive", -3),
        ("problematic", -2),
        ("reckless", -3),
        ("rude", -3),
        ("sad", -2),
        ("stupid", -3),
        ("terrible", -3),
        ("unpleasant", -2),
        ("unreliable", -3),
        ("unstable", -2),
        ("useless", -3),
        ("wasteful", -2),
        ("wrong", -2),
        // Strong negative (-4 to -5)
        ("hate", -5),
        ("awful", -5),
        ("horrible", -5),
        ("disgusting", -5),
        ("dreadful", -5),
        ("miserable", -4),
        ("toxic", -4),
        ("devastating", -5),
        ("atrocious", -5),
        ("abysmal", -5),
        ("abominable", -5),
        ("appalling", -5),
        ("catastrophic", -5),
        ("despicable", -5),
        ("disastrous", -5),
        ("evil", -5),
        ("execrable", -5),
        ("horrendous", -5),
        ("loathe", -5),
        ("loathing", -5),
        ("monstrous", -5),
        ("nightmarish", -5),
        ("pathetic", -4),
        ("repulsive", -5),
        ("revolting", -5),
        ("ruinous", -5),
        ("terrifying", -4),
        ("vile", -5),
        ("wretched", -5),
        ("unbearable", -4),
        ("unacceptable", -4),
        ("outrageous", -4),
        ("infuriating", -4),
        ("agonizing", -4),
        ("depressing", -4),
        ("deplorable", -4),
        ("demoralizing", -4),
        ("degrading", -4),
        ("humiliating", -4),
        ("shameful", -4),
        ("pitiful", -4),
        // Emotional words
        ("excited", 3),
        ("anxious", -2),
        ("worried", -2),
        ("nervous", -1),
        ("content", 2),
        ("melancholy", -2),
        ("hopeless", -4),
        ("desperate", -3),
        ("lonely", -2),
        ("isolated", -2),
        ("overwhelmed", -3),
        ("stressed", -2),
        ("relieved", 2),
        ("energized", 3),
        ("exhausted", -2),
        ("bored", -1),
        ("apathetic", -2),
        ("inspired", 3),
        ("discouraged", -2),
        ("determined", 2),
        ("fearful", -3),
        ("courageous", 3),
        ("ashamed", -3),
        ("embarrassed", -2),
        ("peaceful", 3),
        ("restless", -1),
        ("irritable", -2),
        ("nostalgic", 1),
        ("regretful", -2),
        ("resentful", -3),
        ("envious", -2),
        ("jealous", -2),
        ("suspicious", -1),
        ("conflicted", -1),
        ("curious", 1),
        ("baffled", -1),
        ("shocked", -1),
        ("surprised", 1),
        ("amused", 2),
        ("disturbed", -2),
        ("horrified", -4),
        ("intimidated", -2),
        ("offended", -3),
        ("betrayed", -4),
        ("humiliated", -4),
        ("vindicated", 3),
        ("empowered", 3),
        // Technical opinion words
        ("elegant", 3),
        ("hacky", -2),
        ("clean", 2),
        ("verbose", -1),
        ("concise", 2),
        ("efficient", 2),
        ("scalable", 2),
        ("maintainable", 2),
        ("readable", 2),
        ("fragile", -2),
        ("robust", 3),
        ("resilient", 2),
        ("performant", 2),
        ("bloated", -2),
        ("convoluted", -2),
        ("overengineered", -2),
        ("underengineered", -1),
        ("deprecated", -1),
        ("outdated", -1),
        ("modern", 1),
        ("innovative", 3),
        ("intuitive", 2),
        ("cumbersome", -2),
        ("seamless", 3),
        ("automated", 2),
        ("responsive", 2),
        ("sluggish", -2),
        ("fast", 2),
        ("powerful", 3),
        ("flexible", 2),
        ("rigid", -1),
        ("polished", 2),
        ("rough", -1),
        ("complete", 2),
        ("incomplete", -1),
        ("documented", 2),
        ("undocumented", -1),
        ("tested", 2),
        ("untested", -2),
        ("secure", 2),
        ("vulnerable", -3),
        ("patched", 1),
        ("working", 1),
    ];
    let mut map = HashMap::with_capacity(entries.len());
    for &(word, score) in entries {
        map.insert(word, score);
    }
    map
});

/// Look up the sentiment score for a single word in the given language.
/// Returns `None` when the word is absent from that language's lexicon.
/// The word is folded (lowercased, Snowball-stemmed, diacritics stripped)
/// before lookup so inflected forms match the same stem as dictionary entries.
pub fn score_word(word: &str, lang: &str) -> Option<i32> {
    // stem=true mirrors the folding applied at build time for sentiment classes.
    let folded = crate::lexicon::fold_for_matching(word, lang, true);
    sentiment_map_for(lang).get(&folded).copied()
}

/// Compute the average sentiment score for a block of text.
/// Words not in the lexicon are ignored. Returns `0.0` if no lexicon words
/// are found. Language is detected automatically from the text.
pub fn score_text(text: &str) -> f64 {
    let (sum, count) = score_text_sum(text);
    if count == 0 {
        0.0
    } else {
        sum as f64 / count as f64
    }
}

/// Compute the raw sentiment sum for a block of text using auto-detected language.
/// Returns `(sum, count)` where `count` is the number of lexicon words found.
/// Delegates to [`score_text_sum_for_lang`] after a single language detection call.
pub fn score_text_sum(text: &str) -> (i64, u32) {
    let lang = crate::lang::detect_lang(text);
    score_text_sum_for_lang(text, &lang)
}

/// Compute the raw sentiment sum for a block of text with an explicit language.
/// Lets callers bypass auto-detection when the language is already known,
/// returning `(sum, count)` where `count` is the number of lexicon words found.
pub fn score_text_sum_for_lang(text: &str, lang: &str) -> (i64, u32) {
    let mut sum: i64 = 0;
    let mut count: u32 = 0;
    let map = sentiment_map_for(lang);
    for word in text.split_whitespace() {
        // Strip leading/trailing punctuation before lookup.
        let cleaned: &str = word.trim_matches(|c: char| !c.is_alphanumeric());
        if cleaned.is_empty() {
            continue;
        }
        // Fold mirrors the build-time folding used when populating SENTIMENT_MAP.
        let folded = crate::lexicon::fold_for_matching(cleaned, lang, true);
        if let Some(&score) = map.get(&folded) {
            sum += score as i64;
            count += 1;
        }
    }
    (sum, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Positive English words score above zero via the EN lexicon path.
    #[test]
    fn score_word_positive() {
        assert_eq!(score_word("love", "en"), Some(5));
        assert_eq!(score_word("good", "en"), Some(3));
        assert_eq!(score_word("ok", "en"), Some(1));
    }

    /// Negative English words score below zero via the EN lexicon path.
    #[test]
    fn score_word_negative() {
        assert_eq!(score_word("hate", "en"), Some(-5));
        assert_eq!(score_word("bad", "en"), Some(-2));
        assert_eq!(score_word("boring", "en"), Some(-1));
    }

    /// Folding is case-insensitive so upper-case lookups match the lowercased map.
    #[test]
    fn score_word_case_insensitive() {
        assert_eq!(score_word("LOVE", "en"), Some(5));
        assert_eq!(score_word("Love", "en"), Some(5));
        assert_eq!(score_word("HATE", "en"), Some(-5));
    }

    /// Unknown words return `None` rather than a zero score.
    #[test]
    fn score_word_unknown() {
        assert_eq!(score_word("asdfghjkl", "en"), None);
        assert_eq!(score_word("the", "en"), None);
    }

    /// A strongly positive English sentence scores well above zero.
    #[test]
    fn score_text_positive() {
        let score = score_text("I love this amazing fantastic thing");
        assert!(score > 3.0, "Expected strongly positive, got {score}");
    }

    /// A strongly negative English sentence scores well below zero.
    #[test]
    fn score_text_negative() {
        let score = score_text("I hate this awful horrible thing");
        assert!(score < -3.0, "Expected strongly negative, got {score}");
    }

    /// Mixed positive and negative words cancel out near zero.
    #[test]
    fn score_text_mixed() {
        let score = score_text("I love the good parts but hate the bad parts");
        assert!(
            score.abs() < 2.0,
            "Expected near-zero for mixed, got {score}"
        );
    }

    /// Empty text returns exactly `0.0` (no division-by-zero panic).
    #[test]
    fn score_text_empty() {
        assert_eq!(score_text(""), 0.0);
    }

    /// Text containing only stopwords scores `0.0` (no lexicon hits).
    #[test]
    fn score_text_no_lexicon_words() {
        assert_eq!(score_text("the quick brown fox jumps over"), 0.0);
    }

    /// Punctuation attached to words is stripped before lookup.
    #[test]
    fn score_text_with_punctuation() {
        let score = score_text("This is amazing! Really love it.");
        assert!(
            score > 2.0,
            "Expected positive with punctuation, got {score}"
        );
    }

    /// Equal-magnitude positive and negative words sum to zero with count 2.
    #[test]
    fn score_text_sum_balanced() {
        let (sum, count) = score_text_sum("love hate");
        assert_eq!(sum, 0);
        assert_eq!(count, 2);
    }

    /// Technical English opinion words are scored correctly via the EN path.
    #[test]
    fn technical_words() {
        assert_eq!(score_word("elegant", "en"), Some(3));
        assert_eq!(score_word("hacky", "en"), Some(-2));
        assert_eq!(score_word("robust", "en"), Some(3));
        assert_eq!(score_word("bloated", "en"), Some(-2));
    }

    /// A clearly-French positive sentence scores positive via the FR lexicon
    /// when the language is supplied explicitly via `score_text_sum_for_lang`.
    /// This proves per-language routing works independently -- no cross-language
    /// homograph collision where an en/fr shared stem would resolve arbitrarily.
    #[test]
    fn score_text_homograph_uses_detected_language() {
        // Words from the FR sentiment_pos4/pos5 bucket (magnifique, formidable,
        // merveilleux, extraordinaire). Scoring these via "fr" must yield hits;
        // if the old union map were still in use, a French word scored under "en"
        // iteration order would produce arbitrary or zero results.
        let french_positive =
            "Ce projet est magnifique et formidable vraiment extraordinaire et merveilleux";
        let (sum, count) = score_text_sum_for_lang(french_positive, "fr");
        assert!(
            count > 0,
            "Expected at least one FR lexicon hit; got 0. Check fr.toml sentiment buckets."
        );
        assert!(
            sum > 0,
            "Expected positive sum for French positive sentence, got sum={sum} count={count}"
        );
    }
}
