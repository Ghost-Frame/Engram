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
use rusqlite::params;
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

pub(crate) fn row_to_skill(row: &rusqlite::Row<'_>) -> rusqlite::Result<Skill> {
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
        is_active: row.get::<_, i32>(14)? != 0,
        is_deprecated: row.get::<_, i32>(15)? != 0,
        metadata: row.get(16)?,
        user_id: row.get(17)?,
        created_at: row.get(18)?,
        updated_at: row.get(19)?,
    })
}

// -- CRUD --

pub async fn create_skill(db: &Database, req: CreateSkillRequest) -> Result<Skill> {
    let user_id = req
        .user_id
        .ok_or_else(|| crate::EngError::InvalidInput("user_id required".into()))?;
    let language = req.language.unwrap_or_else(|| "javascript".into());

    // Determine version and root. Parent must belong to the same tenant.
    let (version, root_skill_id) = if let Some(parent_id) = req.parent_skill_id {
        let result = db
            .read(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT version, root_skill_id FROM skill_records \
                     WHERE id = ?1 AND user_id = ?2",
                )?;
                let mut rows = stmt.query(params![parent_id, user_id])?;
                if let Some(row) = rows.next()? {
                    let pv: i32 = row.get(0)?;
                    let pr: Option<i64> = row.get(1)?;
                    Ok(Some((pv, pr)))
                } else {
                    Ok(None)
                }
            })
            .await
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        if let Some((pv, pr)) = result {
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

    let name = req.name.clone();
    let agent = req.agent.clone();
    let description = req.description.clone();
    let code = req.code.clone();
    let language_clone = language.clone();
    let parent_skill_id = req.parent_skill_id;
    let tags = req.tags.clone();
    let tool_deps = req.tool_deps.clone();

    let id = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO skill_records (name, agent, description, code, language, version, \
                 parent_skill_id, root_skill_id, user_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    name,
                    agent,
                    description,
                    code,
                    language_clone,
                    version,
                    parent_skill_id,
                    root_skill_id,
                    user_id
                ],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            let id = conn.last_insert_rowid();

            // Record lineage
            if let Some(parent_id) = parent_skill_id {
                conn.execute(
                    "INSERT OR IGNORE INTO skill_lineage_parents (skill_id, parent_id) VALUES (?1, ?2)",
                    params![id, parent_id],
                )
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            }

            // Insert tags
            if let Some(ref t) = tags {
                for tag in t {
                    conn.execute(
                        "INSERT OR IGNORE INTO skill_tags (skill_id, tag) VALUES (?1, ?2)",
                        params![id, tag],
                    )
                    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
                }
            }

            // Insert tool deps
            if let Some(ref deps) = tool_deps {
                for dep in deps {
                    conn.execute(
                        "INSERT OR IGNORE INTO skill_tool_deps (skill_id, tool_name, is_optional) VALUES (?1, ?2, 0)",
                        params![id, dep],
                    )
                    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
                }
            }

            Ok(id)
        })
        .await?;

    get_skill(db, id, user_id).await
}

pub async fn get_skill(db: &Database, id: i64, user_id: i64) -> Result<Skill> {
    let sql = format!(
        "SELECT {} FROM skill_records WHERE id = ?1 AND user_id = ?2",
        SKILL_COLUMNS
    );
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        let mut rows = stmt
            .query(params![id, user_id])
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        rows.next()
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
            .map(|row| row_to_skill(row).map_err(|e| EngError::DatabaseMessage(e.to_string())))
            .transpose()?
            .ok_or_else(|| EngError::NotFound(format!("skill {} not found", id)))
    })
    .await
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
    let agent_owned = agent.map(|s| s.to_string());

    db.read(move |conn| {
        let mut skills = Vec::new();
        if let Some(ref agent_str) = agent_owned {
            let sql = format!(
                "SELECT {} FROM skill_records WHERE user_id = ?1 AND agent = ?2 AND is_active = 1 ORDER BY trust_score DESC LIMIT ?3 OFFSET ?4",
                SKILL_COLUMNS
            );
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            let mut rows = stmt
                .query(params![user_id, agent_str, limit as i64, offset as i64])
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            while let Some(row) = rows.next().map_err(|e| EngError::DatabaseMessage(e.to_string()))? {
                skills.push(row_to_skill(row).map_err(|e| EngError::DatabaseMessage(e.to_string()))?);
            }
        } else {
            let sql = format!(
                "SELECT {} FROM skill_records WHERE user_id = ?1 AND is_active = 1 ORDER BY trust_score DESC LIMIT ?2 OFFSET ?3",
                SKILL_COLUMNS
            );
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            let mut rows = stmt
                .query(params![user_id, limit as i64, offset as i64])
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            while let Some(row) = rows.next().map_err(|e| EngError::DatabaseMessage(e.to_string()))? {
                skills.push(row_to_skill(row).map_err(|e| EngError::DatabaseMessage(e.to_string()))?);
            }
        };
        Ok(skills)
    })
    .await
}

