use crate::db::Database;
use crate::Result;
use libsql::params;
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
    let completion_rate = if total > 0 { success_count as f64 / total as f64 } else { 0.0 };
    let applied_rate = if execution_count > 0 { total as f64 / execution_count as f64 } else { 0.0 };
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
pub async fn get_overview(db: &Database, user_id: i64) -> Result<SkillOverview> {
    let conn = db.connection();
    let mut rows = conn.query(
        "SELECT COUNT(*), SUM(CASE WHEN is_active = 1 THEN 1 ELSE 0 END), SUM(CASE WHEN is_deprecated = 1 THEN 1 ELSE 0 END), SUM(execution_count), AVG(trust_score) FROM skill_records WHERE user_id = ?1",
        params![user_id],
    ).await?;
    if let Some(row) = rows.next().await? {
        Ok(SkillOverview {
            total_skills: row.get::<i64>(0)?,
            active_skills: row.get::<Option<i64>>(1)?.unwrap_or(0),
            deprecated_skills: row.get::<Option<i64>>(2)?.unwrap_or(0),
            total_executions: row.get::<Option<i64>>(3)?.unwrap_or(0),
            avg_trust_score: row.get::<Option<f64>>(4)?.unwrap_or(0.0),
        })
    } else {
        Ok(SkillOverview { total_skills: 0, active_skills: 0, deprecated_skills: 0, total_executions: 0, avg_trust_score: 0.0 })
    }
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
pub async fn get_skill_stats(db: &Database, user_id: i64, sort_by: Option<&str>, limit: usize) -> Result<Vec<SkillStats>> {
    let conn = db.connection();
    let order = match sort_by {
        Some("trust") => "trust_score DESC",
        Some("executions") => "execution_count DESC",
        Some("name") => "name ASC",
        _ => "trust_score DESC",
    };
    let sql = format!("SELECT id, name, execution_count, success_count, failure_count, trust_score, updated_at FROM skill_records WHERE user_id = ?1 AND is_active = 1 ORDER BY {} LIMIT ?2", order);
    let mut rows = conn.query(&sql, params![user_id, limit as i64]).await?;
    let mut stats = Vec::new();
    while let Some(row) = rows.next().await? {
        let updated: String = row.get(6)?;
        let ec: i32 = row.get(2)?;
        let sc: i32 = row.get(3)?;
        let fc: i32 = row.get(4)?;
        let ds = days_since(&updated);
        stats.push(SkillStats {
            id: row.get(0)?,
            name: row.get(1)?,
            execution_count: ec,
            success_count: sc,
            failure_count: fc,
            trust_score: row.get(5)?,
            computed_score: compute_skill_score(sc, fc, ec, ds),
        });
    }
    Ok(stats)
}
/// Get detailed info for a single skill.
pub async fn get_skill_detail(db: &Database, skill_id: i64) -> Result<serde_json::Value> {
    let conn = db.connection();
    let mut rows = conn.query(
        "SELECT id, name, agent, description, trust_score, execution_count, success_count, failure_count, avg_duration_ms, version, created_at, updated_at FROM skill_records WHERE id = ?1",
        params![skill_id],
    ).await?;
    let row = rows.next().await?
        .ok_or_else(|| crate::EngError::NotFound(format!("skill {} not found", skill_id)))?;

    let updated: String = row.get(11)?;
    let ec: i32 = row.get(5)?;
    let sc: i32 = row.get(6)?;
    let fc: i32 = row.get(7)?;

    let mut tag_rows = conn.query("SELECT tag FROM skill_tags WHERE skill_id = ?1", params![skill_id]).await?;
    let mut tags = Vec::new();
    while let Some(tr) = tag_rows.next().await? { tags.push(tr.get::<String>(0)?); }

    let mut lin_rows = conn.query("SELECT parent_id FROM skill_lineage_parents WHERE skill_id = ?1", params![skill_id]).await?;
    let mut parents = Vec::new();
    while let Some(lr) = lin_rows.next().await? { parents.push(lr.get::<i64>(0)?); }

    let mut exec_rows = conn.query(
        "SELECT id, success, duration_ms, error_type, created_at FROM execution_analyses WHERE skill_id = ?1 ORDER BY id DESC LIMIT 10",
        params![skill_id],
    ).await?;
    let mut executions = Vec::new();
    while let Some(er) = exec_rows.next().await? {
        executions.push(serde_json::json!({
            "id": er.get::<i64>(0)?,
            "success": er.get::<i32>(1)? != 0,
            "duration_ms": er.get::<Option<f64>>(2)?,
            "error_type": er.get::<Option<String>>(3)?,
            "created_at": er.get::<String>(4)?,
        }));
    }

    Ok(serde_json::json!({
        "id": row.get::<i64>(0)?,
        "name": row.get::<String>(1)?,
        "agent": row.get::<String>(2)?,
        "description": row.get::<Option<String>>(3)?,
        "trust_score": row.get::<f64>(4)?,
        "execution_count": ec,
        "success_count": sc,
        "failure_count": fc,
        "avg_duration_ms": row.get::<Option<f64>>(8)?,
        "version": row.get::<i32>(9)?,
        "created_at": row.get::<String>(10)?,
        "updated_at": &updated,
        "computed_score": compute_skill_score(sc, fc, ec, days_since(&updated)),
        "tags": tags,
        "parent_ids": parents,
        "recent_executions": executions,
    }))
}

/// Health check for the skills subsystem.
pub async fn health_check(db: &Database) -> Result<serde_json::Value> {
    let conn = db.connection();
    let mut rows = conn.query("SELECT COUNT(*) FROM skill_records", ()).await?;
    let count: i64 = if let Some(row) = rows.next().await? { row.get(0)? } else { 0 };
    Ok(serde_json::json!({ "status": "ok", "skills_count": count }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn test_score_perfect() { assert!(compute_skill_score(10, 0, 10, 0.0) > 0.9); }
    #[test] fn test_score_zero() { assert!(compute_skill_score(0, 0, 0, 999.0) < 0.01); }
    #[test] fn test_score_mixed() {
        let s = compute_skill_score(5, 5, 10, 7.0);
        assert!(s > 0.0 && s < 1.0);
    }
    #[test] fn test_days_since_recent() {
        let now = chrono::Utc::now().naive_utc();
        let s = now.format("%Y-%m-%d %H:%M:%S").to_string();
        assert!(days_since(&s) < 1.0);
    }
    #[test] fn test_days_since_invalid() { assert_eq!(days_since("not-a-date"), 999.0); }
}