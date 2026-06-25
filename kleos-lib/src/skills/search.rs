use super::{row_to_skill, Skill, SKILL_COLUMNS};
use crate::db::Database;
use crate::Result;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Search skills using FTS, scoped to the calling user.
///
/// Results are filtered to rows where `skill_records.user_id = user_id` so
/// that single-DB mode cannot leak one user's skills into another's search
/// results. Migration 78 (monolith) / v69 (tenant) restored the column.
#[tracing::instrument(skip(db, query), fields(query_len = query.len(), limit))]
pub async fn search_skills(
    db: &Database,
    query: &str,
    user_id: i64,
    limit: usize,
) -> Result<Vec<Skill>> {
    // Sanitize query for FTS5.
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
    let sanitized = sanitized
        .split_whitespace()
        .filter(|w| w.len() >= 2)
        .collect::<Vec<_>>()
        .join(" ");

    if sanitized.is_empty() {
        return Ok(vec![]);
    }

    let sql = format!(
        "SELECT {} FROM skill_records sr \
         JOIN (SELECT rowid FROM skills_fts WHERE skills_fts MATCH ?1) fts ON fts.rowid = sr.id \
         WHERE sr.is_active = 1 AND sr.user_id = ?3 \
         ORDER BY sr.trust_score DESC LIMIT ?2",
        SKILL_COLUMNS
    );

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql)?;
        let skills = stmt
            .query_map(params![sanitized, limit as i64, user_id], row_to_skill)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(skills)
    })
    .await
}

// -----------------------------------------------------------------------
// Hybrid find -- Skills Cloud (v50+)
// -----------------------------------------------------------------------
//
// `find_skills` is the on-demand dispatch entry point. Combines:
//   - FTS5 keyword match on (name, description, code) -- weight 1.0
//   - alias exact / prefix match -- weight 2.0 (highest single signal)
//   - kind / plugin / tag filters
//   - trust_score multiplier so high-trust skills float up among ties
//
// Trigram fuzzy + vector cosine are scaffolded but skipped when the
// supporting tables / embeddings are absent. They become active once
// Phase 1.5 (trigrams) and Phase 1.6 (embeddings) ship.

/// One ranked candidate returned by `find_skills`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindResult {
    pub skill: Skill,
    pub score: f64,
    // Per-signal contributions (debug / explain mode).
    pub fts_score: f64,
    pub alias_score: f64,
    pub fuzzy_score: f64,
    pub vector_score: f64,
}

/// Filter knobs threaded into `find_skills`; `None` means no filter on that dimension.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FindOpts {
    pub kind: Option<String>,
    pub plugin: Option<String>,
    pub tag: Option<String>,
    pub limit: Option<usize>,
    pub include_deprecated: Option<bool>,
}

/// Internal accumulator; per-skill signal components summed before trust multiplication.
#[derive(Default, Clone, Copy)]
struct Score {
    fts: f64,
    alias: f64,
    fuzzy: f64,
    vector: f64,
}

// Score composition helpers.
impl Score {
    /// Returns the weighted composite of all signal components.
    fn weighted(&self) -> f64 {
        self.fts * 1.0 + self.alias * 2.0 + self.fuzzy * 0.7 + self.vector * 1.5
    }
}

use crate::memory::fts::sanitize_fts_query as sanitize_fts;