pub async fn update_skill(
    db: &Database,
    id: i64,
    req: UpdateSkillRequest,
    user_id: i64,
) -> Result<Skill> {
    // Verify ownership
    get_skill(db, id, user_id).await?;

    let code = req.code.clone();
    let desc = req.description.clone();
    let is_active = req.is_active;
    let is_deprecated = req.is_deprecated;
    let meta = req.metadata.clone();

    db.write(move |conn| {
        if let Some(ref c) = code {
            conn.execute(
                "UPDATE skill_records SET code = ?1, updated_at = datetime('now') WHERE id = ?2 AND user_id = ?3",
                params![c, id, user_id],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        }
        if let Some(ref d) = desc {
            conn.execute(
                "UPDATE skill_records SET description = ?1, updated_at = datetime('now') WHERE id = ?2 AND user_id = ?3",
                params![d, id, user_id],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        }
        if let Some(active) = is_active {
            conn.execute(
                "UPDATE skill_records SET is_active = ?1, updated_at = datetime('now') WHERE id = ?2 AND user_id = ?3",
                params![active as i32, id, user_id],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        }
        if let Some(deprecated) = is_deprecated {
            conn.execute(
                "UPDATE skill_records SET is_deprecated = ?1, updated_at = datetime('now') WHERE id = ?2 AND user_id = ?3",
                params![deprecated as i32, id, user_id],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        }
        if let Some(ref m) = meta {
            conn.execute(
                "UPDATE skill_records SET metadata = ?1, updated_at = datetime('now') WHERE id = ?2 AND user_id = ?3",
                params![m, id, user_id],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        }
        Ok(())
    })
    .await?;

    get_skill(db, id, user_id).await
}

pub async fn delete_skill(db: &Database, id: i64, user_id: i64) -> Result<()> {
    let affected = db
        .write(move |conn| {
            conn.execute(
                "DELETE FROM skill_records WHERE id = ?1 AND user_id = ?2",
                params![id, user_id],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))
        })
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
    // Fail closed if the skill does not belong to this tenant.
    get_skill(db, skill_id, user_id).await?;

    let error_type_owned = error_type.map(|s| s.to_string());
    let error_message_owned = error_message.map(|s| s.to_string());

    db.write(move |conn| {
        conn.execute(
            "INSERT INTO execution_analyses (skill_id, success, duration_ms, error_type, error_message) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![skill_id, success as i32, duration_ms, error_type_owned, error_message_owned],
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        // Update skill counters. Scope UPDATEs by user_id as defense in depth.
        if success {
            conn.execute(
                "UPDATE skill_records SET success_count = success_count + 1, \
                 execution_count = execution_count + 1, updated_at = datetime('now') \
                 WHERE id = ?1 AND user_id = ?2",
                params![skill_id, user_id],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        } else {
            conn.execute(
                "UPDATE skill_records SET failure_count = failure_count + 1, \
                 execution_count = execution_count + 1, updated_at = datetime('now') \
                 WHERE id = ?1 AND user_id = ?2",
                params![skill_id, user_id],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        }

        // Update avg_duration_ms
        if let Some(dur) = duration_ms {
            conn.execute(
                "UPDATE skill_records SET avg_duration_ms = \
                 COALESCE((avg_duration_ms * (execution_count - 1) + ?1) / execution_count, ?1), \
                 updated_at = datetime('now') WHERE id = ?2 AND user_id = ?3",
                params![dur, skill_id, user_id],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        }

        Ok(())
    })
    .await
}

/// Get execution history for a skill.
pub async fn get_executions(
    db: &Database,
    skill_id: i64,
    user_id: i64,
    limit: usize,
) -> Result<Vec<ExecutionRecord>> {
    // Fail closed if the skill does not belong to this tenant.
    get_skill(db, skill_id, user_id).await?;

    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT ea.id, ea.skill_id, ea.success, ea.duration_ms, ea.error_type, ea.error_message, \
                 ea.input_hash, ea.output_hash, ea.metadata, ea.created_at \
                 FROM execution_analyses ea \
                 INNER JOIN skill_records sr ON sr.id = ea.skill_id \
                 WHERE ea.skill_id = ?1 AND sr.user_id = ?2 \
                 ORDER BY ea.id DESC LIMIT ?3",
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        let records = stmt
            .query_map(params![skill_id, user_id, limit as i64], |row| {
                Ok(ExecutionRecord {
                    id: row.get(0)?,
                    skill_id: row.get(1)?,
                    success: row.get::<_, i32>(2)? != 0,
                    duration_ms: row.get(3)?,
                    error_type: row.get(4)?,
                    error_message: row.get(5)?,
                    input_hash: row.get(6)?,
                    output_hash: row.get(7)?,
                    metadata: row.get(8)?,
                    created_at: row.get(9)?,
                })
            })
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        Ok(records)
    })
    .await
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
    // Fail closed if the skill does not belong to this tenant.
    get_skill(db, skill_id, user_id).await?;

    let judge_agent_owned = judge_agent.to_string();
    let rationale_owned = rationale.map(|s| s.to_string());

    let id = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO skill_judgments (skill_id, judge_agent, score, rationale) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![skill_id, judge_agent_owned, score, rationale_owned],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            let id = conn.last_insert_rowid();

            // Update trust_score as weighted average of all judgments. Scope update by user_id.
            conn.execute(
                "UPDATE skill_records SET trust_score = \
                 (SELECT AVG(score) FROM skill_judgments WHERE skill_id = ?1), \
                 updated_at = datetime('now') WHERE id = ?1 AND user_id = ?2",
                params![skill_id, user_id],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

            Ok(id)
        })
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
    get_skill(db, skill_id, user_id).await?;

    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT sj.id, sj.skill_id, sj.judge_agent, sj.score, sj.rationale, sj.created_at \
                 FROM skill_judgments sj \
                 INNER JOIN skill_records sr ON sr.id = sj.skill_id \
                 WHERE sj.skill_id = ?1 AND sr.user_id = ?2 \
                 ORDER BY sj.id DESC",
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        let judgments = stmt
            .query_map(params![skill_id, user_id], |row| {
                Ok(SkillJudgment {
                    id: row.get(0)?,
                    skill_id: row.get(1)?,
                    judge_agent: row.get(2)?,
                    score: row.get(3)?,
                    rationale: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        Ok(judgments)
    })
    .await
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
    let tool_name_owned = tool_name.to_string();
    let agent_owned = agent.to_string();
    let error_type_owned = error_type.map(|s| s.to_string());

    db.write(move |conn| {
        conn.execute(
            "INSERT INTO tool_quality_records (tool_name, agent, success, latency_ms, error_type) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![tool_name_owned, agent_owned, success as i32, latency_ms, error_type_owned],
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        Ok(())
    })
    .await
}

pub async fn get_tool_quality(db: &Database, tool_name: &str) -> Result<serde_json::Value> {
    let tool_name_owned = tool_name.to_string();

    let result = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT COUNT(*) as total, SUM(CASE WHEN success THEN 1 ELSE 0 END) as successes, \
                     AVG(latency_ms) as avg_latency \
                     FROM tool_quality_records WHERE tool_name = ?1",
                )
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            let mut rows = stmt
                .query(params![tool_name_owned])
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            if let Some(row) = rows.next().map_err(|e| EngError::DatabaseMessage(e.to_string()))? {
                let total: i64 =
                    row.get(0).map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
                let successes: i64 = row
                    .get::<_, Option<i64>>(1)
                    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
                    .unwrap_or(0);
                let avg_latency: Option<f64> =
                    row.get(2).map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
                Ok(Some((total, successes, avg_latency)))
            } else {
                Ok(None)
            }
        })
        .await?;

    if let Some((total, successes, avg_latency)) = result {
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

pub async fn get_skill_tags(db: &Database, skill_id: i64, user_id: i64) -> Result<Vec<String>> {
    get_skill(db, skill_id, user_id).await?;

    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT st.tag FROM skill_tags st \
                 INNER JOIN skill_records sr ON sr.id = st.skill_id \
                 WHERE st.skill_id = ?1 AND sr.user_id = ?2",
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        let tags = stmt
            .query_map(params![skill_id, user_id], |row| row.get(0))
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
            .collect::<rusqlite::Result<Vec<String>>>()
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        Ok(tags)
    })
    .await
}

// -- Tool deps --

pub async fn get_tool_deps(db: &Database, skill_id: i64, user_id: i64) -> Result<Vec<String>> {
    get_skill(db, skill_id, user_id).await?;

    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT std.tool_name FROM skill_tool_deps std \
                 INNER JOIN skill_records sr ON sr.id = std.skill_id \
                 WHERE std.skill_id = ?1 AND sr.user_id = ?2",
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        let deps = stmt
            .query_map(params![skill_id, user_id], |row| row.get(0))
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
            .collect::<rusqlite::Result<Vec<String>>>()
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        Ok(deps)
    })
    .await
}

/// Check if all required tools for a skill are available.
pub fn check_tool_safety(required_tools: &[String], available_tools: &[String]) -> bool {
    required_tools.iter().all(|t| available_tools.contains(t))
}

// -- Skill lineage --

pub async fn get_lineage(db: &Database, skill_id: i64, user_id: i64) -> Result<Vec<i64>> {
    get_skill(db, skill_id, user_id).await?;
    // Only return parents that also belong to the caller; filter out any foreign-tenant ids
    // even if the lineage table ever held one from a pre-patch row.
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT slp.parent_id FROM skill_lineage_parents slp \
                 INNER JOIN skill_records psr ON psr.id = slp.parent_id \
                 WHERE slp.skill_id = ?1 AND psr.user_id = ?2",
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        let parents = stmt
            .query_map(params![skill_id, user_id], |row| row.get(0))
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
            .collect::<rusqlite::Result<Vec<i64>>>()
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        Ok(parents)
    })
    .await
}
