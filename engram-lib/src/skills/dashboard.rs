use crate::db::Database;
use crate::{EngError, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

/// Compute a weighted skill score.
/// 50% completion rate, 30% applied rate, 20% recency.
pub fn compute_skill_score(
    success_count: i32,
    failure_count: i32,
    execution_count: i32,
    days_since_last: f64,
) -> f64 {
    let total = success_count + failure_count;
    let completion_rate = if total > 0 {
        success_count as f64 / total as f64
    } else {
        0.0
    };
    let applied_rate = if execution_count > 0 {
        total as f64 / execution_count as f64
    } else {
        0.0
    };
    let recency = 1.0 / (1.0 + days_since_last / 30.0);
    completion_rate * 0.5 + applied_rate * 0.3 + recency * 0.2
}

/// Calculate days since a datetime string.
pub fn days_since(datetime_str: &str) -> f64 {
    chrono::NaiveDateTime::parse_from_str(datetime_str, "%Y-%m-%d %H:%M:%S")
        .map(|dt| {
            let now = chrono::Utc::now().naive_utc();
            (now - dt).num_seconds() as f64 / 86400.0
        })
        .unwrap_or(999.0)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillOverview {
    pub total_skills: i64,
    pub active_skills: i64,
    pub deprecated_skills: i64,
    pub total_executions: i64,
    pub avg_trust_score: f64,
}

/// Get dashboard overview stats.
#[tracing::instrument(skip(db), fields(user_id))]
pub async fn get_overview(db: &Database, user_id: i64) -> Result<SkillOverview> {
    db.read(move |conn| {
        conn.query_row(
            "SELECT COUNT(*), SUM(CASE WHEN is_active = 1 THEN 1 ELSE 0 END), SUM(CASE WHEN is_deprecated = 1 THEN 1 ELSE 0 END), SUM(execution_count), AVG(trust_score) FROM skill_records WHERE user_id = ?1",
            params![user_id],
            |row| {
                Ok(SkillOverview {
                    total_skills: row.get(0)?,
                    active_skills: row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                    deprecated_skills: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    total_executions: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                    avg_trust_score: row.get::<_, Option<f64>>(4)?.unwrap_or(0.0),
                })
            },
        )
        .optional()
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
        .ok_or_else(|| EngError::Internal("no result from overview query".into()))
    }).await.or_else(|_| Ok(SkillOverview {
        total_skills: 0,
        active_skills: 0,
        deprecated_skills: 0,
        total_executions: 0,
        avg_trust_score: 0.0,
    }))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillStats {
    pub id: i64,
    pub name: String,
    pub execution_count: i32,
    pub success_count: i32,
    pub failure_count: i32,
    pub trust_score: f64,
    pub computed_score: f64,
}

/// Get stats for all active skills.
#[tracing::instrument(skip(db), fields(user_id, sort_by = ?sort_by, limit))]
pub async fn get_skill_stats(
    db: &Database,
    user_id: i64,
    sort_by: Option<&str>,
    limit: usize,
) -> Result<Vec<SkillStats>> {
    let order = match sort_by {
        Some("trust") => "trust_score DESC",
        Some("executions") => "execution_count DESC",
        Some("name") => "name ASC",
        _ => "trust_score DESC",
    };
    let sql = format!("SELECT id, name, execution_count, success_count, failure_count, trust_score, updated_at FROM skill_records WHERE user_id = ?1 AND is_active = 1 ORDER BY {} LIMIT ?2", order);
    let limit = limit as i64;

    db.read(move |conn| {
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        let rows = stmt
            .query_map(params![user_id, limit], |row| {
                let updated: String = row.get(6)?;
                let ec: i32 = row.get(2)?;
                let sc: i32 = row.get(3)?;
                let fc: i32 = row.get(4)?;
                let ds = days_since(&updated);
                Ok(SkillStats {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    execution_count: ec,
                    success_count: sc,
                    failure_count: fc,
                    trust_score: row.get(5)?,
                    computed_score: compute_skill_score(sc, fc, ec, ds),
                })
            })
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))
    })
    .await
}

/// Get detailed info for a single skill.
#[tracing::instrument(skip(db), fields(skill_id))]
pub async fn get_skill_detail(db: &Database, skill_id: i64) -> Result<serde_json::Value> {
    db.read(move |conn| {
        let row = conn.query_row(
            "SELECT id, name, agent, description, trust_score, execution_count, success_count, failure_count, avg_duration_ms, version, created_at, updated_at FROM skill_records WHERE id = ?1",
            params![skill_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, f64>(4)?,
                    row.get::<_, i32>(5)?,
                    row.get::<_, i32>(6)?,
                    row.get::<_, i32>(7)?,
                    row.get::<_, Option<f64>>(8)?,
                    row.get::<_, i32>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, String>(11)?,
                ))
            },
        )
        .optional()
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
        .ok_or_else(|| EngError::NotFound(format!("skill {} not found", skill_id)))?;

        let (id, name, agent, description, trust_score, ec, sc, fc, avg_duration_ms, version, created_at, updated) = row;

        // Get tags
        let mut stmt = conn.prepare("SELECT tag FROM skill_tags WHERE skill_id = ?1")
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        let tags: Vec<String> = stmt.query_map(params![skill_id], |row| row.get(0))
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        // Get parents
        let mut stmt = conn.prepare("SELECT parent_id FROM skill_lineage_parents WHERE skill_id = ?1")
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        let parents: Vec<i64> = stmt.query_map(params![skill_id], |row| row.get(0))
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        // Get recent executions
        let mut stmt = conn.prepare(
            "SELECT id, success, duration_ms, error_type, created_at FROM execution_analyses WHERE skill_id = ?1 ORDER BY id DESC LIMIT 10"
        ).map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        let executions: Vec<serde_json::Value> = stmt.query_map(params![skill_id], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, i64>(0)?,
                "success": row.get::<_, i32>(1)? != 0,
                "duration_ms": row.get::<_, Option<f64>>(2)?,
                "error_type": row.get::<_, Option<String>>(3)?,
                "created_at": row.get::<_, String>(4)?,
            }))
        })
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
        .filter_map(|r| r.ok())
        .collect();

        Ok(serde_json::json!({
            "id": id,
            "name": name,
            "agent": agent,
            "description": description,
            "trust_score": trust_score,
            "execution_count": ec,
            "success_count": sc,
            "failure_count": fc,
            "avg_duration_ms": avg_duration_ms,
            "version": version,
            "created_at": created_at,
            "updated_at": &updated,
            "computed_score": compute_skill_score(sc, fc, ec, days_since(&updated)),
            "tags": tags,
            "parent_ids": parents,
            "recent_executions": executions,
        }))
    }).await
}

/// Health check for the skills subsystem.
#[tracing::instrument(skip(db))]
pub async fn health_check(db: &Database) -> Result<serde_json::Value> {
    db.read(move |conn| {
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM skill_records", [], |row| row.get(0))
            .unwrap_or(0);
        Ok(serde_json::json!({ "status": "ok", "skills_count": count }))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_score_perfect() {
        assert!(compute_skill_score(10, 0, 10, 0.0) > 0.9);
    }
    #[test]
    fn test_score_zero() {
        assert!(compute_skill_score(0, 0, 0, 999.0) < 0.01);
    }
    #[test]
    fn test_score_mixed() {
        let s = compute_skill_score(5, 5, 10, 7.0);
        assert!(s > 0.0 && s < 1.0);
    }
    #[test]
    fn test_days_since_recent() {
        let now = chrono::Utc::now().naive_utc();
        let s = now.format("%Y-%m-%d %H:%M:%S").to_string();
        assert!(days_since(&s) < 1.0);
    }
    #[test]
    fn test_days_since_invalid() {
        assert_eq!(days_since("not-a-date"), 999.0);
    }
}
