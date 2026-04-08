use crate::db::Database;
use crate::{EngError, Result};
use libsql::params;
use serde::{Deserialize, Serialize};

pub const FIX_SYSTEM_PROMPT: &str = "You are a skill fixer. Analyze failures and improve skill content.";
pub const DERIVE_SYSTEM_PROMPT: &str = "You are a skill deriver. Combine parent skills into a new derived skill.";
pub const CAPTURE_SYSTEM_PROMPT: &str = "You are a skill capturer. Create a new skill from a workflow description.";

/// Strip code fences from LLM output.
pub fn strip_code_fences(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.starts_with("```") {
        let lines: Vec<&str> = trimmed.lines().collect();
        if lines.len() >= 2 {
            let start = 1;
            let end = if lines.last().is_some_and(|l| l.trim() == "```") {
                lines.len() - 1
            } else {
                lines.len()
            };
            return lines[start..end].join("
");
        }
    }
    trimmed.to_string()
}

/// Generate a new unique skill ID.
pub fn generate_skill_id(base_name: &str) -> String {
    let slug: String = base_name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    let short_id = &uuid::Uuid::new_v4().to_string()[..8];
    format!("{}-{}", slug, short_id)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionRequest {
    pub evolution_type: String,
    pub target_skill_ids: Vec<i64>,
    pub category: Option<String>,
    pub direction: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionResult {
    pub success: bool,
    pub skill_id: Option<i64>,
    pub evolution_type: String,
    pub message: String,
}

/// Persist an evolved skill to the database.
pub async fn persist_evolved_skill(
    db: &Database, name: &str, description: &str, code: &str,
    agent: &str, parent_ids: &[i64], tags: &[String], user_id: i64,
) -> Result<i64> {
    let conn = db.connection();
    let (version, root_id) = if let Some(&parent_id) = parent_ids.first() {
        let mut rows = conn.query("SELECT version, root_skill_id FROM skill_records WHERE id = ?1", params![parent_id]).await?;
        if let Some(row) = rows.next().await? {
            let pv: i32 = row.get(0)?;
            let pr: Option<i64> = row.get(1)?;
            (pv + 1, pr.or(Some(parent_id)))
        } else { (1, None) }
    } else { (1, None) };

    conn.execute(
        "INSERT INTO skill_records (name, agent, description, code, language, version, parent_skill_id, root_skill_id, user_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![name.to_string(), agent.to_string(), description.to_string(), code.to_string(), "markdown".to_string(), version, parent_ids.first().copied(), root_id, user_id],
    ).await?;

    let mut id_rows = conn.query("SELECT last_insert_rowid()", ()).await?;
    let new_id: i64 = if let Some(row) = id_rows.next().await? { row.get(0)? } else {
        return Err(EngError::Internal("failed to get new skill id".into()));
    };
    for &pid in parent_ids {
        conn.execute("INSERT OR IGNORE INTO skill_lineage_parents (skill_id, parent_id) VALUES (?1, ?2)", params![new_id, pid]).await?;
    }
    for tag in tags {
        conn.execute("INSERT OR IGNORE INTO skill_tags (skill_id, tag) VALUES (?1, ?2)", params![new_id, tag.clone()]).await?;
    }
    Ok(new_id)
}

/// Deactivate a skill (soft-delete).
pub async fn deactivate_skill(db: &Database, skill_id: i64) -> Result<()> {
    let conn = db.connection();
    conn.execute("UPDATE skill_records SET is_active = 0, updated_at = datetime('now') WHERE id = ?1", params![skill_id]).await?;
    Ok(())
}

/// Stub: fix a failing skill.
pub async fn fix_skill(db: &Database, skill_id: i64, _agent: &str, _user_id: i64) -> Result<EvolutionResult> {
    let conn = db.connection();
    let mut rows = conn.query("SELECT name FROM skill_records WHERE id = ?1", params![skill_id]).await?;
    let name: String = rows.next().await?.map(|r| r.get::<String>(0)).transpose()?.ok_or_else(|| EngError::NotFound(format!("skill {} not found", skill_id)))?;
    Ok(EvolutionResult { success: false, skill_id: Some(skill_id), evolution_type: "fix".into(), message: format!("LLM not yet wired for fix of {} (id={})", name, skill_id) })
}

/// Stub: derive a new skill from parents.
pub async fn derive_skill(db: &Database, parent_ids: &[i64], direction: &str, _agent: &str, _user_id: i64) -> Result<EvolutionResult> {
    if parent_ids.is_empty() { return Err(EngError::InvalidInput("derive requires at least one parent".into())); }
    let conn = db.connection();
    for &pid in parent_ids {
        let mut rows = conn.query("SELECT id FROM skill_records WHERE id = ?1", params![pid]).await?;
        if rows.next().await?.is_none() { return Err(EngError::NotFound(format!("parent skill {} not found", pid))); }
    }
    Ok(EvolutionResult { success: false, skill_id: None, evolution_type: "derived".into(), message: format!("LLM not yet wired. Derive {:?}, direction: {}", parent_ids, direction) })
}

/// Stub: capture a new skill from a description.
pub async fn capture_skill(_db: &Database, description: &str, _agent: &str, _user_id: i64) -> Result<EvolutionResult> {
    if description.trim().is_empty() { return Err(EngError::InvalidInput("capture requires a non-empty description".into())); }
    Ok(EvolutionResult { success: false, skill_id: None, evolution_type: "captured".into(), message: format!("LLM not yet wired. Capture from: {}", &description[..description.len().min(100)]) })
}

/// Main evolution dispatcher.
pub async fn evolve(db: &Database, req: &EvolutionRequest, agent: &str, user_id: i64) -> Result<EvolutionResult> {
    match req.evolution_type.as_str() {
        "fix" => { let sid = req.target_skill_ids.first().ok_or_else(|| EngError::InvalidInput("fix requires a target skill id".into()))?; fix_skill(db, *sid, agent, user_id).await }
        "derived" => derive_skill(db, &req.target_skill_ids, &req.direction, agent, user_id).await,
        "captured" => capture_skill(db, &req.direction, agent, user_id).await,
        other => Err(EngError::InvalidInput(format!("unknown evolution type: {}", other))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn test_strip_none() { assert_eq!(strip_code_fences("hello"), "hello"); }
    #[test] fn test_strip_with_lang() {
        let input = "```md
content
```";
        assert_eq!(strip_code_fences(input), "content");
    }
    #[test] fn test_gen_id() {
        let id = generate_skill_id("My Cool Skill");
        assert!(id.starts_with("my-cool-skill-"));
    }
}
