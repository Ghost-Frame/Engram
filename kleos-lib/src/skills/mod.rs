//! Skills domain -- versioned, composable agent workflows backed by the
//! `skills` table.
//!
//! A skill bundles a name, prompt, and metadata; submodules add behavior:
//! - [`search`]    keyword + semantic search across skills.
//! - [`analyzer`]  structural analysis passes (size, graph shape).
//! - [`evolver`]   controlled mutation + evaluation loops.
//! - [`dashboard`] aggregate stats/health for UI surfaces.
//! - [`cloud`]     import/export to shared cloud skill libraries.
//!
//! Routes under `/skills/*` in `kleos-server` dispatch into these.
//! Skills read memories and write derived rows; they must not mutate raw
//! `memories` outside their own tables.

pub mod aliases;
pub mod analyzer;
pub mod bundles;
pub mod cloud;
pub mod dashboard;
pub mod evolver;
pub mod materializations;
pub mod search;
pub mod types;

use crate::db::Database;
use crate::{EngError, Result};
use rusqlite::params;

pub use types::{
    CreateSkillRequest, EvolutionFeedRow, ExecutionRecord, Skill, SkillJudgment, SkillKind,
    ToolQuality, UpdateSkillRequest,
};

// -- Constants --

/// Canonical SELECT column list for `skill_records`; order must match `row_to_skill` indices.
/// user_id is at index 23 (zero-based), appended after the v50 cloud columns.
pub(crate) const SKILL_COLUMNS: &str = "id, name, agent, description, code, language, version, \
    parent_skill_id, root_skill_id, trust_score, success_count, failure_count, \
    execution_count, avg_duration_ms, is_active, is_deprecated, metadata, \
    created_at, updated_at, \
    kind, source_plugin, source_path, content_hash, user_id";

// -- Helpers --

/// Maps a `SELECT SKILL_COLUMNS` row into a `Skill` struct; trailing v50 columns may be NULL.
/// Column indices must match SKILL_COLUMNS order: 0..18 core, 19..22 cloud, 23 user_id.
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
        // Index 23: user_id column restored by migration 78 (monolith) / v69 (tenant).
        user_id: row.get(23)?,
        created_at: row.get(17)?,
        updated_at: row.get(18)?,
        kind: row
            .get::<_, Option<String>>(19)?
            .unwrap_or_else(|| "skill".to_string()),
        source_plugin: row.get(20)?,
        source_path: row.get(21)?,
        content_hash: row.get(22)?,
    })
}

// -- CRUD --

/// Creates a new skill record and returns the persisted skill.
#[tracing::instrument(skip(db, req), fields(name = %req.name, agent = %req.agent))]
pub async fn create_skill(db: &Database, req: CreateSkillRequest) -> Result<Skill> {
    let user_id = req
        .user_id
        .ok_or_else(|| crate::EngError::InvalidInput("user_id required".into()))?;
    let language = req.language.unwrap_or_else(|| "javascript".into());

    // Determine version and root. Parent must belong to the same tenant.
    let (version, root_skill_id) = if let Some(parent_id) = req.parent_skill_id {
        let result = db
            .read(move |conn| {
                // Scope the parent lookup to this tenant. Without the user_id predicate a
                // caller could name another tenant's skill as parent, leaking its
                // version/root metadata and forging a cross-tenant lineage link.
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
            .await?;
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
    let kind = req.kind.clone().unwrap_or_else(|| "skill".to_string());
    kind.parse::<SkillKind>()
        .map_err(|_| EngError::InvalidInput(format!("invalid skill kind: '{}'", kind)))?;
    let source_plugin = req.source_plugin.clone();
    let source_path = req.source_path.clone();
    let content_hash = req.content_hash.clone();

    let id = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO skill_records (name, agent, description, code, language, version, \
                 parent_skill_id, root_skill_id, kind, source_plugin, source_path, content_hash, \
                 user_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    name,
                    agent,
                    description,
                    code,
                    language_clone,
                    version,
                    parent_skill_id,
                    root_skill_id,
                    kind,
                    source_plugin,
                    source_path,
                    content_hash,
                    user_id,
                ],
            )?;
            let id = conn.last_insert_rowid();

            // Record lineage
            if let Some(parent_id) = parent_skill_id {
                conn.execute(
                    "INSERT OR IGNORE INTO skill_lineage_parents (skill_id, parent_id) VALUES (?1, ?2)",
                    params![id, parent_id],
                )?;
            }

            // Insert tags
            if let Some(ref t) = tags {
                for tag in t {
                    conn.execute(
                        "INSERT OR IGNORE INTO skill_tags (skill_id, tag) VALUES (?1, ?2)",
                        params![id, tag],
                    )?;
                }
            }

            // Insert tool deps
            if let Some(ref deps) = tool_deps {
                for dep in deps {
                    conn.execute(
                        "INSERT OR IGNORE INTO skill_tool_deps (skill_id, tool_name, is_optional) VALUES (?1, ?2, 0)",
                        params![id, dep],
                    )?;
                }
            }

            Ok(id)
        })
        .await?;

    get_skill(db, id, user_id).await
}

