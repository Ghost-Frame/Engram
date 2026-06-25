//! Skill aliases -- fuzzy / nickname dispatch for the Skills Cloud (v50+).
//!
//! Aliases solve two problems:
//! 1. **Collisions across plugins.** `superpowers__brainstorming` and any
//!    future `xyz__brainstorming` both need to resolve when a caller asks
//!    for "brainstorming". Each gets the bare-name alias inserted; the
//!    search layer ranks among them by trust_score.
//! 2. **User shortcuts.** A user can hand-add `bs` -> a brainstorming
//!    skill via `kleos-cli skill alias add ...`.
//!
//! The table is intentionally simple: alias TEXT, skill_id FK, confidence,
//! source ('auto' from the importer or 'user' from the CLI), created_at.
//! UNIQUE(alias, skill_id) prevents dupe inserts on idempotent re-import.

use crate::db::Database;
use crate::{EngError, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};

// One alias row. Multiple rows can share `alias` when the same nickname
// resolves to several skills; the caller is expected to disambiguate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillAlias {
    pub id: i64,
    pub alias: String,
    pub skill_id: i64,
    pub confidence: f64,
    pub source: String,
    pub created_at: String,
}

// One match returned by `resolve_alias`: the target skill plus the
// confidence stamped on the alias row that matched. Search layers combine
// this with FTS / vector / fuzzy scores.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AliasMatch {
    pub skill_id: i64,
    pub alias: String,
    pub confidence: f64,
    pub source: String,
}

// Insert a single alias. Idempotent: existing (alias, skill_id) pairs are
// silently kept. Returns the row id (existing or newly inserted).
//
// `skill_aliases` carries no `user_id` of its own, so ownership is enforced by
// joining `skill_records`: the insert only fires when `skill_id` belongs to
// `user_id`, and the id lookup is gated the same way. A caller that does not
// own `skill_id` adds nothing and the lookup returns no row (a not-found
// error), so aliases cannot be attached to another tenant's skill. The
// ownership predicate is a no-op in a single-owner shard.
pub async fn add_alias(
    db: &Database,
    skill_id: i64,
    alias: &str,
    confidence: f64,
    source: &str,
    user_id: i64,
) -> Result<i64> {
    let alias = alias.trim().to_lowercase();
    if alias.is_empty() {
        return Err(EngError::InvalidInput("alias cannot be empty".into()));
    }
    let alias_clone = alias.clone();
    let source = source.to_string();
    db.write(move |conn| {
        conn.execute(
            "INSERT OR IGNORE INTO skill_aliases (alias, skill_id, confidence, source) \
             SELECT ?1, ?2, ?3, ?4 \
             WHERE EXISTS (SELECT 1 FROM skill_records WHERE id = ?2 AND user_id = ?5)",
            params![alias_clone, skill_id, confidence, source, user_id],
        )?;
        // Return the row id of the matching pair (newly inserted or
        // pre-existing -- INSERT OR IGNORE leaves last_insert_rowid at 0
        // on conflict, so look it up explicitly). The join to skill_records
        // keeps a non-owner from reading back another tenant's alias id.
        let id: i64 = conn.query_row(
            "SELECT a.id FROM skill_aliases a \
             JOIN skill_records sr ON sr.id = a.skill_id \
             WHERE a.alias = ?1 AND a.skill_id = ?2 AND sr.user_id = ?3",
            params![alias, skill_id, user_id],
            |r| r.get(0),
        )?;
        Ok(id)
    })
    .await
}