/// Hybrid skill search combining FTS5, alias, fuzzy, and vector signals.
///
/// `user_id` scopes every candidate read to `skill_records.user_id = user_id`
/// so single-DB (monolith) mode cannot surface another tenant's skills. The
/// predicate is a no-op in a single-owner shard. A caller with no owned skills
/// matching the query gets an empty result set, not an error.
#[tracing::instrument(skip(db, query), fields(query_len = query.len()))]
pub async fn find_skills(
    db: &Database,
    query: &str,
    user_id: i64,
    opts: FindOpts,
) -> Result<Vec<FindResult>> {
    let limit = opts.limit.unwrap_or(20).clamp(1, 200);
    let sanitized = sanitize_fts(query);
    let raw_query = query.trim().to_lowercase();
    if sanitized.is_empty() && raw_query.is_empty() {
        return Ok(Vec::new());
    }

    // Pull a wider candidate set than `limit` so filter+rerank still has
    // material to work with. 4x the requested limit, capped at 200.
    let candidate_pool = (limit * 4).min(200);

    // 1. FTS5 candidates (keyword match).
    let fts_hits = if sanitized.is_empty() {
        Vec::new()
    } else {
        fts_candidates(db, &sanitized, user_id, candidate_pool).await?
    };

    // 2. Alias candidates (exact + prefix). Goes through the aliases
    //    module so we get the same ranking logic the dispatch layer uses.
    let alias_hits = if raw_query.is_empty() {
        Vec::new()
    } else {
        super::aliases::resolve_alias(db, &raw_query, user_id, candidate_pool).await?
    };

    // 3. Score composition.
    let mut scores: HashMap<i64, Score> = HashMap::new();
    for (id, rank) in &fts_hits {
        // FTS5 returns lower rank = better; flip into a [0, 1] band by
        // dividing 1.0 by (1 + position).
        let entry = scores.entry(*id).or_default();
        entry.fts = entry.fts.max(1.0 / (1.0 + *rank as f64));
    }
    for hit in &alias_hits {
        let entry = scores.entry(hit.skill_id).or_default();
        entry.alias = entry.alias.max(hit.confidence);
    }

    if scores.is_empty() {
        return Ok(Vec::new());
    }

    // 4. Hydrate skills (single read; we already have the candidate id set).
    let ids: Vec<i64> = scores.keys().copied().collect();
    let skills = fetch_by_ids(db, &ids, user_id, opts.include_deprecated.unwrap_or(false)).await?;

    // 5. Apply filters + compose final score.
    let mut results: Vec<FindResult> = skills
        .into_iter()
        .filter_map(|s| {
            // Kind filter -- exact match on the v50 kind column.
            if let Some(ref k) = opts.kind {
                if s.kind != *k {
                    return None;
                }
            }
            // Plugin filter -- exact match on source_plugin.
            if let Some(ref p) = opts.plugin {
                match s.source_plugin.as_deref() {
                    Some(sp) if sp == p => {}
                    _ => return None,
                }
            }
            // Score; trust_score is in [0,100] so divide by 100 before
            // using as a multiplier (avoids one skill at trust 99 dwarfing
            // every other signal).
            let raw = scores.get(&s.id).copied().unwrap_or_default();
            let trust_mult = 1.0 + (s.trust_score / 100.0);
            let score = raw.weighted() * trust_mult;
            Some(FindResult {
                skill: s,
                score,
                fts_score: raw.fts,
                alias_score: raw.alias,
                fuzzy_score: raw.fuzzy,
                vector_score: raw.vector,
            })
        })
        .collect();

    // Tag filter requires a per-skill tag lookup; only run it if asked.
    if let Some(ref tag) = opts.tag {
        let tag_owned = tag.clone();
        let tag_owned_for_db = tag_owned.clone();
        let candidate_ids: Vec<i64> = results.iter().map(|r| r.skill.id).collect();
        if !candidate_ids.is_empty() {
            let kept: std::collections::HashSet<i64> =
                ids_with_tag(db, &candidate_ids, &tag_owned_for_db).await?;
            results.retain(|r| kept.contains(&r.skill.id));
        }
    }

    // Sort by composite score DESC, ties by trust_score DESC, then id ASC.
    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.skill
                    .trust_score
                    .partial_cmp(&a.skill.trust_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then(a.skill.id.cmp(&b.skill.id))
    });
    results.truncate(limit);
    Ok(results)
}

/// Returns the top-N FTS candidate skill ids with their result-set position as rank.
///
/// `user_id` restricts candidates to `skill_records.user_id = user_id`; the
/// predicate is a no-op in a single-owner shard and the tenant boundary in
/// monolith mode.
async fn fts_candidates(
    db: &Database,
    sanitized: &str,
    user_id: i64,
    limit: usize,
) -> Result<Vec<(i64, usize)>> {
    let q = sanitized.to_string();
    let limit_i = limit as i64;
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT sr.id FROM skill_records sr \
                 JOIN (SELECT rowid FROM skills_fts WHERE skills_fts MATCH ?1) fts \
                 ON fts.rowid = sr.id \
                 WHERE sr.is_active = 1 AND sr.user_id = ?3 \
                 ORDER BY sr.trust_score DESC \
                 LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![q, limit_i, user_id], |r| r.get::<_, i64>(0))?;
        let mut out = Vec::new();
        for (idx, r) in rows.enumerate() {
            let id = r?;
            out.push((id, idx));
        }
        Ok(out)
    })
    .await
}

/// Fetches full `Skill` rows for a candidate id set in a single query.
///
/// `user_id` scopes the hydration to `skill_records.user_id = user_id` so a
/// candidate id that slipped in from another tenant cannot be returned; the
/// predicate is a no-op in a single-owner shard.
async fn fetch_by_ids(
    db: &Database,
    ids: &[i64],
    user_id: i64,
    include_deprecated: bool,
) -> Result<Vec<Skill>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    // Build "?,?,?" placeholders. Cap at 500 to stay below SQLite's
    // default expression limit; callers should never pass more.
    let ids = ids.to_vec();
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let dep_clause = if include_deprecated {
        ""
    } else {
        " AND is_deprecated = 0"
    };
    // The trailing `AND user_id = ?` binds after the IN(...) placeholders; its
    // value is pushed last onto the bound parameter vector below.
    let sql = format!(
        "SELECT {cols} FROM skill_records \
         WHERE id IN ({ph}) AND is_active = 1{dep} AND user_id = ?",
        cols = SKILL_COLUMNS,
        ph = placeholders,
        dep = dep_clause
    );
    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql)?;
        let mut bound: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        bound.push(&user_id);
        let rows = stmt.query_map(bound.as_slice(), row_to_skill)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    })
    .await
}