/// Fetches a single skill by id scoped to user_id; returns `NotFound` if absent
/// or if the skill belongs to a different user.
#[tracing::instrument(skip(db))]
pub async fn get_skill(db: &Database, id: i64, user_id: i64) -> Result<Skill> {
    let sql = format!(
        "SELECT {} FROM skill_records WHERE id = ?1 AND user_id = ?2",
        SKILL_COLUMNS
    );
    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(params![id, user_id])?;
        rows.next()?
            .map(|row| row_to_skill(row))
            .transpose()?
            .ok_or_else(|| EngError::NotFound(format!("skill {} not found", id)))
    })
    .await
}

pub use crate::validation::MAX_SKILLS_LIMIT;

/// Lists active skills, optionally filtered by agent, with pagination.
#[tracing::instrument(skip(db))]
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
                "SELECT {} FROM skill_records \
                 WHERE agent = ?1 AND user_id = ?2 AND is_active = 1 \
                 ORDER BY trust_score DESC LIMIT ?3 OFFSET ?4",
                SKILL_COLUMNS
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(params![agent_str, user_id, limit as i64, offset as i64])?;
            while let Some(row) = rows.next()? {
                skills.push(row_to_skill(row)?);
            }
        } else {
            let sql = format!(
                "SELECT {} FROM skill_records \
                 WHERE user_id = ?1 AND is_active = 1 \
                 ORDER BY trust_score DESC LIMIT ?2 OFFSET ?3",
                SKILL_COLUMNS
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(params![user_id, limit as i64, offset as i64])?;
            while let Some(row) = rows.next()? {
                skills.push(row_to_skill(row)?);
            }
        };
        Ok(skills)
    })
    .await
}

