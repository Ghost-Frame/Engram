use super::types::FtsHit;
use crate::db::Database;
use crate::EngError;
use crate::Result;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use tracing::warn;

/// Sanitize a query string for FTS5 (remove special chars that break FTS syntax).
pub fn sanitize_fts_query(query: &str) -> String {
    // Remove FTS5 operators and special chars, keep alphanumeric and spaces
    let sanitized: String = query
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() {
                c
            } else {
                ' '
            }
        })
        .collect();
    // Split into tokens, filter short ones, join with spaces (implicit AND)
    sanitized
        .split_whitespace()
        .filter(|w| w.len() >= 2)
        .collect::<Vec<_>>()
        .join(" ")
}

/// High-frequency English function words dropped from the OR-fusion MATCH expression.
///
/// FTS5 here uses the default unicode61 tokenizer with no stemming and no stopword list, so
/// OR-ing a word like "the" or "for" matches a large fraction of the corpus and floods BM25
/// with near-universal hits, drowning the content tokens that actually carry query intent.
/// Removing these keeps the disjunction focused on meaningful terms. Tokens shorter than 2
/// chars are already filtered, so single-letter stopwords are omitted here.
const FTS_STOPWORDS: &[&str] = &[
    "the", "and", "for", "are", "was", "were", "with", "that", "this", "from", "have", "has",
    "had", "you", "your", "but", "not", "all", "any", "can", "our", "out", "his", "her", "she",
    "him", "they", "them", "their", "what", "when", "where", "which", "who", "how", "why", "into",
    "than", "then", "there", "here", "been", "being", "would", "could", "should", "about", "over",
    "some", "such", "only", "also", "more", "most", "other", "is", "to", "of", "in", "on", "at",
    "by", "be", "as", "it", "or", "an", "we", "if", "do", "so", "no", "up", "my", "me", "us",
];

/// Maximum number of OR terms in a memory-search MATCH expression. Caps the size of the FTS5
/// query so a pathological many-token input cannot expand into an unbounded disjunction even
/// after stopword removal; natural-language queries rarely carry this many content tokens.
const MAX_FTS_OR_TERMS: usize = 32;

/// Lexicon classes whose members are close synonyms, so OR-expanding a query token with its
/// classmates raises recall (especially for preference queries) without much precision loss.
/// Deliberately narrow: `verb_buy` and the emotion classes are excluded for now because their
/// members are polysemous ("got", "received") or split across many small classes, which would
/// add noise. Broadening waits on the offline harness showing a gain.
const FTS_SYNONYM_CLASSES: &[&str] = &["verb_like", "verb_dislike"];

/// Reverse lookup: folded query token -> the synonym set (surface forms) of the lexicon class
/// it belongs to. Built once for English; multilingual expansion waits on the language
/// detection owned by the German retrieval plan. Multiword / underscore / apostrophe entries
/// are dropped so every emitted term stays a single clean FTS token.
static SYNONYM_MAP: LazyLock<HashMap<String, Vec<String>>> = LazyLock::new(|| {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for class in FTS_SYNONYM_CLASSES {
        let words: Vec<String> = crate::lexicon::word_class("en", class)
            .into_iter()
            .filter(|w| !w.contains(|c: char| c.is_whitespace() || c == '_' || c == '\''))
            .collect();
        // Key every class member by its folded form so an inflected query token (e.g.
        // "preferred") resolves to the same synonym set as its base form.
        for w in &words {
            let key = crate::lexicon::fold_for_matching(w, "en", true);
            if key.len() >= 2 {
                map.entry(key).or_default().extend(words.iter().cloned());
            }
        }
    }
    for syns in map.values_mut() {
        syns.sort();
        syns.dedup();
    }
    map
});

/// Whether query synonym expansion is enabled (KLEOS_FTS_SYNONYMS=1/true; default off).
fn fts_synonyms_enabled() -> bool {
    static ENABLED: LazyLock<bool> = LazyLock::new(|| {
        std::env::var("KLEOS_FTS_SYNONYMS")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    });
    *ENABLED
}

