pub mod analyzer;
pub mod cloud;
pub mod conversation_formatter;
pub mod dashboard;
pub mod evolver;
pub mod patch;
pub mod registry;
pub mod search;
pub mod types;

use crate::db::Database;
use crate::{EngError, Result};
use libsql::params;
use serde::{Deserialize, Serialize};

// -- Types --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub id: i64,
    pub name: String,
    pub agent: String,
    pub description: Option<String>,
    pub code: String,
    pub language: String,
    pub version: i32,
    pub parent_skill_id: Option<i64>,
    pub root_skill_id: Option<i64>,
    pub trust_score: f64,
    pub success_count: i32,
    pub failure_count: i32,
    pub execution_count: i32,
    pub avg_duration_ms: Option<f64>,
    pub is_active: bool,
    pub is_deprecated: bool,
    pub metadata: Option<String>,
    pub user_id: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSkillRequest {
    pub name: String,
    pub agent: String,
    pub description: Option<String>,
    pub code: String,
    pub language: Option<String>,
    pub parent_skill_id: Option<i64>,
    pub metadata: Option<String>,
    pub user_id: Option<i64>,
    pub tags: Option<Vec<String>>,
    pub tool_deps: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateSkillRequest {
    pub code: Option<String>,
    pub description: Option<String>,
    pub is_active: Option<bool>,
    pub is_deprecated: Option<bool>,
    pub metadata: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionRecord {
    pub id: i64,
    pub skill_id: i64,
    pub success: bool,
    pub duration_ms: Option<f64>,
    pub error_type: Option<String>,
    pub error_message: Option<String>,
    pub input_hash: Option<String>,
    pub output_hash: Option<String>,
    pub metadata: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillJudgment {
    pub id: i64,
    pub skill_id: i64,
    pub judge_agent: String,
    pub score: f64,
    pub rationale: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolQuality {
    pub id: i64,
    pub tool_name: String,
    pub agent: String,
    pub success: bool,
    pub latency_ms: Option<f64>,
    pub error_type: Option<String>,
    pub created_at: String,
}

// -- Constants --

pub(crate) const SKILL_COLUMNS: &str = "id, name, agent, description, code, language, version, \
    parent_skill_id, root_skill_id, trust_score, success_count, failure_count, \
    execution_count, avg_duration_ms, is_active, is_deprecated, metadata, \
    user_id, created_at, updated_at";

// -- Helpers --

pub(crate) fn row_to_skill(row: &libsql::Row) -> Result<Skill> {
    Ok(Skill {
        id: row.get(0)?,
        name: row.get(1)?,
        agent: row.get(2)?,
        description: row.get(3)?,
        code: row.get(4)?,
        language: row.get(5)?,
        version: row.get(6)?,
        parent_skill_id: row.get(7)?,
        root_skill_id: row.get(8)?,
        trust_score: row.get(9)?,
        success_count: row.get(10)?,
        failure_count: row.get(11)?,
        execution_count: row.get(12)?,
        avg_duration_ms: row.get(13)?,
        is_active: row.get::<i32>(14)? != 0,
        is_deprecated: row.get::<i32>(15)? != 0,
        metadata: row.get(16)?,
        user_id: row.get(17)?,
        created_at: row.get(18)?,
        updated_at: row.get(19)?,
    })
}

// -- CRUD --

pub async fn create_skill(db: &Database, req: CreateSkillRequest) -> Result<Skill> {
    let conn = db.connection();
    let user_id = req
        .user_id
        .ok_or_else(|| crate::EngError::InvalidInput("user_id required".into()))?;
    let language = req.language.unwrap_or_else(|| "javascript".into());

    // Determine version and root. Parent must belong to the same tenant.
    let (version, root_skill_id) = if let Some(parent_id) = req.parent_skill_id {
        let mut rows = conn
            .query(
                "SELECT version, root_skill_id FROM skill_records \
                 WHERE id = ?1 AND user_id = ?2",
                params![parent_id, user_id],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            let pv: i32 = row.get(0)?;
            let pr: Option<i64> = row.get(1)?;
            (pv + 1, pr.or(Some(parent_id)))
        } else {
            return Err(EngError::NotFound(format!(
                "parent skill {} not found",
                parent_id
            )));
        }
    } else {
        (1, None)
    };

    let mut id_rows = conn
        .query(
            "INSERT INTO skill_records (name, agent, description, code, language, version, \
             parent_skill_id, root_skill_id, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) RETURNING id",
            params![
                req.name,
                req.agent,
                req.description,
                req.code,
                language,
                version,
                req.parent_skill_id,
                root_skill_id,
                user_id
            ],
        )
        .await?;
    let id: i64 = if let Some(row) = id_rows.next().await? {
        row.get(0)?
    } else {
        return Err(EngError::Internal("failed to insert skill".into()));
    };

    // Record lineage
    if let Some(parent_id) = req.parent_skill_id {
        conn.execute(
            "INSERT OR IGNORE INTO skill_lineage_parents (skill_id, parent_id) VALUES (?1, ?2)",
            params![id, parent_id],
        )
        .await?;
    }

    // Insert tags
    if let Some(ref tags) = req.tags {
        for tag in tags {
            conn.execute(
                "INSERT OR IGNORE INTO skill_tags (skill_id, tag) VALUES (?1, ?2)",
                params![id, tag.as_str()],
            )
            .await?;
        }
    }

    // Insert tool deps
    if let Some(ref deps) = req.tool_deps {
        for dep in deps {
            conn.execute(
                "INSERT OR IGNORE INTO skill_tool_deps (skill_id, tool_name, is_optional) VALUES (?1, ?2, 0)",
                params![id, dep.as_str()],
            ).await?;
        }
    }

    get_skill(db, id, user_id).await
}

pub async fn get_skill(db: &Database, id: i64, user_id: i64) -> Result<Skill> {
    let conn = db.connection();
    let sql = format!(
        "SELECT {} FROM skill_records WHERE id = ?1 AND user_id = ?2",
        SKILL_COLUMNS
    );
    let mut rows = conn.query(&sql, params![id, user_id]).await?;
    rows.next()
        .await?
        .map(|row| row_to_skill(&row))
        .transpose()?
        .ok_or_else(|| EngError::NotFound(format!("skill {} not found", id)))
}

/// Hard upper bound on how many skill rows `list_skills` will ever return in
/// a single call, regardless of what the caller asks for. Route handlers
/// apply their own clamp, but library consumers (other modules, tests,
/// scripts) must not be trusted to pass sensible values; a bug or a compromised
/// caller could otherwise OOM the process by loading every row.
pub const MAX_SKILLS_LIMIT: usize = 500;

pub async fn list_skills(
    db: &Database,
    user_id: i64,
    agent: Option<&str>,
    limit: usize,
    offset: usize,
) -> Result<Vec<Skill>> {
    let limit = limit.clamp(1, MAX_SKILLS_LIMIT);
    let conn = db.connection();
    let (sql, has_agent) = if agent.is_some() {
        (format!(
            "SELECT {} FROM skill_records WHERE user_id = ?1 AND agent = ?2 AND is_active = 1 ORDER BY trust_score DESC LIMIT ?3 OFFSET ?4",
            SKILL_COLUMNS
        ), true)
    } else {
        (format!(
            "SELECT {} FROM skill_records WHERE user_id = ?1 AND is_active = 1 ORDER BY trust_score DESC LIMIT ?2 OFFSET ?3",
            SKILL_COLUMNS
        ), false)
    };

    let mut rows = if has_agent {
        conn.query(
            &sql,
            params![user_id, agent.unwrap_or(""), limit as i64, offset as i64],
        )
        .await?
    } else {
        conn.query(&sql, params![user_id, limit as i64, offset as i64])
            .await?
    };

    let mut skills = Vec::new();
    while let Some(row) = rows.next().await? {
        skills.push(row_to_skill(&row)?);
    }
    Ok(skills)
}

pub async fn update_skill(
    db: &Database,
    id: i64,
    req: UpdateSkillRequest,
    user_id: i64,
) -> Result<Skill> {
    let conn = db.connection();
    // Verify ownership
    get_skill(db, id, user_id).await?;

    if let Some(ref code) = req.code {
        conn.execute("UPDATE skill_records SET code = ?1, updated_at = datetime('now') WHERE id = ?2 AND user_id = ?3", params![code.as_str(), id, user_id]).await?;
    }
    if let Some(ref desc) = req.description {
        conn.execute("UPDATE skill_records SET description = ?1, updated_at = datetime('now') WHERE id = ?2 AND user_id = ?3", params![desc.as_str(), id, user_id]).await?;
    }
    if let Some(active) = req.is_active {
        conn.execute("UPDATE skill_records SET is_active = ?1, updated_at = datetime('now') WHERE id = ?2 AND user_id = ?3", params![active as i32, id, user_id]).await?;
    }
    if let Some(deprecated) = req.is_deprecated {
        conn.execute("UPDATE skill_records SET is_deprecated = ?1, updated_at = datetime('now') WHERE id = ?2 AND user_id = ?3", params![deprecated as i32, id, user_id]).await?;
    }
    if let Some(ref meta) = req.metadata {
        conn.execute("UPDATE skill_records SET metadata = ?1, updated_at = datetime('now') WHERE id = ?2 AND user_id = ?3", params![meta.as_str(), id, user_id]).await?;
    }

    get_skill(db, id, user_id).await
}

pub async fn delete_skill(db: &Database, id: i64, user_id: i64) -> Result<()> {
    let conn = db.connection();
    let affected = conn
        .execute(
            "DELETE FROM skill_records WHERE id = ?1 AND user_id = ?2",
            params![id, user_id],
        )
        .await?;
    if affected == 0 {
        return Err(EngError::NotFound(format!("skill {} not found", id)));
    }
    Ok(())
}

// -- Execution recording --

pub async fn record_execution(
    db: &Database,
    skill_id: i64,
    user_id: i64,
    success: bool,
    duration_ms: Option<f64>,
    error_type: Option<&str>,
    error_message: Option<&str>,
) -> Result<()> {
    let conn = db.connection();

    // Fail closed if the skill does not belong to this tenant.
    get_skill(db, skill_id, user_id).await?;

    conn.execute(
        "INSERT INTO execution_analyses (skill_id, success, duration_ms, error_type, error_message) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![skill_id, success as i32, duration_ms, error_type, error_message],
    ).await?;

    // Update skill counters. Scope UPDATEs by user_id as defense in depth.
    if success {
        conn.execute(
            "UPDATE skill_records SET success_count = success_count + 1, \
             execution_count = execution_count + 1, updated_at = datetime('now') \
             WHERE id = ?1 AND user_id = ?2",
            params![skill_id, user_id],
        )
        .await?;
    } else {
        conn.execute(
            "UPDATE skill_records SET failure_count = failure_count + 1, \
             execution_count = execution_count + 1, updated_at = datetime('now') \
             WHERE id = ?1 AND user_id = ?2",
            params![skill_id, user_id],
        )
        .await?;
    }

    // Update avg_duration_ms
    if let Some(dur) = duration_ms {
        conn.execute(
            "UPDATE skill_records SET avg_duration_ms = \
             COALESCE((avg_duration_ms * (execution_count - 1) + ?1) / execution_count, ?1), \
             updated_at = datetime('now') WHERE id = ?2 AND user_id = ?3",
            params![dur, skill_id, user_id],
        )
        .await?;
    }

    Ok(())
}

/// Get execution history for a skill.
pub async fn get_executions(
    db: &Database,
    skill_id: i64,
    user_id: i64,
    limit: usize,
) -> Result<Vec<ExecutionRecord>> {
    let conn = db.connection();
    // Fail closed if the skill does not belong to this tenant.
    get_skill(db, skill_id, user_id).await?;
    let mut rows = conn
        .query(
            "SELECT ea.id, ea.skill_id, ea.success, ea.duration_ms, ea.error_type, ea.error_message, \
         ea.input_hash, ea.output_hash, ea.metadata, ea.created_at \
         FROM execution_analyses ea \
         INNER JOIN skill_records sr ON sr.id = ea.skill_id \
         WHERE ea.skill_id = ?1 AND sr.user_id = ?2 \
         ORDER BY ea.id DESC LIMIT ?3",
            params![skill_id, user_id, limit as i64],
        )
        .await?;

    let mut records = Vec::new();
    while let Some(row) = rows.next().await? {
        records.push(ExecutionRecord {
            id: row.get(0)?,
            skill_id: row.get(1)?,
            success: row.get::<i32>(2)? != 0,
            duration_ms: row.get(3)?,
            error_type: row.get(4)?,
            error_message: row.get(5)?,
            input_hash: row.get(6)?,
            output_hash: row.get(7)?,
            metadata: row.get(8)?,
            created_at: row.get(9)?,
        });
    }
    Ok(records)
}

// -- Judgments --

pub async fn add_judgment(
    db: &Database,
    skill_id: i64,
    user_id: i64,
    judge_agent: &str,
    score: f64,
    rationale: Option<&str>,
) -> Result<SkillJudgment> {
    let conn = db.connection();
    // Fail closed if the skill does not belong to this tenant.
    get_skill(db, skill_id, user_id).await?;

    let mut rows = conn
        .query(
            "INSERT INTO skill_judgments (skill_id, judge_agent, score, rationale) \
             VALUES (?1, ?2, ?3, ?4) RETURNING id",
            params![skill_id, judge_agent, score, rationale],
        )
        .await?;
    let id: i64 = if let Some(row) = rows.next().await? {
        row.get(0)?
    } else {
        return Err(EngError::Internal("failed to insert judgment".into()));
    };

    // Update trust_score as weighted average of all judgments. Scope update by user_id.
    conn.execute(
        "UPDATE skill_records SET trust_score = \
         (SELECT AVG(score) FROM skill_judgments WHERE skill_id = ?1), \
         updated_at = datetime('now') WHERE id = ?1 AND user_id = ?2",
        params![skill_id, user_id],
    )
    .await?;

    Ok(SkillJudgment {
        id,
        skill_id,
        judge_agent: judge_agent.into(),
        score,
        rationale: rationale.map(|s| s.into()),
        created_at: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    })
}

pub async fn get_judgments(
    db: &Database,
    skill_id: i64,
    user_id: i64,
) -> Result<Vec<SkillJudgment>> {
    let conn = db.connection();
    get_skill(db, skill_id, user_id).await?;
    let mut rows = conn
        .query(
            "SELECT sj.id, sj.skill_id, sj.judge_agent, sj.score, sj.rationale, sj.created_at \
         FROM skill_judgments sj \
         INNER JOIN skill_records sr ON sr.id = sj.skill_id \
         WHERE sj.skill_id = ?1 AND sr.user_id = ?2 \
         ORDER BY sj.id DESC",
            params![skill_id, user_id],
        )
        .await?;

    let mut judgments = Vec::new();
    while let Some(row) = rows.next().await? {
        judgments.push(SkillJudgment {
            id: row.get(0)?,
            skill_id: row.get(1)?,
            judge_agent: row.get(2)?,
            score: row.get(3)?,
            rationale: row.get(4)?,
            created_at: row.get(5)?,
        });
    }
    Ok(judgments)
}

// -- Tool quality --

pub async fn record_tool_quality(
    db: &Database,
    tool_name: &str,
    agent: &str,
    success: bool,
    latency_ms: Option<f64>,
    error_type: Option<&str>,
) -> Result<()> {
    let conn = db.connection();
    conn.execute(
        "INSERT INTO tool_quality_records (tool_name, agent, success, latency_ms, error_type) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![tool_name, agent, success as i32, latency_ms, error_type],
    )
    .await?;
    Ok(())
}

pub async fn get_tool_quality(db: &Database, tool_name: &str) -> Result<serde_json::Value> {
    let conn = db.connection();
    let mut rows = conn
        .query(
            "SELECT COUNT(*) as total, SUM(CASE WHEN success THEN 1 ELSE 0 END) as successes, \
         AVG(latency_ms) as avg_latency \
         FROM tool_quality_records WHERE tool_name = ?1",
            params![tool_name],
        )
        .await?;

    if let Some(row) = rows.next().await? {
        let total: i64 = row.get(0)?;
        let successes: i64 = row.get::<Option<i64>>(1)?.unwrap_or(0);
        let avg_latency: Option<f64> = row.get(2)?;
        Ok(serde_json::json!({
            "tool_name": tool_name,
            "total_executions": total,
            "success_count": successes,
            "success_rate": if total > 0 { successes as f64 / total as f64 } else { 0.0 },
            "avg_latency_ms": avg_latency,
        }))
    } else {
        Ok(serde_json::json!({ "tool_name": tool_name, "total_executions": 0 }))
    }
}

// -- Skill tags --

pub async fn get_skill_tags(
    db: &Database,
    skill_id: i64,
    user_id: i64,
) -> Result<Vec<String>> {
    let conn = db.connection();
    get_skill(db, skill_id, user_id).await?;
    let mut rows = conn
        .query(
            "SELECT st.tag FROM skill_tags st \
             INNER JOIN skill_records sr ON sr.id = st.skill_id \
             WHERE st.skill_id = ?1 AND sr.user_id = ?2",
            params![skill_id, user_id],
        )
        .await?;
    let mut tags = Vec::new();
    while let Some(row) = rows.next().await? {
        tags.push(row.get::<String>(0)?);
    }
    Ok(tags)
}

// -- Tool deps --

pub async fn get_tool_deps(
    db: &Database,
    skill_id: i64,
    user_id: i64,
) -> Result<Vec<String>> {
    let conn = db.connection();
    get_skill(db, skill_id, user_id).await?;
    let mut rows = conn
        .query(
            "SELECT std.tool_name FROM skill_tool_deps std \
             INNER JOIN skill_records sr ON sr.id = std.skill_id \
             WHERE std.skill_id = ?1 AND sr.user_id = ?2",
            params![skill_id, user_id],
        )
        .await?;
    let mut deps = Vec::new();
    while let Some(row) = rows.next().await? {
        deps.push(row.get::<String>(0)?);
    }
    Ok(deps)
}

/// Check if all required tools for a skill are available.
pub fn check_tool_safety(required_tools: &[String], available_tools: &[String]) -> bool {
    required_tools.iter().all(|t| available_tools.contains(t))
}

// -- Skill lineage --

pub async fn get_lineage(db: &Database, skill_id: i64, user_id: i64) -> Result<Vec<i64>> {
    let conn = db.connection();
    get_skill(db, skill_id, user_id).await?;
    // Only return parents that also belong to the caller; filter out any foreign-tenant ids
    // even if the lineage table ever held one from a pre-patch row.
    let mut rows = conn
        .query(
            "SELECT slp.parent_id FROM skill_lineage_parents slp \
             INNER JOIN skill_records psr ON psr.id = slp.parent_id \
             WHERE slp.skill_id = ?1 AND psr.user_id = ?2",
            params![skill_id, user_id],
        )
        .await?;
    let mut parents = Vec::new();
    while let Some(row) = rows.next().await? {
        parents.push(row.get::<i64>(0)?);
    }
    Ok(parents)
}