/// Applies partial updates to an existing skill and returns the refreshed record.
#[tracing::instrument(skip(db, req))]
pub async fn update_skill(
    db: &Database,
    id: i64,
    req: UpdateSkillRequest,
    user_id: i64,
) -> Result<Skill> {
    // Verify ownership
    get_skill(db, id, user_id).await?;

    if let Some(ref k) = req.kind {
        k.parse::<SkillKind>()
            .map_err(|_| EngError::InvalidInput(format!("invalid skill kind: '{}'", k)))?;
    }

    let code = req.code.clone();
    let desc = req.description.clone();
    let is_active = req.is_active;
    let is_deprecated = req.is_deprecated;
    let meta = req.metadata.clone();
    let kind = req.kind.clone();
    let source_path = req.source_path.clone();
    let content_hash = req.content_hash.clone();

    db.write(move |conn| {
        if let Some(ref c) = code {
            conn.execute(
                "UPDATE skill_records SET code = ?1, updated_at = datetime('now') WHERE id = ?2",
                params![c, id],
            )?;
        }
        if let Some(ref d) = desc {
            conn.execute(
                "UPDATE skill_records SET description = ?1, updated_at = datetime('now') WHERE id = ?2",
                params![d, id],
            )?;
        }
        if let Some(active) = is_active {
            conn.execute(
                "UPDATE skill_records SET is_active = ?1, updated_at = datetime('now') WHERE id = ?2",
                params![active as i32, id],
            )?;
        }
        if let Some(deprecated) = is_deprecated {
            conn.execute(
                "UPDATE skill_records SET is_deprecated = ?1, updated_at = datetime('now') WHERE id = ?2",
                params![deprecated as i32, id],
            )?;
        }
        if let Some(ref m) = meta {
            conn.execute(
                "UPDATE skill_records SET metadata = ?1, updated_at = datetime('now') WHERE id = ?2",
                params![m, id],
            )?;
        }
        if let Some(ref k) = kind {
            conn.execute(
                "UPDATE skill_records SET kind = ?1, updated_at = datetime('now') WHERE id = ?2",
                params![k, id],
            )?;
        }
        if let Some(ref sp) = source_path {
            conn.execute(
                "UPDATE skill_records SET source_path = ?1, updated_at = datetime('now') WHERE id = ?2",
                params![sp, id],
            )?;
        }
        if let Some(ref ch) = content_hash {
            conn.execute(
                "UPDATE skill_records SET content_hash = ?1, updated_at = datetime('now') WHERE id = ?2",
                params![ch, id],
            )?;
        }
        Ok(())
    })
    .await?;

    get_skill(db, id, user_id).await
}

/// Bump a skill's version and clear running counters so analyzers and
/// evolvers re-evaluate it from scratch. Returns the refreshed skill.
///
/// Recompute is per-tenant; a caller cannot recompute another user's
/// skill. Execution history rows (`execution_analyses`) are kept for
/// audit; only the rolled-up counters on `skill_records` are reset.
#[tracing::instrument(skip(db))]
pub async fn recompute_skill(db: &Database, id: i64, user_id: i64) -> Result<Skill> {
    get_skill(db, id, user_id).await?;

    let affected = db
        .write(move |conn| {
            Ok(conn.execute(
                "UPDATE skill_records SET \
                    version = version + 1, \
                    success_count = 0, \
                    failure_count = 0, \
                    execution_count = 0, \
                    avg_duration_ms = NULL, \
                    duration_sample_count = 0, \
                    trust_score = 0.0, \
                    updated_at = datetime('now') \
                 WHERE id = ?1",
                params![id],
            )?)
        })
        .await?;
    if affected == 0 {
        return Err(EngError::NotFound(format!("skill {} not found", id)));
    }
    get_skill(db, id, user_id).await
}

/// Permanently deletes a skill record scoped to the calling user; returns `NotFound`
/// if absent or owned by a different user.
#[tracing::instrument(skip(db))]
pub async fn delete_skill(db: &Database, id: i64, user_id: i64) -> Result<()> {
    let affected = db
        .write(move |conn| {
            Ok(conn.execute(
                "DELETE FROM skill_records WHERE id = ?1 AND user_id = ?2",
                params![id, user_id],
            )?)
        })
        .await?;
    if affected == 0 {
        return Err(EngError::NotFound(format!("skill {} not found", id)));
    }
    Ok(())
}

// -- Execution recording --

