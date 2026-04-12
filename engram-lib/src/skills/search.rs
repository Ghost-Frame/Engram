use super::{row_to_skill, Skill, SKILL_COLUMNS};
use crate::db::Database;
use crate::{EngError, Result};
use rusqlite::params;

/// Search skills using FTS.
pub async fn search_skills(
    db: &Database,
    query: &str,
    user_id: i64,
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
         WHERE sr.user_id = ?2 AND sr.is_active = 1 \
         ORDER BY sr.trust_score DESC LIMIT ?3",
        SKILL_COLUMNS
    );

    db.read(move |conn| {
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        let skills = stmt
            .query_map(params![sanitized, user_id, limit as i64], |row| {
                row_to_skill(row)
            })
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        Ok(skills)
    })
    .await
}
