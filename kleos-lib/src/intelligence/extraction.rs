//! Fact extraction -- regex-based extraction of structured facts, preferences, and state.
//!
//! Ported from intelligence/extraction.ts. Pure regex, no LLM needed.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use crate::db::Database;
use crate::intelligence::types::ExtractionStats;
use crate::Result;
use regex::Regex;
use tracing::{debug, warn};

/// Build a per-language regex from a template by interpolating
/// `lexicon::word_class_alternation` for each placeholder.
///
/// Helper. Used by the like/dislike/favorite/location/role
/// patterns whose verbs vary across languages. The remaining patterns
/// (buy / spent / have / exercise / made / earned) keep their English
/// surface form because they encode unit-specific syntax (currency `$`,
/// quantity prefix, time units) that does not port symmetrically to
/// French and is left as future work.
fn compile_lang_regex(pattern: &str) -> Option<Regex> {
    Regex::new(pattern).ok()
}

fn like_regex_for(lang: &str) -> Option<Regex> {
    // Wildcard-after-stem: TOML lists infinitives
    // (`aimer`, `adorer`) but the source is raw user text with
    // conjugated forms. Stem the alternation and add `\w*` so the
    // root matches every inflection (`aime`, `aimait`, `aimerions`).
    // The capture group is preserved (cap[1] = verb, cap[2] = object).
    let verbs = crate::lexicon::word_class_alternation_stemmed(lang, "verb_like");
    if verbs.is_empty() {
        return None;
    }
    let pronouns = crate::lexicon::word_class_alternation_stemmed(lang, "first_person_pronoun");
    let pronoun_clause = if pronouns.is_empty() {
        String::new()
    } else {
        format!(r"(?:(?:{pronouns})\w*\s+)")
    };
    // Regex fix: wrap the alternation in `(?:...)` BEFORE
    // applying the `\w*` wildcard. Without the inner group, regex
    // priority makes `aim|ador|appreci|prefer|kiff\w*` parse as
    // `(aim) OR (ador) OR ... OR (kiff\w*)` and only the last
    // alternative gets the suffix. With `(?:...)\w*` the wildcard
    // applies to every alternative.
    let pattern = format!(r"(?i)\b{pronoun_clause}?((?:{verbs})\w*)\s+(.+?)(?:\.|,|$)");
    compile_lang_regex(&pattern)
}

fn dislike_regex_for(lang: &str) -> Option<Regex> {
    let verbs = crate::lexicon::word_class_alternation_stemmed(lang, "verb_dislike");
    if verbs.is_empty() {
        return None;
    }
    let pronouns = crate::lexicon::word_class_alternation_stemmed(lang, "first_person_pronoun");
    let pronoun_clause = if pronouns.is_empty() {
        String::new()
    } else {
        format!(r"(?:(?:{pronouns})\w*\s+)")
    };
    let pattern = format!(r"(?i)\b{pronoun_clause}?((?:{verbs})\w*)\s+(.+?)(?:\.|,|$)");
    compile_lang_regex(&pattern)
}

fn favorite_regex_for(lang: &str) -> Option<Regex> {
    let markers = crate::lexicon::word_class_alternation_stemmed(lang, "favorite_marker");
    let categories = crate::lexicon::word_class_alternation_stemmed(lang, "favorite_category");
    let copula = crate::lexicon::word_class_alternation_stemmed(lang, "is_or_are");
    if markers.is_empty() || categories.is_empty() || copula.is_empty() {
        return None;
    }
    // English form: "my favorite food is X" (marker before category).
    // French form: "mon plat préféré est X" (marker after category).
    // The template accepts either order so the same regex covers both
    // languages. Marker groups stay non-capturing so the caller still
    // reads cap[1] = category, cap[2] = value (signature preserved).
    // Regex fix: each alternation is wrapped in `(?:...)` so
    // the `\w*` wildcard applies to every alternative (regex priority
    // would otherwise attach the wildcard only to the last word).
    let pattern = format!(
        r"(?i)\b(?:my|mon|ma)\s+(?:(?:{markers})\w*\s+)?((?:{categories})\w*)\s+(?:(?:{markers})\w*\s+)?(?:(?:{copula})\w*)\s+(.+?)(?:\.|,|$)"
    );
    compile_lang_regex(&pattern)
}

fn location_regex_for(lang: &str) -> Option<Regex> {
    let verbs = crate::lexicon::word_class_alternation_stemmed(lang, "location_verbs");
    if verbs.is_empty() {
        return None;
    }
    let pronouns = crate::lexicon::word_class_alternation_stemmed(lang, "first_person_pronoun");
    let pronoun_clause = if pronouns.is_empty() {
        String::new()
    } else {
        format!(r"(?:{pronouns})\w*\s+")
    };
    let pattern = format!(r"(?i)\b(?:{pronoun_clause})?(?:{verbs})\w*\s+(.+?)(?:\.|,|$)");
    compile_lang_regex(&pattern)
}