/// Records a single skill execution outcome and updates rolled-up counters.
#[tracing::instrument(skip(db, error_message))]
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
        )?;

        // Update skill counters.
        if success {
            conn.execute(
                "UPDATE skill_records SET success_count = success_count + 1, \
                 execution_count = execution_count + 1, updated_at = datetime('now') \
                 WHERE id = ?1",
                params![skill_id],
            )?;
        } else {
            conn.execute(
                "UPDATE skill_records SET failure_count = failure_count + 1, \
                 execution_count = execution_count + 1, updated_at = datetime('now') \
                 WHERE id = ?1",
                params![skill_id],
            )?;
        }

        // Update avg_duration_ms. Finding [51]: the running average divides by
        // duration_sample_count (executions that actually reported a duration),
        // not execution_count -- otherwise duration-less executions inflate the
        // denominator and drag the average toward zero. The sample counter is
        // incremented in the same statement so numerator and denominator move
        // together.
        if let Some(dur) = duration_ms {
            conn.execute(
                "UPDATE skill_records SET \
                 avg_duration_ms = COALESCE( \
                     (avg_duration_ms * duration_sample_count + ?1) / (duration_sample_count + 1), \
                     ?1), \
                 duration_sample_count = duration_sample_count + 1, \
                 updated_at = datetime('now') WHERE id = ?2",
                params![dur, skill_id],
            )?;
        }

        Ok(())
    })
    .await
}

/// Get execution history for a skill.
#[tracing::instrument(skip(db))]
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
                 WHERE ea.skill_id = ?1 \
                 ORDER BY ea.id DESC LIMIT ?2",
            )?;

        let records = stmt
            .query_map(params![skill_id, limit as i64], |row| {
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
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(records)
    })
    .await
}

// -- Judgments --

/// Records a judgment score and updates the skill's trust_score average.
#[tracing::instrument(skip(db, rationale), fields(judge_agent = %judge_agent))]
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
            )?;
            let id = conn.last_insert_rowid();

            // Update trust_score as weighted average of all judgments.
            conn.execute(
                "UPDATE skill_records SET trust_score = \
                 (SELECT AVG(score) FROM skill_judgments WHERE skill_id = ?1), \
                 updated_at = datetime('now') WHERE id = ?1",
                params![skill_id],
            )?;

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

/// Returns all judgments for a skill ordered by most recent first.
#[tracing::instrument(skip(db))]
pub async fn get_judgments(
    db: &Database,
    skill_id: i64,
    user_id: i64,
) -> Result<Vec<SkillJudgment>> {
    get_skill(db, skill_id, user_id).await?;

    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT sj.id, sj.skill_id, sj.judge_agent, sj.score, sj.rationale, sj.created_at \
                 FROM skill_judgments sj \
                 WHERE sj.skill_id = ?1 \
                 ORDER BY sj.id DESC",
        )?;

        let judgments = stmt
            .query_map(params![skill_id], |row| {
                Ok(SkillJudgment {
                    id: row.get(0)?,
                    skill_id: row.get(1)?,
                    judge_agent: row.get(2)?,
                    score: row.get(3)?,
                    rationale: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(judgments)
    })
    .await
}

// -- Tool quality --

/// Appends a tool quality observation to `tool_quality_records`.
#[tracing::instrument(skip(db), fields(tool_name = %tool_name, agent = %agent))]
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
            params![
                tool_name_owned,
                agent_owned,
                success as i32,
                latency_ms,
                error_type_owned
            ],
        )?;
        Ok(())
    })
    .await
}

/// Returns aggregate quality stats for a tool as a JSON value.
#[tracing::instrument(skip(db), fields(tool_name = %tool_name))]
pub async fn get_tool_quality(db: &Database, tool_name: &str) -> Result<serde_json::Value> {
    let tool_name_owned = tool_name.to_string();

    let result = db
        .read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT COUNT(*) as total, SUM(CASE WHEN success THEN 1 ELSE 0 END) as successes, \
                     AVG(latency_ms) as avg_latency \
                     FROM tool_quality_records WHERE tool_name = ?1",
            )?;
            let mut rows = stmt.query(params![tool_name_owned])?;
            if let Some(row) = rows.next()? {
                let total: i64 = row.get(0)?;
                let successes: i64 = row.get::<_, Option<i64>>(1)?.unwrap_or(0);
                let avg_latency: Option<f64> = row.get(2)?;
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

/// Returns all tags associated with a skill.
#[tracing::instrument(skip(db))]
pub async fn get_skill_tags(db: &Database, skill_id: i64, user_id: i64) -> Result<Vec<String>> {
    get_skill(db, skill_id, user_id).await?;

    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT st.tag FROM skill_tags st \
                 WHERE st.skill_id = ?1",
        )?;
        let tags = stmt
            .query_map(params![skill_id], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(tags)
    })
    .await
}