// Bulk insert helper used by the importer. Each entry is (alias, confidence).
// Source is fixed to "auto"; user-driven aliases go through `add_alias`
// so the audit trail stays clean.
//
// Ownership comes from `skill_records`: each INSERT only lands when `skill_id`
// belongs to `user_id` (the same WHERE EXISTS guard as `add_alias`), so an
// importer fed a skill_id it does not own cannot write cross-tenant aliases.
// The predicate is a no-op in a single-owner shard.
pub async fn add_auto_aliases(
    db: &Database,
    skill_id: i64,
    aliases: &[(String, f64)],
    user_id: i64,
) -> Result<()> {
    let aliases: Vec<(String, f64)> = aliases
        .iter()
        .map(|(a, c)| (a.trim().to_lowercase(), *c))
        .filter(|(a, _)| !a.is_empty())
        .collect();
    if aliases.is_empty() {
        return Ok(());
    }
    db.write(move |conn| {
        let tx = conn.unchecked_transaction()?;
        for (a, c) in &aliases {
            tx.execute(
                "INSERT OR IGNORE INTO skill_aliases (alias, skill_id, confidence, source) \
                 SELECT ?1, ?2, ?3, 'auto' \
                 WHERE EXISTS (SELECT 1 FROM skill_records WHERE id = ?2 AND user_id = ?4)",
                params![a, skill_id, c, user_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    })
    .await
}

// Remove a single (alias, skill_id) pair. Returns rows deleted (0 or 1).
//
// Ownership comes from `skill_records`: the delete only matches when `skill_id`
// belongs to `user_id`, so a caller cannot remove aliases from another tenant's
// skill (the delete affects 0 rows). The predicate is a no-op in a single-owner
// shard.
pub async fn remove_alias(
    db: &Database,
    skill_id: i64,
    alias: &str,
    user_id: i64,
) -> Result<usize> {
    let alias = alias.trim().to_lowercase();
    db.write(move |conn| {
        let n = conn.execute(
            "DELETE FROM skill_aliases WHERE alias = ?1 AND skill_id = ?2 \
             AND skill_id IN (SELECT id FROM skill_records WHERE id = ?2 AND user_id = ?3)",
            params![alias, skill_id, user_id],
        )?;
        Ok(n)
    })
    .await
}

// Drop every auto-generated alias for a skill; used when the importer
// rewrites a row and wants the alias set rebuilt from scratch. User-added
// aliases (source='user') are preserved.
//
// Scoped to `user_id` via `skill_records` so a caller cannot clear another
// tenant's aliases. The predicate is a no-op in a single-owner shard.
pub async fn clear_auto_aliases(db: &Database, skill_id: i64, user_id: i64) -> Result<usize> {
    db.write(move |conn| {
        let n = conn.execute(
            "DELETE FROM skill_aliases WHERE skill_id = ?1 AND source = 'auto' \
             AND skill_id IN (SELECT id FROM skill_records WHERE id = ?1 AND user_id = ?2)",
            params![skill_id, user_id],
        )?;
        Ok(n)
    })
    .await
}

// List every alias attached to a skill, newest first.
//
// Joined to `skill_records` so only the owner (`user_id`) of `skill_id` sees
// its aliases; a non-owner gets an empty list. The predicate is a no-op in a
// single-owner shard.
pub async fn list_for_skill(db: &Database, skill_id: i64, user_id: i64) -> Result<Vec<SkillAlias>> {
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT a.id, a.alias, a.skill_id, a.confidence, a.source, a.created_at \
                 FROM skill_aliases a \
                 JOIN skill_records sr ON sr.id = a.skill_id \
                 WHERE a.skill_id = ?1 AND sr.user_id = ?2 \
                 ORDER BY a.created_at DESC, a.id DESC",
        )?;
        let rows = stmt.query_map(params![skill_id, user_id], |r| {
            Ok(SkillAlias {
                id: r.get(0)?,
                alias: r.get(1)?,
                skill_id: r.get(2)?,
                confidence: r.get(3)?,
                source: r.get(4)?,
                created_at: r.get(5)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    })
    .await
}

// Resolve a string to candidate skill ids. Tries exact match first, then
// case-insensitive prefix. Returns up to `limit` matches ordered by
// confidence DESC. The hybrid search layer combines this with FTS and
// vector signals; this fn alone is sufficient for "/skill <exact-alias>".
//
// `skill_aliases` has no `user_id`, so both queries join `skill_records` and
// require `skill_records.user_id = user_id`; a caller only resolves aliases
// pointing at skills they own. The predicate is a no-op in a single-owner
// shard and the tenant boundary in monolith mode.
pub async fn resolve_alias(
    db: &Database,
    query: &str,
    user_id: i64,
    limit: usize,
) -> Result<Vec<AliasMatch>> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let limit_i = limit as i64;
    db.read(move |conn| {
        // Exact match first; ORDER BY confidence DESC so user-added 1.0
        // aliases win over auto 0.7 prefix-aliases.
        let mut stmt = conn.prepare(
            "SELECT a.skill_id, a.alias, a.confidence, a.source \
                 FROM skill_aliases a \
                 JOIN skill_records sr ON sr.id = a.skill_id \
                 WHERE a.alias = ?1 AND sr.user_id = ?3 \
                 ORDER BY a.confidence DESC, a.id DESC LIMIT ?2",
        )?;
        let mut hits: Vec<AliasMatch> = stmt
            .query_map(params![q, limit_i, user_id], |r| {
                Ok(AliasMatch {
                    skill_id: r.get(0)?,
                    alias: r.get(1)?,
                    confidence: r.get(2)?,
                    source: r.get(3)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        // If we still have headroom, fall back to prefix match. Confidence
        // is dampened by 0.7 to keep prefix hits below exact hits.
        if hits.len() < limit {
            let remaining = (limit - hits.len()) as i64;
            let pattern = format!("{}%", q);
            let mut stmt2 = conn.prepare(
                "SELECT a.skill_id, a.alias, a.confidence, a.source \
                     FROM skill_aliases a \
                     JOIN skill_records sr ON sr.id = a.skill_id \
                     WHERE a.alias LIKE ?1 AND a.alias != ?2 AND sr.user_id = ?4 \
                     ORDER BY a.confidence DESC, length(a.alias) ASC LIMIT ?3",
            )?;
            let prefix_hits: Vec<AliasMatch> = stmt2
                .query_map(params![pattern, q, remaining, user_id], |r| {
                    Ok(AliasMatch {
                        skill_id: r.get(0)?,
                        alias: r.get(1)?,
                        confidence: r.get::<_, f64>(2)? * 0.7,
                        source: r.get(3)?,
                    })
                })?
                .filter_map(|r| r.ok())
                .collect();
            hits.extend(prefix_hits);
        }
        Ok(hits)
    })
    .await
}

// Generate the canonical auto-alias set for a skill given its (plugin,
// original_name) pair. The importer calls this to populate `skill_aliases`
// so users can refer to skills by bare name, kebab variant, or snake variant.
pub fn auto_aliases_for(plugin: Option<&str>, original_name: &str) -> Vec<(String, f64)> {
    // Normalize to lowercase early.
    let raw = original_name.trim();
    if raw.is_empty() {
        return Vec::new();
    }
    let bare = raw.to_lowercase();
    let snake = bare.replace(['-', ' '], "_");
    let kebab = bare.replace(['_', ' '], "-");

    // Use a small set to dedupe variants that collapse together (e.g.
    // names with no special chars).
    let mut out: Vec<(String, f64)> = Vec::new();
    let mut push = |s: String, c: f64| {
        if !s.is_empty() && !out.iter().any(|(existing, _)| existing == &s) {
            out.push((s, c));
        }
    };
    // Bare name: highest-confidence auto alias since it's the most likely
    // user input ("brainstorming"). 0.9 to leave headroom for user 1.0.
    push(bare.clone(), 0.9);
    push(snake, 0.85);
    push(kebab, 0.85);
    // Plugin-qualified short form (`superpowers/brainstorming`) for the
    // collision case where the caller wants to be explicit without typing
    // the full snake-cased namespaced name.
    if let Some(plug) = plugin {
        let p = plug.to_lowercase();
        push(format!("{p}/{bare}"), 0.95);
        push(format!("{p}:{bare}"), 0.95);
    }
    out
}

/// Unit tests for skill alias generation.
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies kebab and scoped alias variants are produced.
    #[test]
    fn auto_aliases_dedup_kebab_snake() {
        let v = auto_aliases_for(Some("superpowers"), "brainstorming");
        assert!(v.iter().any(|(s, _)| s == "brainstorming"));
        assert!(v.iter().any(|(s, _)| s == "superpowers/brainstorming"));
        assert!(v.iter().any(|(s, _)| s == "superpowers:brainstorming"));
    }

    /// Verifies both kebab-case and snake_case variants appear.
    #[test]
    fn auto_aliases_with_dashes() {
        let v = auto_aliases_for(Some("pr-review-toolkit"), "code-reviewer");
        // Kebab and snake variants both present.
        assert!(v.iter().any(|(s, _)| s == "code-reviewer"));
        assert!(v.iter().any(|(s, _)| s == "code_reviewer"));
    }

    /// Verifies empty or blank inputs produce no aliases.
    #[test]
    fn auto_aliases_empty_input() {
        assert!(auto_aliases_for(None, "").is_empty());
        assert!(auto_aliases_for(Some("p"), "  ").is_empty());
    }
}

/// Shared-DB scoping regressions for the alias surface. `skill_aliases` has no
/// `user_id`; ownership flows from the parent `skill_records` row, so a caller
/// can only resolve, add, remove, or list aliases for skills they own when
/// `user_id` is the tenant boundary (monolith mode).
#[cfg(test)]
mod scope_tests {
    use super::*;
    use crate::skills::create_skill;
    use crate::skills::types::CreateSkillRequest;

    /// Build a shared monolith in-memory database with the v50 `skill_aliases`
    /// table added (it ships only in the tenant shard schema).
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

    /// Insert one skill owned by `user_id` and return its row id.
    async fn make_skill(db: &Database, name: &str, user_id: i64) -> i64 {
        let req = CreateSkillRequest {
            name: name.to_string(),
            agent: "test".to_string(),
            description: Some("alias scope skill".to_string()),
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

    /// `resolve_alias` only returns matches whose skill is owned by the caller.
    #[tokio::test]
    async fn resolve_alias_is_scoped() {
        let db = monolith().await;
        let alice = make_skill(&db, "alice_skill", 1).await;
        let bob = make_skill(&db, "bob_skill", 2).await;
        // Same nickname points at both owners' skills.
        add_alias(&db, alice, "bs", 1.0, "user", 1)
            .await
            .expect("alice alias");
        add_alias(&db, bob, "bs", 1.0, "user", 2)
            .await
            .expect("bob alias");

        let mine = resolve_alias(&db, "bs", 1, 10).await.expect("resolve");
        let ids: Vec<i64> = mine.iter().map(|m| m.skill_id).collect();
        assert_eq!(ids, vec![alice], "only the caller's alias target resolves");
    }

    /// `add_alias` refuses to attach an alias to a skill the caller does not own.
    #[tokio::test]
    async fn add_alias_rejects_foreign_skill() {
        let db = monolith().await;
        let bob = make_skill(&db, "bob_skill", 2).await;

        // User 1 tries to alias user 2's skill: nothing is inserted and the
        // gated id lookup finds no row, so the call errors.
        let attempt = add_alias(&db, bob, "pwn", 1.0, "user", 1).await;
        assert!(attempt.is_err(), "cannot alias another user's skill");

        // The owner sees no such alias either, confirming no row was written.
        let owner_view = resolve_alias(&db, "pwn", 2, 10).await.expect("resolve");
        assert!(
            owner_view.is_empty(),
            "no alias row should exist for the foreign add attempt"
        );
    }

    /// `remove_alias` cannot delete an alias on a skill the caller does not own.
    #[tokio::test]
    async fn remove_alias_is_scoped() {
        let db = monolith().await;
        let bob = make_skill(&db, "bob_skill", 2).await;
        add_alias(&db, bob, "keep", 1.0, "user", 2)
            .await
            .expect("bob alias");

        // User 1 attempts to remove it: 0 rows affected.
        let removed = remove_alias(&db, bob, "keep", 1).await.expect("remove");
        assert_eq!(removed, 0, "non-owner removal must affect no rows");

        // The alias survives for its owner.
        let still = resolve_alias(&db, "keep", 2, 10).await.expect("resolve");
        assert_eq!(still.len(), 1, "owner's alias must remain");
    }

    /// `list_for_skill` returns aliases only to the skill's owner.
    #[tokio::test]
    async fn list_for_skill_is_scoped() {
        let db = monolith().await;
        let bob = make_skill(&db, "bob_skill", 2).await;
        add_alias(&db, bob, "keep", 1.0, "user", 2)
            .await
            .expect("bob alias");

        let foreign = list_for_skill(&db, bob, 1).await.expect("list as user 1");
        assert!(
            foreign.is_empty(),
            "another user's aliases must not be listable"
        );

        let owner = list_for_skill(&db, bob, 2).await.expect("list as user 2");
        assert_eq!(owner.len(), 1, "owner sees their own alias");
        assert_eq!(owner[0].alias, "keep");
    }
}