/// Push a term into the OR-expression list, deduplicated case-insensitively.
fn push_fts_term(terms: &mut Vec<String>, seen: &mut HashSet<String>, term: &str) {
    if seen.insert(term.to_ascii_lowercase()) {
        terms.push(term.to_string());
    }
}

/// Build an OR-of-tokens FTS5 MATCH expression for memory search.
///
/// Space-joined tokens (see `sanitize_fts_query`) are an implicit AND in FTS5, so a
/// multi-term natural-language query returns zero hits unless every stem co-occurs in
/// one document. That collapses hybrid search to vector-only whenever one term is
/// missing. Memory search instead ORs the tokens, so partial matches surface while
/// BM25 still ranks documents that match more terms higher. Stopwords are dropped and the
/// term count is capped (see FTS_STOPWORDS / MAX_FTS_OR_TERMS) so the disjunction stays
/// focused and bounded. Each token is alphanumeric-only (special chars already mapped to
/// spaces) and wrapped as a quoted phrase so that FTS5 boolean keywords appearing inside the
/// user query (AND/OR/NOT/NEAR) cannot be reinterpreted as operators. Returns an empty string
/// when no usable token remains, matching `sanitize_fts_query`'s contract.
pub fn fts_or_match_query(query: &str) -> String {
    // Same character sanitisation as sanitize_fts_query: keep alphanumerics and
    // whitespace, replace every other character with a space.
    let cleaned: String = query
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() {
                c
            } else {
                ' '
            }
        })
        .collect();
    // Default path (unchanged): keep meaningful tokens (>= 2 chars, non-stopword), cap the
    // count, quote each, OR-join. Kept byte-identical so the offline FTS eval is unaffected
    // when synonym expansion is off (the default).
    if !fts_synonyms_enabled() {
        return cleaned
            .split_whitespace()
            .filter(|w| w.len() >= 2)
            .filter(|w| !FTS_STOPWORDS.contains(&w.to_ascii_lowercase().as_str()))
            .take(MAX_FTS_OR_TERMS)
            .map(|w| format!("\"{w}\""))
            .collect::<Vec<_>>()
            .join(" OR ");
    }

    // B.6 expansion path: additionally OR in lexicon synonyms of each content token so a
    // preference query ("what do I enjoy?") also matches memories phrased with a synonym
    // ("I love X"). Porter stemming at the FTS layer already covers inflection, so this adds
    // only genuine synonyms. Terms are deduped case-insensitively and capped at the same bound.
    let mut terms: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for w in cleaned.split_whitespace() {
        if w.len() < 2 || FTS_STOPWORDS.contains(&w.to_ascii_lowercase().as_str()) {
            continue;
        }
        push_fts_term(&mut terms, &mut seen, w);
        if let Some(synonyms) = SYNONYM_MAP.get(&crate::lexicon::fold_for_matching(w, "en", true)) {
            for s in synonyms {
                push_fts_term(&mut terms, &mut seen, s);
            }
        }
        if terms.len() >= MAX_FTS_OR_TERMS {
            break;
        }
    }
    terms.truncate(MAX_FTS_OR_TERMS);
    terms
        .iter()
        .map(|t| format!("\"{t}\""))
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// Maximum FTS query length in bytes. Queries beyond this are rejected to
/// prevent denial-of-service through pathological FTS5 expressions.
use crate::validation::MAX_FTS_QUERY_LEN;

