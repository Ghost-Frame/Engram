use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Instant;

pub struct MemoryCandidate {
    pub content: String,
    pub category: String,
    pub tags: Vec<String>,
    pub importance: i32,
    pub source: String,
    pub session_id: String,
    #[expect(dead_code)]
    pub project: String,
}

pub struct Extractor {
    recent_hashes: HashMap<String, Instant>,
}

impl Extractor {
    pub fn new() -> Self {
        Self {
            recent_hashes: HashMap::new(),
        }
    }

    pub fn extract(
        &mut self,
        turn_text: &str,
        session_id: &str,
        project: &str,
    ) -> Option<MemoryCandidate> {
        let trimmed = turn_text.trim();

        // Skip short turns (quick acks)
        if trimmed.len() < 50 {
            return None;
        }

        // In-session dedup: skip if same 200-char prefix seen in last 10 min
        let prefix: String = trimmed.chars().take(200).collect();
        let now = Instant::now();
        self.recent_hashes
            .retain(|_, t| now.duration_since(*t).as_secs() < 600);
        if self.recent_hashes.contains_key(&prefix) {
            return None;
        }

        let lower = trimmed.to_lowercase();
        let mut tags = Vec::new();
        let category;
        let importance;

        // Classification rules. The keyword signals are sourced from the i18n
        // lexicon (kleos_lib::lexicon) so a French session is bucketed the same
        // way as an English one; only the language-agnostic infra/commit-hash
        // signals stay hardcoded. The if/else-if order is significant: a turn
        // is assigned to the first category it matches (deploy > decision >
        // preference > issue > procedure > infra).
        let mut matcher = LexiconMatcher::new(trimmed);
        if matcher.matches("ingest_deploy_markers") || has_commit_hash(trimmed) {
            category = "state";
            importance = 6;
            tags.push("deploy".to_string());
            infer_service_tags(&lower, &mut tags);
        } else if matcher.matches("ingest_decision_markers") {
            category = "decision";
            importance = 8;
            tags.push("decision".to_string());
        } else if matcher.matches("ingest_preference_markers") {
            category = "decision";
            importance = 7;
            tags.push("preference".to_string());
        } else if matcher.matches("ingest_issue_markers") {
            category = "issue";
            importance = 7;
            infer_service_tags(&lower, &mut tags);
        } else if matcher.matches("ingest_procedure_markers") {
            category = "reference";
            importance = 5;
        } else if has_infra_info(&lower) {
            category = "infrastructure";
            importance = 6;
            infer_host_tags(&lower, &mut tags);
        } else {
            // No strong signal -- skip
            return None;
        }

        self.recent_hashes.insert(prefix, now);

        Some(MemoryCandidate {
            content: if trimmed.len() > 2000 {
                kleos_lib::validation::truncate_on_char_boundary(trimmed, 2000).to_string()
            } else {
                trimmed.to_string()
            },
            category: category.to_string(),
            tags,
            importance,
            source: "kleos-ingest".to_string(),
            session_id: session_id.to_string(),
            project: project.to_string(),
        })
    }
}

/// Lexicon classes the extractor classifies against, in priority order.
const INGEST_CLASSES: &[&str] = &[
    "ingest_deploy_markers",
    "ingest_decision_markers",
    "ingest_preference_markers",
    "ingest_issue_markers",
    "ingest_procedure_markers",
];

/// A compiled membership matcher for one lexicon class in one language.
///
/// The regex is built from the FOLDED (lowercased, diacritic-stripped,
/// optionally stemmed) markers and is therefore only correct when run against
/// text folded the SAME way -- the lexicon contract is "fold both ends" so that
/// matching is invariant to case, accents and (when stemming applies)
/// inflection. The `\b ... \w*` shape keeps word-boundary precision (so `fix`
/// matches `fixed`/`fixes` but not `prefix`).
struct ClassRegex {
    lang: String,
    with_stem: bool,
    re: Regex,
}

fn build_class_regexes(class: &str) -> Vec<ClassRegex> {
    let mut out = Vec::new();
    for lang in kleos_lib::lexicon::supported_languages() {
        let markers = kleos_lib::lexicon::word_class(&lang, class);
        if markers.is_empty() {
            continue;
        }
        let with_stem = kleos_lib::lexicon::class_stem_enabled(&lang, class);
        let alternation = markers
            .iter()
            .map(|m| {
                // Fold (and optionally stem) each whitespace-separated token,
                // escape it, and re-glue multi-word markers with `\s+` so they
                // still match normalised source text.
                m.split_whitespace()
                    .map(|tok| {
                        let folded = kleos_lib::lexicon::fold_for_matching(tok, &lang, with_stem);
                        regex::escape(&folded)
                    })
                    .collect::<Vec<_>>()
                    .join(r"\s+")
            })
            .filter(|alt| !alt.is_empty())
            .collect::<Vec<_>>()
            .join("|");
        if alternation.is_empty() {
            continue;
        }
        if let Ok(re) = Regex::new(&format!(r"(?i)\b(?:{alternation})\w*")) {
            out.push(ClassRegex {
                lang,
                with_stem,
                re,
            });
        }
    }
    out
}

