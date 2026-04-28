use super::{row_to_skill, Skill, SKILL_COLUMNS};
use crate::db::Database;
use crate::{EngError, Result};
use rusqlite::params;

/// Search skills using FTS.
///
/// `_user_id` is retained in the signature for API compatibility with
/// callers in handlers that have not yet dropped the param. The
/// `skill_records.user_id` column was removed by migration 42
/// (drop_user_id_skills) so the query no longer filters on it.
#[tracing::instrument(skip(db, query), fields(query_len = query.len(), limit))]
pub async fn search_skills(
    db: &Database,
    query: &str,
    _user_id: i64,
    limit: usize,
) -> Result<Vec<Skill>> {
    // Sanitize query for FTS5
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
         WHERE sr.is_active = 1 \
         ORDER BY sr.trust_score DESC LIMIT ?2",
        SKILL_COLUMNS
    );

    db.read(move |conn| {
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        let skills = stmt
            .query_map(params![sanitized, limit as i64], |row| row_to_skill(row))
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        Ok(skills)
    })
    .await
}