/// Filters a candidate id set to those carrying the requested tag.
async fn ids_with_tag(
    db: &Database,
    ids: &[i64],
    tag: &str,
) -> Result<std::collections::HashSet<i64>> {
    let ids = ids.to_vec();
    let tag = tag.to_string();
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT skill_id FROM skill_tags \
         WHERE tag = ? AND skill_id IN ({ph})",
        ph = placeholders
    );
    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql)?;
        let mut bound: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(1 + ids.len());
        bound.push(&tag);
        for id in &ids {
            bound.push(id);
        }
        let rows = stmt.query_map(bound.as_slice(), |r| r.get::<_, i64>(0))?;
        let mut out = std::collections::HashSet::new();
        for r in rows {
            out.insert(r?);
        }
        Ok(out)
    })
    .await
}

/// Shared-DB scoping regressions for skill search. Two users live in one
/// monolith database; the search read paths must never surface another user's
/// `skill_records` rows when `user_id` is the tenant boundary.
#[cfg(test)]
mod scope_tests {
    use super::*;
    use crate::skills::create_skill;
    use crate::skills::types::CreateSkillRequest;

    /// Build a shared monolith in-memory database with the v50 `skill_aliases`
    /// table added. That table lives only in the tenant shard schema, but
    /// `find_skills` resolves aliases on every call, so it must exist here for
    /// the search path to run end to end.
    async fn monolith() -> Database {
        let db = Database::connect_memory().await.expect("monolith db");
        db.write(|conn| {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS skill_aliases ( \
                     id INTEGER PRIMARY KEY AUTOINCREMENT, \
                     alias TEXT NOT NULL, \
                     skill_id INTEGER NOT NULL REFERENCES skill_records(id) ON DELETE CASCADE, \
                     confidence REAL NOT NULL DEFAULT 1.0, \
                     source TEXT NOT NULL DEFAULT 'auto', \
                     created_at TEXT NOT NULL DEFAULT (datetime('now')), \
                     UNIQUE(alias, skill_id) \
                 );",
            )?;
            Ok(())
        })
        .await
        .expect("create skill_aliases");
        db
    }

    /// Insert one active skill owned by `user_id` carrying the FTS token
    /// `widget`, and return its row id.
    async fn make_widget(db: &Database, name: &str, user_id: i64) -> i64 {
        let req = CreateSkillRequest {
            name: name.to_string(),
            agent: "test".to_string(),
            description: Some("widget search scope skill".to_string()),
            code: "fn run() {}".to_string(),
            language: Some("rust".to_string()),
            parent_skill_id: None,
            metadata: None,
            user_id: Some(user_id),
            tags: None,
            tool_deps: None,
            kind: None,
            source_plugin: None,
            source_path: None,
            content_hash: None,
        };
        create_skill(db, req).await.expect("create skill").id
    }

    /// `find_skills` returns only the caller's own matching skills.
    #[tokio::test]
    async fn find_skills_is_scoped_to_caller() {
        let db = monolith().await;
        let alice = make_widget(&db, "alice_widget", 1).await;
        let bob = make_widget(&db, "bob_widget", 2).await;

        let mine = find_skills(&db, "widget", 1, FindOpts::default())
            .await
            .expect("find as user 1");
        let mine_ids: Vec<i64> = mine.iter().map(|r| r.skill.id).collect();
        assert!(mine_ids.contains(&alice), "owner skill should be found");
        assert!(
            !mine_ids.contains(&bob),
            "another user's skill must not appear in the caller's search"
        );

        let theirs = find_skills(&db, "widget", 2, FindOpts::default())
            .await
            .expect("find as user 2");
        let their_ids: Vec<i64> = theirs.iter().map(|r| r.skill.id).collect();
        assert!(their_ids.contains(&bob));
        assert!(!their_ids.contains(&alice));
    }

    /// A caller with no matching owned skills gets an empty result, not an error.
    #[tokio::test]
    async fn find_skills_empty_for_non_owner() {
        let db = monolith().await;
        let _alice = make_widget(&db, "alice_widget", 1).await;
        let res = find_skills(&db, "widget", 3, FindOpts::default())
            .await
            .expect("find as user 3");
        assert!(res.is_empty(), "non-owner search returns empty, not error");
    }

    /// `fetch_by_ids` will not hydrate a row owned by another user even when its
    /// id is handed in explicitly.
    #[tokio::test]
    async fn fetch_by_ids_is_scoped() {
        let db = monolith().await;
        let alice = make_widget(&db, "alice_widget", 1).await;
        let bob = make_widget(&db, "bob_widget", 2).await;

        let rows = fetch_by_ids(&db, &[alice, bob], 1, false)
            .await
            .expect("fetch");
        let ids: Vec<i64> = rows.iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![alice], "only the caller's row hydrates");
    }
}
