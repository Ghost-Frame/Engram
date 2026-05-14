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
pub async fn add_alias(
    db: &Database,
    skill_id: i64,
    alias: &str,
    confidence: f64,
    source: &str,
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
             VALUES (?1, ?2, ?3, ?4)",
            params![alias_clone, skill_id, confidence, source],
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        // Return the row id of the matching pair (newly inserted or
        // pre-existing -- INSERT OR IGNORE leaves last_insert_rowid at 0
        // on conflict, so look it up explicitly).
        let id: i64 = conn
            .query_row(
                "SELECT id FROM skill_aliases WHERE alias = ?1 AND skill_id = ?2",
                params![alias, skill_id],
                |r| r.get(0),
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        Ok(id)
    })
    .await
}

// Bulk insert helper used by the importer. Each entry is (alias, confidence).
// Source is fixed to "auto"; user-driven aliases go through `add_alias`
// so the audit trail stays clean.
pub async fn add_auto_aliases(
    db: &Database,
    skill_id: i64,
    aliases: &[(String, f64)],
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
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        for (a, c) in &aliases {
            tx.execute(
                "INSERT OR IGNORE INTO skill_aliases (alias, skill_id, confidence, source) \
                 VALUES (?1, ?2, ?3, 'auto')",
                params![a, skill_id, c],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        }
        tx.commit()
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        Ok(())
    })
    .await
}

// Remove a single (alias, skill_id) pair. Returns rows deleted (0 or 1).
pub async fn remove_alias(db: &Database, skill_id: i64, alias: &str) -> Result<usize> {
    let alias = alias.trim().to_lowercase();
    db.write(move |conn| {
        let n = conn
            .execute(
                "DELETE FROM skill_aliases WHERE alias = ?1 AND skill_id = ?2",
                params![alias, skill_id],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        Ok(n)
    })
    .await
}

// Drop every auto-generated alias for a skill; used when the importer
// rewrites a row and wants the alias set rebuilt from scratch. User-added
// aliases (source='user') are preserved.
pub async fn clear_auto_aliases(db: &Database, skill_id: i64) -> Result<usize> {
    db.write(move |conn| {
        let n = conn
            .execute(
                "DELETE FROM skill_aliases WHERE skill_id = ?1 AND source = 'auto'",
                params![skill_id],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        Ok(n)
    })
    .await
}

// List every alias attached to a skill, newest first.
pub async fn list_for_skill(db: &Database, skill_id: i64) -> Result<Vec<SkillAlias>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, alias, skill_id, confidence, source, created_at \
                 FROM skill_aliases WHERE skill_id = ?1 \
                 ORDER BY created_at DESC, id DESC",
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        let rows = stmt
            .query_map(params![skill_id], |r| {
                Ok(SkillAlias {
                    id: r.get(0)?,
                    alias: r.get(1)?,
                    skill_id: r.get(2)?,
                    confidence: r.get(3)?,
                    source: r.get(4)?,
                    created_at: r.get(5)?,
                })
            })
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| EngError::DatabaseMessage(e.to_string()))?);
        }
        Ok(out)
    })
    .await
}

// Resolve a string to candidate skill ids. Tries exact match first, then
// case-insensitive prefix. Returns up to `limit` matches ordered by
// confidence DESC. The hybrid search layer combines this with FTS and
// vector signals; this fn alone is sufficient for "/skill <exact-alias>".
pub async fn resolve_alias(db: &Database, query: &str, limit: usize) -> Result<Vec<AliasMatch>> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let limit_i = limit as i64;
    db.read(move |conn| {
        // Exact match first; ORDER BY confidence DESC so user-added 1.0
        // aliases win over auto 0.7 prefix-aliases.
        let mut stmt = conn
            .prepare(
                "SELECT skill_id, alias, confidence, source \
                 FROM skill_aliases WHERE alias = ?1 \
                 ORDER BY confidence DESC, id DESC LIMIT ?2",
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        let mut hits: Vec<AliasMatch> = stmt
            .query_map(params![q, limit_i], |r| {
                Ok(AliasMatch {
                    skill_id: r.get(0)?,
                    alias: r.get(1)?,
                    confidence: r.get(2)?,
                    source: r.get(3)?,
                })
            })
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();
        // If we still have headroom, fall back to prefix match. Confidence
        // is dampened by 0.7 to keep prefix hits below exact hits.
        if hits.len() < limit {
            let remaining = (limit - hits.len()) as i64;
            let pattern = format!("{}%", q);
            let mut stmt2 = conn
                .prepare(
                    "SELECT skill_id, alias, confidence, source \
                     FROM skill_aliases WHERE alias LIKE ?1 AND alias != ?2 \
                     ORDER BY confidence DESC, length(alias) ASC LIMIT ?3",
                )
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            let prefix_hits: Vec<AliasMatch> = stmt2
                .query_map(params![pattern, q, remaining], |r| {
                    Ok(AliasMatch {
                        skill_id: r.get(0)?,
                        alias: r.get(1)?,
                        confidence: r.get::<_, f64>(2)? * 0.7,
                        source: r.get(3)?,
                    })
                })
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
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
