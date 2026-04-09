// GROUNDING QUALITY - Tool quality tracking (ported from TS grounding/quality.ts)
use crate::Result;
use libsql::Connection;
use super::types::{build_tool_key, parse_tool_key, ToolQualityRecord};

const DEFAULT_DEGRADATION_THRESHOLD: f64 = 0.7;

pub struct ToolQualityManager { degradation_threshold: f64 }

impl ToolQualityManager {
    pub fn new(threshold: Option<f64>) -> Self { Self { degradation_threshold: threshold.unwrap_or(DEFAULT_DEGRADATION_THRESHOLD) } }

    pub async fn record_execution(&self, conn: &Connection, tool_key: &str, success: bool, latency_ms: f64, _error_message: Option<&str>) -> Result<()> {
        let parsed = parse_tool_key(tool_key);
        let normalized_key = build_tool_key(&parsed.backend, &parsed.server, &parsed.tool_name);
        conn.execute(
            "INSERT INTO tool_quality_records (
                tool_key, backend, server, tool_name, description_hash, total_calls,
                total_successes, total_failures, avg_execution_ms, quality_score,
                last_execution_at, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, '', 1, ?5, ?6, ?7, ?8, datetime('now'), datetime('now'), datetime('now'))
            ON CONFLICT(tool_key) DO UPDATE SET
                total_calls = tool_quality_records.total_calls + 1,
                total_successes = tool_quality_records.total_successes + ?5,
                total_failures = tool_quality_records.total_failures + ?6,
                avg_execution_ms = ((tool_quality_records.avg_execution_ms * tool_quality_records.total_calls) + ?7) / (tool_quality_records.total_calls + 1),
                quality_score = CAST(tool_quality_records.total_successes + ?5 AS REAL) / (tool_quality_records.total_calls + 1),
                last_execution_at = datetime('now'),
                updated_at = datetime('now')",
            libsql::params![
                normalized_key,
                parsed.backend,
                parsed.server,
                parsed.tool_name,
                if success { 1 } else { 0 },
                if success { 0 } else { 1 },
                latency_ms,
                if success { 1.0 } else { 0.0 },
            ],
        ).await?;
        Ok(())
    }

    pub async fn get_quality_score(&self, conn: &Connection, tool_name: &str) -> Result<f64> {
        let mut rows = conn.query("SELECT quality_score FROM tool_quality_records WHERE tool_key = ?1 OR tool_name = ?1 ORDER BY updated_at DESC LIMIT 1", libsql::params![tool_name.to_string()]).await?;
        match rows.next().await? {
            Some(r) => Ok(r.get(0)?),
            None => Ok(1.0)
        }
    }

    pub async fn get_degraded_tools(&self, conn: &Connection) -> Result<Vec<(String, f64)>> {
        let mut rows = conn.query("SELECT tool_name, quality_score FROM tool_quality_records WHERE quality_score < ?1 ORDER BY quality_score ASC", libsql::params![self.degradation_threshold]).await?;
        let mut tools = Vec::new();
        while let Some(r) = rows.next().await? { tools.push((r.get::<String>(0)?, r.get::<f64>(1)?)); }
        Ok(tools)
    }

    pub async fn get_all_records(&self, conn: &Connection, limit: i64) -> Result<Vec<ToolQualityRecord>> {
        let mut rows = conn.query(
            "SELECT tool_key, backend, server, tool_name, description_hash, total_calls, total_successes, total_failures, avg_execution_ms, llm_flagged_count, quality_score, last_execution_at
             FROM tool_quality_records
             ORDER BY total_calls DESC, updated_at DESC
             LIMIT ?1",
            libsql::params![limit],
        ).await?;
        let mut records = Vec::new();
        while let Some(r) = rows.next().await? {
            records.push(ToolQualityRecord {
                tool_key: r.get(0)?,
                backend: r.get(1)?,
                server: r.get(2)?,
                tool_name: r.get(3)?,
                description_hash: r.get(4)?,
                total_calls: r.get(5)?,
                total_successes: r.get(6)?,
                total_failures: r.get(7)?,
                avg_execution_ms: r.get(8)?,
                llm_flagged_count: r.get(9)?,
                quality_score: r.get(10)?,
                last_execution_at: r.get(11)?,
            });
        }
        Ok(records)
    }

    pub fn adjust_tool_ranking(&self, tool_scores: &mut [(String, f64)]) { tool_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)); }
}