fn role_regex_for(lang: &str) -> Option<Regex> {
    let verbs = crate::lexicon::word_class_alternation_stemmed(lang, "role_verbs");
    if verbs.is_empty() {
        return None;
    }
    let pronouns = crate::lexicon::word_class_alternation_stemmed(lang, "first_person_pronoun");
    let pronoun_clause = if pronouns.is_empty() {
        String::new()
    } else {
        format!(r"(?:{pronouns})\w*\s+")
    };
    let pattern = format!(
        r"(?i)\b(?:{pronoun_clause})?(?:{verbs})\w*\s+(?:a\s+|an\s+|my\s+|un\s+|une\s+)?(.+?)(?:\.|,|$)"
    );
    compile_lang_regex(&pattern)
}

// Helpers for the 6 previously English-only patterns
// (buy / spent / have / exercise / made / earned). Each helper composes a
// regex fragment from per-language lexicon classes so French (or any other
// language with the appropriate classes) gets first-class support.
//
// Convention:
//   - The verb class is *_stemmed so conjugated forms match (`achete`,
//     `achetait`, `acheterais`...) via the trailing `\w*` wildcard.
//   - Currency: `currency_symbols_prefix` (EN: `$`, `USD`) vs
//     `currency_symbols_suffix` (FR: `euros`, `EUR`, `€`). A language
//     populates only one of the two; the helper picks the non-empty side.
//   - Prepositions (`on/for` EN, `pour/en/sur` FR) live in their own classes
//     so callers do not need to hardcode them per language.

/// Compose a regex fragment matching `<currency>50` (prefix
/// convention) or `50 <currency>` (suffix convention) with a single
/// capturing group for the numeric amount. Returns `None` when neither
/// currency class is populated for `lang`.
fn currency_amount_fragment(lang: &str) -> Option<String> {
    // Use the stemmed alternation so symbols like `$` are regex::escape-d.
    // `currency_symbols_*` classes declare `stem = false`, so the stemmer
    // is a no-op; only the lowercase + diacritic fold + escape happens.
    let pre = crate::lexicon::word_class_alternation_stemmed(lang, "currency_symbols_prefix");
    let suf = crate::lexicon::word_class_alternation_stemmed(lang, "currency_symbols_suffix");
    if !pre.is_empty() {
        Some(format!(r"(?:{pre})\s*([\d,.]+)"))
    } else if !suf.is_empty() {
        Some(format!(r"([\d,.]+)\s*(?:{suf})"))
    } else {
        None
    }
}

fn pronoun_clause_for(lang: &str) -> String {
    let pronouns = crate::lexicon::word_class_alternation_stemmed(lang, "first_person_pronoun");
    if pronouns.is_empty() {
        String::new()
    } else {
        format!(r"(?:(?:{pronouns})\w*\s+)?")
    }
}

fn buy_regex_for(lang: &str) -> Option<Regex> {
    let verbs = crate::lexicon::word_class_alternation_stemmed(lang, "extract_buy_verbs");
    if verbs.is_empty() {
        return None;
    }
    let pronoun_clause = pronoun_clause_for(lang);
    // Cap[1] = verb, cap[2] = quantity, cap[3] = object
    let pattern = format!(r"(?i)\b{pronoun_clause}((?:{verbs})\w*)\s+(\d+)\s+(.+?)(?:\.|,|$)");
    compile_lang_regex(&pattern)
}

fn spent_regex_for(lang: &str) -> Option<Regex> {
    let verbs = crate::lexicon::word_class_alternation_stemmed(lang, "extract_spent_verbs");
    if verbs.is_empty() {
        return None;
    }
    let amount = currency_amount_fragment(lang)?;
    let preps = crate::lexicon::word_class_alternation_stemmed(lang, "extract_spent_preposition");
    if preps.is_empty() {
        return None;
    }
    let pronoun_clause = pronoun_clause_for(lang);
    // Cap[1] = amount, cap[2] = object
    let pattern =
        format!(r"(?i)\b{pronoun_clause}(?:{verbs})\w*\s+{amount}\s+(?:{preps})\s+(.+?)(?:\.|,|$)");
    compile_lang_regex(&pattern)
}

fn have_regex_for(lang: &str) -> Option<Regex> {
    let verbs = crate::lexicon::word_class_alternation_stemmed(lang, "extract_have_verbs");
    if verbs.is_empty() {
        return None;
    }
    let pronoun_clause = pronoun_clause_for(lang);
    // Cap[1] = quantity, cap[2] = object
    // Tail clause matches both EN (and/but/so/now) and FR (et/mais/alors/donc) connectors.
    let pattern = format!(
        r"(?i)\b{pronoun_clause}(?:{verbs})\w*\s+(\d+)\s+(.+?)(?:\.|,|\s+(?:and|but|so|now|et|mais|alors|donc))"
    );
    compile_lang_regex(&pattern)
}