// -- Tool deps --

/// Returns all tool dependency names declared by a skill.
#[tracing::instrument(skip(db))]
pub async fn get_tool_deps(db: &Database, skill_id: i64, user_id: i64) -> Result<Vec<String>> {
    get_skill(db, skill_id, user_id).await?;

    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT std.tool_name FROM skill_tool_deps std \
                 WHERE std.skill_id = ?1",
        )?;
        let deps = stmt
            .query_map(params![skill_id], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(deps)
    })
    .await
}

/// List recently-evolved skills for a user. An evolution is any
/// `skill_records` row that carries a `skill_tags` entry of
/// `fixed` | `derived` | `captured`. Parent ids come from
/// `skill_lineage_parents` (empty for captured skills).
#[tracing::instrument(skip(db), fields(user_id, since_hours, limit))]
pub async fn list_recent_evolutions(
    db: &Database,
    user_id: i64,
    since_hours: u32,
    limit: usize,
) -> Result<Vec<EvolutionFeedRow>> {
    let since_clause = format!("-{} hours", since_hours as i64);
    db.read(move |conn| {
        // Scope to the caller's skills so single-DB (shared) mode does not
        // surface another user's evolution feed; a no-op in a per-user shard.
        let sql = "SELECT sr.id, sr.name, sr.version, st.tag, sr.agent, sr.created_at \
                   FROM skill_records sr \
                   INNER JOIN skill_tags st ON st.skill_id = sr.id \
                   WHERE st.tag IN ('fixed', 'derived', 'captured') \
                     AND sr.created_at > datetime('now', ?1) \
                     AND sr.user_id = ?3 \
                   ORDER BY sr.created_at DESC \
                   LIMIT ?2";
        let mut stmt = conn.prepare(sql)?;
        let raw: Vec<(i64, String, i32, String, String, String)> = stmt
            .query_map(params![since_clause, limit as i64, user_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i32>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .collect();

        let mut parents_stmt = conn.prepare(
            "SELECT parent_id FROM skill_lineage_parents \
                 WHERE skill_id = ?1 ORDER BY parent_id",
        )?;

        let mut out = Vec::with_capacity(raw.len());
        for (skill_id, name, version, tag, agent, created_at) in raw {
            let parent_ids: Vec<i64> = parents_stmt
                .query_map(params![skill_id], |row| row.get::<_, i64>(0))?
                .filter_map(|r| r.ok())
                .collect();
            out.push(EvolutionFeedRow {
                skill_id,
                name,
                version,
                origin: tag,
                parent_ids,
                agent,
                created_at,
            });
        }
        Ok(out)
    })
    .await
}

// -- Skill lineage --

/// Returns parent skill ids from the lineage table for a given skill.
#[tracing::instrument(skip(db))]
pub async fn get_lineage(db: &Database, skill_id: i64, user_id: i64) -> Result<Vec<i64>> {
    get_skill(db, skill_id, user_id).await?;
    // Only return parents that also belong to the caller. skill_lineage_parents
    // has no user_id column, so ownership is enforced by joining skill_records
    // on parent_id and filtering by user_id -- this actually performs the
    // foreign-tenant filtering the prior comment only claimed.
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT slp.parent_id FROM skill_lineage_parents slp \
                 JOIN skill_records sr ON sr.id = slp.parent_id \
                 WHERE slp.skill_id = ?1 AND sr.user_id = ?2",
        )?;
        let parents = stmt
            .query_map(params![skill_id, user_id], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        Ok(parents)
    })
    .await
}