/// Search memories using FTS5 full-text search with BM25 ranking.
/// Returns up to `limit` results ordered by relevance (most relevant first).
#[tracing::instrument(skip(db, query), fields(query_len = query.len(), limit, user_id))]
pub async fn fts_search(
    db: &Database,
    query: &str,
    limit: usize,
    user_id: i64,
) -> Result<Vec<FtsHit>> {
    // SECURITY (SEC-MED-9): reject oversized queries before sanitization to
    // avoid CPU-intensive tokenisation on pathologically large input.
    if query.len() > MAX_FTS_QUERY_LEN {
        return Err(EngError::InvalidInput(format!(
            "query exceeds maximum length of {} bytes",
            MAX_FTS_QUERY_LEN
        )));
    }
    // Memory search ORs the tokens (not the implicit-AND of sanitize_fts_query) so a
    // multi-term query does not zero out when one term is absent from a document.
    let sanitized = fts_or_match_query(query);
    if sanitized.is_empty() {
        return Ok(vec![]);
    }

    // FTS5 match query joined with memories for user/forgotten filtering.
    // The built-in rank column returns negative scores (more negative = more relevant).
    // We negate it so bm25_score is positive and larger = more relevant.
    // The owner predicate (?2) keeps single-DB (shared) mode from returning
    // another user's full-text hits; a no-op in a single-owner shard.
    let sql = "
        SELECT m.id, -memories_fts.rank as bm25_score
        FROM memories_fts
        JOIN memories m ON m.id = memories_fts.rowid
        WHERE memories_fts MATCH ?1
          AND m.user_id = ?2
          AND m.is_forgotten = 0
          AND m.is_latest = 1
        ORDER BY memories_fts.rank
        LIMIT ?3
    ";

    match db
        .read(move |conn| {
            let mut stmt = conn.prepare(sql)?;
            let mut rows = stmt.query(rusqlite::params![sanitized, user_id, limit as i64])?;

            // 6.9 capacity hint: LIMIT bounds the row count.
            let mut hits = Vec::with_capacity(limit);
            let mut pos: usize = 0;
            while let Some(row) = rows.next()? {
                let memory_id: i64 = row.get(0)?;
                let bm25_score: f64 = row.get(1)?;
                hits.push(FtsHit {
                    memory_id,
                    rank: pos,
                    bm25_score,
                });
                pos += 1;
            }

            Ok(hits)
        })
        .await
    {
        Ok(hits) => Ok(hits),
        Err(e) => {
            warn!("fts search failed: {}", e);
            Ok(vec![])
        }
    }
}

/// Unit tests for the FTS query builder and the lexicon-driven synonym map.
#[cfg(test)]
mod tests {
    use super::*;

    // Expansion must link preference verbs within a class. The lookup folds the query token the
    // same way the map keys are built, so inflected tokens (e.g. "preferred") still resolve.
    #[test]
    fn synonym_map_links_preference_verbs() {
        let key = crate::lexicon::fold_for_matching("enjoy", "en", true);
        let syns = SYNONYM_MAP
            .get(&key)
            .expect("enjoy should be a known preference verb");
        for expected in ["love", "like", "adore", "prefer"] {
            assert!(
                syns.iter().any(|s| s == expected),
                "verb_like expansion missing `{expected}`"
            );
        }
    }

    // Like and dislike are distinct classes; expansion must not bleed across valence.
    #[test]
    fn synonym_map_keeps_like_and_dislike_separate() {
        let like_key = crate::lexicon::fold_for_matching("love", "en", true);
        let likes = SYNONYM_MAP
            .get(&like_key)
            .expect("love is a known like verb");
        assert!(
            !likes.iter().any(|s| s == "hate"),
            "like class must not include dislike synonyms"
        );
        let dislike_key = crate::lexicon::fold_for_matching("hate", "en", true);
        let dislikes = SYNONYM_MAP
            .get(&dislike_key)
            .expect("hate is a known dislike verb");
        assert!(dislikes.iter().any(|s| s == "detest"));
    }

    // With KLEOS_FTS_SYNONYMS unset (the default in the test env), the query builder must stay a
    // plain OR of quoted content tokens -- the path the offline FTS eval depends on.
    #[test]
    fn default_path_is_plain_or_of_tokens() {
        assert_eq!(
            fts_or_match_query("spawn tokio task"),
            "\"spawn\" OR \"tokio\" OR \"task\""
        );
    }
}
