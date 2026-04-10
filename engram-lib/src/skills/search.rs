use crate::db::Database;
use crate::Result;
use super::{row_to_skill, Skill, SKILL_COLUMNS};
use libsql::params;

/// Search skills using FTS.
pub async fn search_skills(db: &Database, query: &str, user_id: i64, limit: usize) -> Result<Vec<Skill>> {
    let conn = db.connection();

    // Sanitize query for FTS5
    let sanitized: String = query.chars()
        .map(|c| if c.is_alphanumeric() || c.is_whitespace() { c } else { ' ' })
        .collect();
    let sanitized = sanitized.split_whitespace()
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

    let mut rows = conn.query(&sql, params![sanitized, user_id, limit as i64]).await?;
    let mut skills = Vec::new();
    while let Some(row) = rows.next().await? {
        skills.push(row_to_skill(&row)?);
    }
    Ok(skills)
}