fn exercise_regex_for(lang: &str) -> Option<Regex> {
    let verbs = crate::lexicon::word_class_alternation_stemmed(lang, "extract_exercise_verbs");
    if verbs.is_empty() {
        return None;
    }
    let time = crate::lexicon::word_class_alternation_stemmed(lang, "time_units");
    let dist = crate::lexicon::word_class_alternation_stemmed(lang, "distance_units");
    let unit_alt = match (time.is_empty(), dist.is_empty()) {
        (true, true) => return None,
        (false, true) => time,
        (true, false) => dist,
        (false, false) => format!("{time}|{dist}"),
    };
    let pronoun_clause = pronoun_clause_for(lang);
    // Cap[1] = verb, cap[2] = quantity, cap[3] = unit
    // Optional duration filler covers EN ("for") and FR ("pour"/"pendant").
    let pattern = format!(
        r"(?i)\b{pronoun_clause}((?:{verbs})\w*)\s+(?:for\s+|pour\s+|pendant\s+)?(\d+(?:\.\d+)?)\s+({unit_alt})"
    );
    compile_lang_regex(&pattern)
}

fn made_regex_for(lang: &str) -> Option<Regex> {
    let verbs = crate::lexicon::word_class_alternation_stemmed(lang, "extract_made_verbs");
    if verbs.is_empty() {
        return None;
    }
    let pronoun_clause = pronoun_clause_for(lang);
    // Cap[1] = verb, cap[2] = object
    // Article filler covers EN (a/some) and FR (un/une/des/du).
    let pattern = format!(
        r"(?i)\b{pronoun_clause}((?:{verbs})\w*)\s+(?:a\s+|some\s+|un\s+|une\s+|des\s+|du\s+|de\s+la\s+)?(.+?)(?:\.|,|\s+(?:and|but|for|from|yesterday|today|last|et|mais|pour|hier|aujourd'hui|dernier|derniere))"
    );
    compile_lang_regex(&pattern)
}

/// Human-readable currency label per language for the
/// `[unit:...]` slot of formatted facts. Defaults to "dollars" so any
/// language without a dedicated mapping behaves like EN historically did.
fn currency_label(lang: &str) -> &'static str {
    match lang {
        "fr" => "euros",
        _ => "dollars",
    }
}

fn earned_regex_for(lang: &str) -> Option<Regex> {
    let verbs = crate::lexicon::word_class_alternation_stemmed(lang, "extract_earned_verbs");
    if verbs.is_empty() {
        return None;
    }
    let amount = currency_amount_fragment(lang)?;
    let preps = crate::lexicon::word_class_alternation_stemmed(lang, "extract_earned_preposition");
    let pronoun_clause = pronoun_clause_for(lang);
    let prep_clause = if preps.is_empty() {
        String::new()
    } else {
        format!(r"(?:\s+(?:{preps})\s+(.+?))?")
    };
    // Cap[1] = verb, cap[2] = amount, cap[3] = object (optional)
    let pattern =
        format!(r"(?i)\b{pronoun_clause}((?:{verbs})\w*)\s+{amount}{prep_clause}(?:\.|,|$)");
    compile_lang_regex(&pattern)
}

/// Cache of compiled per-language regexes for the 5 i18n-portable patterns,
/// plus the cross-language copula set used by the  collision skip.
/// Compiled once on first access from the current state of the lexicon.
struct LangRegexCache {
    like: HashMap<String, Regex>,
    dislike: HashMap<String, Regex>,
    favorite: HashMap<String, Regex>,
    location: HashMap<String, Regex>,
    role: HashMap<String, Regex>,
    // 6 unit-specific patterns migrated to per-language.
    buy: HashMap<String, Regex>,
    spent: HashMap<String, Regex>,
    have: HashMap<String, Regex>,
    exercise: HashMap<String, Regex>,
    made: HashMap<String, Regex>,
    earned: HashMap<String, Regex>,
    /// Union of copula tokens (`is_or_are`
    /// class) folded across ALL supported languages, with stem=false
    /// projection so the runtime check can fold its candidate token the
    /// same way regardless of which lang's pattern produced the match.
    ///
    /// Rationale: a `verb_like` stem (`prefer` from EN `prefer` or FR
    /// `preferer`) often matches across languages thanks to `\w*`. The
    /// match's lang is therefore not a reliable hint for which copula
    /// vocabulary to consult. The union set absorbs this: if the first
    /// token of the captured object is a copula in ANY supported lang,
    /// the match is suspect regardless of which lang's regex produced it.
    ///
    /// False-positive risk is minimal: copules are short, distinctive
    /// grammar words (`is`/`est`/`ist`/`es`...) and rarely appear as
    /// leading tokens of legitimate LIKE/DISLIKE objects.
    all_copulas: HashSet<String>,
}