/// Built once: per ingest class, one regex per supported language.
static CLASS_REGEXES: LazyLock<HashMap<&'static str, Vec<ClassRegex>>> = LazyLock::new(|| {
    INGEST_CLASSES
        .iter()
        .map(|class| (*class, build_class_regexes(class)))
        .collect()
});

/// Folds the candidate text on demand, once per `(lang, with_stem)` pair, so a
/// turn is never folded more than twice (stemmed + unstemmed) per language even
/// when checked against several classes.
struct LexiconMatcher<'a> {
    raw: &'a str,
    folded: HashMap<(String, bool), String>,
}

impl<'a> LexiconMatcher<'a> {
    fn new(raw: &'a str) -> Self {
        Self {
            raw,
            folded: HashMap::new(),
        }
    }

    /// True when the text contains any marker of `class` in any supported
    /// language. Unknown classes and languages with no markers are skipped, so
    /// the call never panics on a missing class or empty language.
    fn matches(&mut self, class: &str) -> bool {
        let Some(regexes) = CLASS_REGEXES.get(class) else {
            return false;
        };
        for cr in regexes {
            let folded = self
                .folded
                .entry((cr.lang.clone(), cr.with_stem))
                .or_insert_with(|| {
                    kleos_lib::lexicon::fold_for_matching(self.raw, &cr.lang, cr.with_stem)
                });
            if cr.re.is_match(folded) {
                return true;
            }
        }
        false
    }
}

/// Substring membership test for the language-agnostic infra signals below.
fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

fn has_commit_hash(text: &str) -> bool {
    text.split_whitespace().any(|word| {
        word.len() >= 7 && word.len() <= 40 && word.chars().all(|c| c.is_ascii_hexdigit())
    })
}

fn has_infra_info(lower: &str) -> bool {
    contains_any(
        lower,
        &[
            "172.",
            "192.168.",
            "10.",
            "port ",
            ":4200",
            ":8080",
            ":443",
            "ssh ",
            "systemctl",
        ],
    )
}

fn infer_service_tags(lower: &str, tags: &mut Vec<String>) {
    let services = ["kleos", "engram", "credd", "eidolon", "kleos-ingest"];
    for svc in services {
        if lower.contains(svc) {
            tags.push(svc.to_string());
        }
    }
}

fn infer_host_tags(_lower: &str, _tags: &mut Vec<String>) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract_one(text: &str) -> Option<MemoryCandidate> {
        Extractor::new().extract(text, "sess-test", "proj-test")
    }

    #[test]
    fn english_decision_is_classified() {
        let c = extract_one(
            "After weighing the options we decided to go with the long-poll \
             approach for the supervisor pending endpoint instead of SSE.",
        )
        .expect("decision turn should classify");
        assert_eq!(c.category, "decision");
        assert!(c.tags.contains(&"decision".to_string()));
    }

    #[test]
    fn french_decision_is_classified_like_english() {
        // Same semantic content in French must land in the same category --
        // this is the whole point of sourcing markers from the i18n lexicon.
        let c = extract_one(
            "Apres avoir pese les options nous avons decide de partir sur le \
             long-poll pour l'endpoint supervisor au lieu du SSE.",
        )
        .expect("french decision turn should classify");
        assert_eq!(c.category, "decision");
    }

    #[test]
    fn french_issue_is_classified() {
        let c = extract_one(
            "J'ai corrige le bug de recursion dans la consolidation, l'erreur \
             venait du filtre is_consolidated absent au call site.",
        )
        .expect("french issue turn should classify");
        assert_eq!(c.category, "issue");
    }

    #[test]
    fn french_deploy_is_classified() {
        let c = extract_one(
            "J'ai deploye kleos-server sur la LXC 121 et redemarre le service, \
             le health check repond bien sur le port 4200.",
        )
        .expect("french deploy turn should classify");
        assert_eq!(c.category, "state");
        assert!(c.tags.contains(&"deploy".to_string()));
    }

    #[test]
    fn unrelated_chatter_is_skipped() {
        // Long enough to pass the length gate but with no strong signal.
        assert!(extract_one(
            "Thanks, that all looks good to me and I appreciate the detailed \
             walkthrough you gave on the topic earlier today honestly."
        )
        .is_none());
    }
}
