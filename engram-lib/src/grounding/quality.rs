// GROUNDING QUALITY - Tool quality tracking (ported from TS grounding/quality.ts)
use crate::Result;
use libsql::Connection;

const DEFAULT_DEGRADATION_THRESHOLD: f64 = 0.7;

pub struct ToolQualityManager { degradation_threshold: f64 }

impl ToolQualityManager {
    pub fn new(threshold: Option<f64>) -> Self { Self { degradation_threshold: threshold.unwrap_or(DEFAULT_DEGRADATION_THRESHOLD) } }

    pub async fn record_execution(&self, conn: &Connection, tool_name: &str, agent: &str, success: bool, latency_ms: f64, error_type: Option<&str>) -> Result<()> {
        conn.execute("INSERT INTO tool_quality_records (tool_name, agent, success, latency_ms, error_type) VALUES (?1, ?2, ?3, ?4, ?5)",
            libsql::params![tool_name.to_string(), agent.to_string(), success, latency_ms, error_type.map(|s| s.to_string())]).await?;
        Ok(())
    }

    pub async fn get_quality_score(&self, conn: &Connection, tool_name: &str) -> Result<f64> {
        let mut rows = conn.query("SELECT COUNT(*) as total, SUM(CASE WHEN success THEN 1 ELSE 0 END) as successes FROM tool_quality_records WHERE tool_name = ?1", libsql::params![tool_name.to_string()]).await?;
        match rows.next().await? {
            Some(r) => { let total: i64 = r.get(0)?; let succ: i64 = r.get(1)?; if total == 0 { Ok(1.0) } else { Ok(succ as f64 / total as f64) } }
            None => Ok(1.0)
        }
    }

    pub async fn get_degraded_tools(&self, conn: &Connection) -> Result<Vec<(String, f64)>> {
        let mut rows = conn.query("SELECT tool_name, CAST(SUM(CASE WHEN success THEN 1 ELSE 0 END) AS REAL) / COUNT(*) as score FROM tool_quality_records GROUP BY tool_name HAVING score < ?1", libsql::params![self.degradation_threshold]).await?;
        let mut tools = Vec::new();
        while let Some(r) = rows.next().await? { tools.push((r.get::<String>(0)?, r.get::<f64>(1)?)); }
        Ok(tools)
    }

    pub fn adjust_tool_ranking(&self, tool_scores: &mut [(String, f64)]) { tool_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)); }
}