static LANG_REGEX: LazyLock<LangRegexCache> = LazyLock::new(|| {
    let mut like = HashMap::new();
    let mut dislike = HashMap::new();
    let mut favorite = HashMap::new();
    let mut location = HashMap::new();
    let mut role = HashMap::new();
    let mut buy = HashMap::new();
    let mut spent = HashMap::new();
    let mut have = HashMap::new();
    let mut exercise = HashMap::new();
    let mut made = HashMap::new();
    let mut earned = HashMap::new();
    let mut all_copulas: HashSet<String> = HashSet::new();
    for lang in crate::lexicon::supported_languages() {
        if let Some(re) = like_regex_for(&lang) {
            like.insert(lang.clone(), re);
        }
        if let Some(re) = dislike_regex_for(&lang) {
            dislike.insert(lang.clone(), re);
        }
        if let Some(re) = favorite_regex_for(&lang) {
            favorite.insert(lang.clone(), re);
        }
        if let Some(re) = location_regex_for(&lang) {
            location.insert(lang.clone(), re);
        }
        if let Some(re) = role_regex_for(&lang) {
            role.insert(lang.clone(), re);
        }
        if let Some(re) = buy_regex_for(&lang) {
            buy.insert(lang.clone(), re);
        }
        if let Some(re) = spent_regex_for(&lang) {
            spent.insert(lang.clone(), re);
        }
        if let Some(re) = have_regex_for(&lang) {
            have.insert(lang.clone(), re);
        }
        if let Some(re) = exercise_regex_for(&lang) {
            exercise.insert(lang.clone(), re);
        }
        if let Some(re) = made_regex_for(&lang) {
            made.insert(lang.clone(), re);
        }
        if let Some(re) = earned_regex_for(&lang) {
            earned.insert(lang.clone(), re);
        }
        // Merge every lang's copula set into the global
        // union. Fold each word with the class's stem policy of THIS
        // lang (typically stem=false for the grammar class), so the
        // stored form matches what the runtime check produces.
        let copula_words = crate::lexicon::word_class(&lang, "is_or_are");
        if !copula_words.is_empty() {
            let with_stem = crate::lexicon::class_stem_enabled(&lang, "is_or_are");
            for w in copula_words {
                all_copulas.insert(crate::lexicon::fold_for_matching(&w, &lang, with_stem));
            }
        }
    }
    LangRegexCache {
        like,
        dislike,
        favorite,
        location,
        role,
        buy,
        spent,
        have,
        exercise,
        made,
        earned,
        all_copulas,
    }
});

/// Returns true when a LIKE / DISLIKE capture is suspect because its
/// object starts with a copula word. Folds the first token under the
/// detected language's rules and checks membership in the global copula
/// union (built at init from all supported languages). A match means the
/// token is a copula in at least one supported language, regardless of
/// which language's regex produced the capture.
///
/// Languages without an `is_or_are` class contribute nothing to the
/// union, so they cannot trigger a false-positive skip.
fn object_starts_with_copula(object: &str, lang: &str) -> bool {
    let Some(first_token) = object.split_whitespace().next() else {
        return false;
    };
    // Fold under the detected language's stem policy; the all_copulas
    // union was built from every lang's folds at init, so a single-lang
    // fold is sufficient to identify copula surface forms.
    let with_stem = crate::lexicon::class_stem_enabled(lang, "is_or_are");
    let folded = crate::lexicon::fold_for_matching(first_token, lang, with_stem);
    LANG_REGEX.all_copulas.contains(&folded)
}

// All 11 prior English-only static regexes
// (like, dislike, favorite, location, role, buy, spent, have, exercise,
// made, earned) are now superseded by their per-language `_regex_for`
// helpers and the LANG_REGEX cache above. The currency / time-unit /
// distance-unit syntax that was previously deemed "non-symmetric" is
// captured via dedicated lexicon classes (`currency_symbols_prefix` for
// the EN `$N` convention, `currency_symbols_suffix` for the FR `N euros`
// convention, plus `time_units` and `distance_units` per language).

// Collected operations to execute in a single transaction
struct FactInsert {
    subject: String,
    verb: String,
    object: String,
}

struct PrefUpsert {
    key: String,
    value: String,
}

struct StateUpsert {
    key: String,
    value: String,
}

/// Extract structured facts, preferences, and state updates from memory content.
#[tracing::instrument(skip(db, content), fields(content_len = content.len()))]
pub async fn fast_extract_facts(
    db: &Database,
    content: &str,
    memory_id: i64,
    user_id: i64,
    episode_id: Option<i64>,
) -> Result<ExtractionStats> {
    // Collect all operations first
    let mut facts: Vec<FactInsert> = Vec::new();
    let mut prefs: Vec<PrefUpsert> = Vec::new();
    let mut states: Vec<StateUpsert> = Vec::new();

    // Extract date context from content
    let _date_approx = extract_date_approx(content);
    let date_ref = extract_date_ref(content);
    // Detect the content language once; all pattern lookups use this
    // single entry point to avoid cross-language duplicate matches.
    let lang = crate::lang::detect_lang(content);

    // -- Patterns 1-6 (buy / spent / have / exercise / made / earned) --
    // Pre-fold the source using the detected language so accented characters
    // (`dépensé` -> `depense`) are normalised to match the lexicon-built
    // regex tokens. Object captures lose accents but the downstream
    // `structured_facts` rows only care about subject/predicate/object
    // identity, which is preserved.
    let folded_content = crate::lexicon::fold_for_matching(content, &lang, false);
    let folded: &str = &folded_content;
    // Single currency label for the detected language.
    let currency = currency_label(&lang);

    // -- Pattern 1: bought/purchased N items --
    if let Some(re) = LANG_REGEX.buy.get(&lang) {
        for cap in re.captures_iter(folded) {
            let verb = cap[1].to_lowercase();
            let quantity: i64 = cap[2].parse().unwrap_or(0);
            let object = cap[3].trim();
            if object.len() > 200 {
                continue;
            }
            facts.push(FactInsert {
                subject: "user".to_string(),
                verb,
                object: format_fact_object(object, Some(quantity), None, date_ref.as_deref()),
            });
        }
    }

    // -- Pattern 2: spent <currency> N <prep> X --
    if let Some(re) = LANG_REGEX.spent.get(&lang) {
        for cap in re.captures_iter(folded) {
            let amount: f64 = cap[1].replace(',', "").parse().unwrap_or(0.0);
            let object = cap[2].trim();
            facts.push(FactInsert {
                subject: "user".to_string(),
                verb: "spent".to_string(),
                object: format_fact_object(
                    object,
                    Some(amount as i64),
                    Some(currency),
                    date_ref.as_deref(),
                ),
            });
        }
    }

    // -- Pattern 3: have/own N X --
    if let Some(re) = LANG_REGEX.have.get(&lang) {
        for cap in re.captures_iter(folded) {
            let quantity: i64 = cap[1].parse().unwrap_or(0);
            let object = cap[2].trim();
            facts.push(FactInsert {
                subject: "user".to_string(),
                verb: "has".to_string(),
                object: format_fact_object(object, Some(quantity), None, date_ref.as_deref()),
            });
        }
    }

    // -- Pattern 4: exercised for N <time/distance unit> --
    if let Some(re) = LANG_REGEX.exercise.get(&lang) {
        for cap in re.captures_iter(folded) {
            let verb = cap[1].to_lowercase();
            let quantity: f64 = cap[2].parse().unwrap_or(0.0);
            let unit = cap[3].to_lowercase();
            facts.push(FactInsert {
                subject: "user".to_string(),
                verb,
                object: format_fact_object(
                    "",
                    Some(quantity as i64),
                    Some(&unit),
                    date_ref.as_deref(),
                ),
            });
        }
    }

    // -- Pattern 5: made/baked/cooked X --
    if let Some(re) = LANG_REGEX.made.get(&lang) {
        for cap in re.captures_iter(folded) {
            let verb = cap[1].to_lowercase();
            let object = cap[2].trim();
            facts.push(FactInsert {
                subject: "user".to_string(),
                verb,
                object: format_fact_object(object, Some(1), None, date_ref.as_deref()),
            });
        }
    }

    // -- Pattern 6: earned/made N <currency> (from X) --
    if let Some(re) = LANG_REGEX.earned.get(&lang) {
        for cap in re.captures_iter(folded) {
            let amount: f64 = cap[2].replace(',', "").parse().unwrap_or(0.0);
            let object = cap.get(3).map(|m| m.as_str().trim()).unwrap_or("");
            facts.push(FactInsert {
                subject: "user".to_string(),
                verb: "earned".to_string(),
                object: format_fact_object(
                    object,
                    Some(amount as i64),
                    Some(currency),
                    date_ref.as_deref(),
                ),
            });
        }
    }

    // -- Preferences: likes/enjoys --
    // The 5 i18n-portable patterns (like, dislike, favorite, location,
    // role) use single-lang LANG_REGEX lookups keyed on the detected
    // language. No dedup sets needed -- each regex's captures_iter is
    // non-overlapping within a single language.

    // -- Preferences: likes --
    if let Some(re) = LANG_REGEX.like.get(&lang) {
        for cap in re.captures_iter(content) {
            let object = cap[2].trim();
            // Skip captures whose object starts with a copula -- strong
            // signal the source is a FAVORITE structure ("mon plat
            // prefere est X") mis-classified as LIKE by stem overlap.
            if object_starts_with_copula(object, &lang) {
                continue;
            }
            if object.len() > 3 && object.len() < 100 {
                let domain = infer_domain(object, &lang);
                let key = format!("{domain}:likes {object}");
                prefs.push(PrefUpsert {
                    key,
                    value: format!("evidence_memory_id:{memory_id}"),
                });
            }
        }
    }

    // -- Preferences: dislikes --
    if let Some(re) = LANG_REGEX.dislike.get(&lang) {
        for cap in re.captures_iter(content) {
            let object = cap[2].trim();
            // Same copula collision skip as LIKE.
            if object_starts_with_copula(object, &lang) {
                continue;
            }
            if object.len() > 3 && object.len() < 100 {
                let domain = infer_domain(object, &lang);
                let key = format!("{domain}:dislikes {object}");
                prefs.push(PrefUpsert {
                    key,
                    value: format!("evidence_memory_id:{memory_id}"),
                });
            }
        }
    }

    // -- Preferences: favorites --
    if let Some(re) = LANG_REGEX.favorite.get(&lang) {
        for cap in re.captures_iter(content) {
            let category = cap[1].trim().to_lowercase();
            let value = cap[2].trim();
            let key = format!("{category}:favorite: {value}");
            prefs.push(PrefUpsert {
                key,
                value: format!("evidence_memory_id:{memory_id}"),
            });
        }
    }

    // -- State updates: location changes --
    if let Some(re) = LANG_REGEX.location.get(&lang) {
        for cap in re.captures_iter(content) {
            let location = cap[1].trim();
            states.push(StateUpsert {
                key: "current_location".to_string(),
                value: format!("{location} (memory:{memory_id})"),
            });
        }
    }

    // -- State updates: role changes --
    if let Some(re) = LANG_REGEX.role.get(&lang) {
        for cap in re.captures_iter(content) {
            let role = cap[1].trim();
            if role.len() > 3 && role.len() < 100 {
                states.push(StateUpsert {
                    key: "current_role".to_string(),
                    value: format!("{role} (memory:{memory_id})"),
                });
            }
        }
    }

    // Execute all operations in a single write
    let fact_count = facts.len();
    let pref_count = prefs.len();
    let state_count = states.len();

    if fact_count + pref_count + state_count > 0 {
        db.write(move |conn| {
            // Insert facts scoped to the owning user so single-DB mode
            // isolates facts per user (user_id column added by monolith v76).
            for fact in &facts {
                if let Err(e) = conn.execute(
                    "INSERT INTO structured_facts \
                     (memory_id, subject, predicate, object, confidence, user_id) \
                     VALUES (?1, ?2, ?3, ?4, 1.0, ?5)",
                    rusqlite::params![memory_id, fact.subject, fact.verb, fact.object, user_id],
                ) {
                    warn!(error = %e, "fact_insert_failed");
                }
            }

            // Upsert preferences scoped to the owning user so single-DB mode
            // isolates preferences per user (user_id column restored by v77).
            for pref in &prefs {
                if let Err(e) = conn.execute(
                    "INSERT INTO user_preferences (user_id, key, value, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, datetime('now'), datetime('now')) \
                     ON CONFLICT(user_id, key) DO UPDATE SET \
                       value = excluded.value, \
                       updated_at = datetime('now')",
                    rusqlite::params![user_id, pref.key, pref.value],
                ) {
                    warn!(error = %e, "preference_upsert_failed");
                }
            }

            // Upsert state scoped to the owning user so single-DB mode isolates
            // state entries per user. UNIQUE constraint is now (agent, key, user_id).
            for state in &states {
                if let Err(e) = conn.execute(
                    "INSERT INTO current_state (agent, key, value, user_id, created_at, updated_at) \
                     VALUES ('system', ?1, ?2, ?3, datetime('now'), datetime('now')) \
                     ON CONFLICT(agent, key, user_id) DO UPDATE SET \
                       value = excluded.value, \
                       updated_at = datetime('now')",
                    rusqlite::params![state.key, state.value, user_id],
                ) {
                    warn!(error = %e, "state_upsert_failed");
                }
            }

            // Stamp episode provenance on newly extracted facts
            if !facts.is_empty() {
                if let Some(ep_id) = episode_id {
                    if let Err(e) = conn.execute(
                        "UPDATE structured_facts SET episode_id = ?1
                         WHERE memory_id = ?2 AND episode_id IS NULL",
                        rusqlite::params![ep_id, memory_id],
                    ) {
                        warn!(memory_id, user_id, error = %e, "extraction: failed to stamp episode_id on structured_facts");
                    }
                }
            }

            Ok(())
        })
        .await?;
    }

    let stats = ExtractionStats {
        facts: fact_count as i32,
        preferences: pref_count as i32,
        state_updates: state_count as i32,
    };

    if stats.facts + stats.preferences + stats.state_updates > 0 {
        debug!(
            memory_id,
            facts = stats.facts,
            prefs = stats.preferences,
            state = stats.state_updates,
            "fast_extract"
        );
    }

    Ok(stats)
}

fn format_fact_object(
    object: &str,
    quantity: Option<i64>,
    unit: Option<&str>,
    date_ref: Option<&str>,
) -> String {
    format!(
        "{}{}{}{}",
        object,
        quantity
            .map(|q| format!(" [qty:{}]", q))
            .unwrap_or_default(),
        unit.map(|u| format!(" [unit:{}]", u)).unwrap_or_default(),
        date_ref
            .map(|d| format!(" [date:{}]", d))
            .unwrap_or_default(),
    )
}

fn extract_date_approx(content: &str) -> Option<String> {
    let re = Regex::new(r"\[Conversation date:\s*([\d/]+)\]").ok()?;
    re.captures(content).map(|c| c[1].to_string())
}

fn extract_date_ref(content: &str) -> Option<String> {
    // Look for explicit date references like "yesterday", "last week", specific dates
    let re = Regex::new(r"(?i)\b(yesterday|today|last\s+(?:week|month|year)|(?:on\s+)?(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[a-z]*\s+\d{1,2}(?:st|nd|rd|th)?(?:\s*,?\s*\d{4})?)\b").ok()?;
    re.find(content).map(|m| m.as_str().to_string())
}

/// Infer the high-level domain of an object string using the detected
/// language first, with a single "en" fallback when the detected language
/// yields no domain match for a given class.
///
/// Compares folded versions of both sides so an object like
/// "petit-déjeuner" matches the lexicon entry regardless of accent or
/// hyphen loss. DOMAINS preserves the original priority
/// (food > entertainment > reading > music > gaming > fitness > travel)
/// so overlapping matches resolve deterministically.
fn infer_domain(object: &str, lang: &str) -> String {
    const DOMAINS: &[(&str, &str)] = &[
        ("domain_food", "food"),
        ("domain_entertainment", "entertainment"),
        ("domain_reading", "reading"),
        ("domain_music", "music"),
        ("domain_gaming", "gaming"),
        ("domain_fitness", "fitness"),
        ("domain_travel", "travel"),
    ];
    for (class, label) in DOMAINS {
        // Check detected language first.
        let folded_obj = crate::lexicon::fold_for_matching(object, lang, true);
        let words = crate::lexicon::word_class(lang, class);
        if words
            .iter()
            .any(|w| folded_obj.contains(&crate::lexicon::fold_word_for_class(w, lang, class)))
        {
            return (*label).to_string();
        }
        // Fall back to "en" when the detected language has no vocabulary
        // for this domain class (e.g. a French user saying "pizza" still
        // needs the EN domain_food entry to classify correctly).
        if lang != "en" {
            let folded_en = crate::lexicon::fold_for_matching(object, "en", true);
            let en_words = crate::lexicon::word_class("en", class);
            if en_words
                .iter()
                .any(|w| folded_en.contains(&crate::lexicon::fold_word_for_class(w, "en", class)))
            {
                return (*label).to_string();
            }
        }
    }
    "general".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: pre-fold a raw transcript the same way fast_extract_facts does
    // at runtime (lowercase + strip diacritics, no stem). Compiled regexes
    // are built from folded lexicon tokens, so the haystack must be folded
    // the same way for the literal stems to match.
    fn fold(raw: &str) -> String {
        crate::lexicon::fold_for_matching(raw, "en", false)
    }

    #[test]
    fn test_buy_regex_captures() {
        let content = fold("I bought 3 apples yesterday.");
        let re = buy_regex_for("en").expect("EN buy regex must compile");
        let caps: Vec<_> = re.captures_iter(&content).collect();
        assert_eq!(caps.len(), 1);
        assert!(caps[0][1].starts_with("bought"));
        assert_eq!(&caps[0][2], "3");
        assert_eq!(&caps[0][3], "apples yesterday");
    }

    #[test]
    fn test_spent_regex_captures() {
        let content = fold("I spent $50.00 on groceries.");
        let re = spent_regex_for("en").expect("EN spent regex must compile");
        let caps: Vec<_> = re.captures_iter(&content).collect();
        assert_eq!(caps.len(), 1);
        assert_eq!(&caps[0][1], "50.00");
        assert_eq!(&caps[0][2], "groceries");
    }

    // FR coverage tests. These confirm the lexicon-driven
    // regex generates a valid Regex per language and that the captures
    // semantics align with the EN equivalent (same group indices).

    #[test]
    fn test_buy_regex_captures_fr() {
        let content = fold("J'ai acheté 3 pommes hier.");
        let re = buy_regex_for("fr").expect("FR buy regex must compile");
        let caps: Vec<_> = re.captures_iter(&content).collect();
        assert!(!caps.is_empty(), "FR buy_regex_for should match");
        assert!(caps[0][1].contains("achet"));
        assert_eq!(&caps[0][2], "3");
        assert!(caps[0][3].contains("pommes"));
    }

    #[test]
    fn test_spent_regex_captures_fr() {
        let content = fold("J'ai dépensé 50 euros pour les courses.");
        let re = spent_regex_for("fr").expect("FR spent regex must compile");
        let caps: Vec<_> = re.captures_iter(&content).collect();
        assert!(!caps.is_empty(), "FR spent_regex_for should match");
        assert_eq!(&caps[0][1], "50");
        assert!(caps[0][2].contains("courses"));
    }

    #[test]
    fn test_exercise_regex_captures_fr() {
        let content = fold("J'ai couru 5 km hier.");
        let re = exercise_regex_for("fr").expect("FR exercise regex must compile");
        let caps: Vec<_> = re.captures_iter(&content).collect();
        assert!(!caps.is_empty(), "FR exercise_regex_for should match");
        assert!(caps[0][1].contains("couru"));
        assert_eq!(&caps[0][2], "5");
        assert_eq!(&caps[0][3], "km");
    }

    #[test]
    fn test_earned_regex_captures_fr() {
        let content = fold("J'ai gagné 100 euros pour le travail.");
        let re = earned_regex_for("fr").expect("FR earned regex must compile");
        let caps: Vec<_> = re.captures_iter(&content).collect();
        assert!(!caps.is_empty(), "FR earned_regex_for should match");
        // Cap[1] = verb, cap[2] = amount, cap[3] = object (optional)
        assert!(caps[0][1].contains("gagn"));
        assert_eq!(&caps[0][2], "100");
    }

    #[test]
    fn test_have_regex_captures_fr() {
        let content = fold("Je possède 4 livres et 2 stylos.");
        let re = have_regex_for("fr").expect("FR have regex must compile");
        let caps: Vec<_> = re.captures_iter(&content).collect();
        assert!(!caps.is_empty(), "FR have_regex_for should match");
        assert_eq!(&caps[0][1], "4");
        assert!(caps[0][2].contains("livres"));
    }

    #[test]
    fn test_made_regex_captures_fr() {
        let content = fold("J'ai préparé une tarte hier.");
        let re = made_regex_for("fr").expect("FR made regex must compile");
        let caps: Vec<_> = re.captures_iter(&content).collect();
        assert!(!caps.is_empty(), "FR made_regex_for should match");
        assert!(caps[0][1].contains("prepar"));
        assert!(caps[0][2].contains("tarte"));
    }

    #[test]
    fn test_currency_label() {
        assert_eq!(currency_label("en"), "dollars");
        assert_eq!(currency_label("fr"), "euros");
        assert_eq!(currency_label("xx"), "dollars"); // fallback
    }

    #[test]
    fn test_infer_domain() {
        assert_eq!(infer_domain("pizza", "en"), "food");
        assert_eq!(infer_domain("movie night", "en"), "entertainment");
        assert_eq!(infer_domain("running shoes", "en"), "fitness");
        assert_eq!(infer_domain("random thing", "en"), "general");
    }

    /// Verify that the FR like-regex (when present in the lexicon) fires on a
    /// canonical French "j'aime ..." sentence, and that the EN path is
    /// unaffected. Tests the per-lang LANG_REGEX lookup directly rather than
    /// going through detect_lang, because whatlang may misdetect short text --
    /// the single-lang lookup path itself is what changed in fast_extract_facts.
    #[test]
    fn fast_extract_french_preference() {
        let fr_sentence = "j'aime la musique.";
        // Access the FR like-regex directly. If FR verb_like is absent from
        // the lexicon the if-let passes silently (no panic) -- graceful no-op.
        if let Some(re) = LANG_REGEX.like.get("fr") {
            // The apostrophe in "j'" is a non-word char, so \b fires before
            // "aime" and the verb stem "aim\w*" matches even without the
            // optional pronoun prefix.
            assert!(
                re.captures_iter(fr_sentence).count() > 0,
                "FR like-regex should match \"j'aime la musique\""
            );
        }
        // English path: detect_lang("I like pizza") -> "en"; EN regex fires.
        let en_sentence = "I like pizza.";
        let en_lang = crate::lang::detect_lang(en_sentence);
        let en_matched = LANG_REGEX
            .like
            .get(&en_lang)
            .map(|re| re.captures_iter(en_sentence).count() > 0)
            .unwrap_or(false);
        assert!(en_matched, "EN like-regex should match \"I like pizza\"");
    }

    #[test]
    fn test_format_fact_object() {
        assert_eq!(
            format_fact_object("apples", Some(3), None, None),
            "apples [qty:3]"
        );
        assert_eq!(
            format_fact_object("", Some(5), Some("miles"), Some("yesterday")),
            " [qty:5] [unit:miles] [date:yesterday]"
        );
    }
}
