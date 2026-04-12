// GROUNDING QUALITY - Tool quality tracking (ported from TS grounding/quality.ts)
use super::types::{build_tool_key, parse_tool_key, ToolQualityRecord};
use crate::db::Database;
use crate::{EngError, Result};
use rusqlite::params;

const DEFAULT_DEGRADATION_THRESHOLD: f64 = 0.7;

pub struct ToolQualityManager {
    degradation_threshold: f64,
}

impl ToolQualityManager {
    pub fn new(threshold: Option<f64>) -> Self {
        Self {
            degradation_threshold: threshold.unwrap_or(DEFAULT_DEGRADATION_THRESHOLD),
        }
    }

    pub async fn record_execution(
        &self,
        db: &Database,
        tool_key: &str,
        success: bool,
        latency_ms: f64,
        _error_message: Option<&str>,
    ) -> Result<()> {
        let parsed = parse_tool_key(tool_key);
        let normalized_key = build_tool_key(&parsed.backend, &parsed.server, &parsed.tool_name);
        let backend = parsed.backend.clone();
        let server = parsed.server.clone();
        let tool_name = parsed.tool_name.clone();
        db.write(move |conn| {
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
                params![
                    normalized_key,
                    backend,
                    server,
                    tool_name,
                    if success { 1 } else { 0 },
                    if success { 0 } else { 1 },
                    latency_ms,
                    if success { 1.0 } else { 0.0 },
                ],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            Ok(())
        })
        .await
    }

    pub async fn get_quality_score(&self, db: &Database, tool_name: &str) -> Result<f64> {
        let tool_name_owned = tool_name.to_string();
        db.read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT quality_score FROM tool_quality_records \
                     WHERE tool_key = ?1 OR tool_name = ?1 ORDER BY updated_at DESC LIMIT 1",
                )
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            let mut rows = stmt
                .query_map(params![tool_name_owned], |row| row.get::<_, f64>(0))
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            match rows.next() {
                Some(r) => Ok(r.map_err(|e| EngError::DatabaseMessage(e.to_string()))?),
                None => Ok(1.0),
            }
        })
        .await
    }

    pub async fn get_degraded_tools(&self, db: &Database) -> Result<Vec<(String, f64)>> {
        let threshold = self.degradation_threshold;
        db.read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT tool_name, quality_score FROM tool_quality_records \
                     WHERE quality_score < ?1 ORDER BY quality_score ASC",
                )
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            let rows = stmt
                .query_map(params![threshold], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
                })
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))
        })
        .await
    }

    pub async fn get_all_records(
        &self,
        db: &Database,
        limit: i64,
    ) -> Result<Vec<ToolQualityRecord>> {
        db.read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT tool_key, backend, server, tool_name, description_hash, total_calls, \
                     total_successes, total_failures, avg_execution_ms, llm_flagged_count, \
                     quality_score, last_execution_at \
                     FROM tool_quality_records \
                     ORDER BY total_calls DESC, updated_at DESC \
                     LIMIT ?1",
                )
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            let rows = stmt
                .query_map(params![limit], |row| {
                    Ok(ToolQualityRecord {
                        tool_key: row.get(0)?,
                        backend: row.get(1)?,
                        server: row.get(2)?,
                        tool_name: row.get(3)?,
                        description_hash: row.get(4)?,
                        total_calls: row.get(5)?,
                        total_successes: row.get(6)?,
                        total_failures: row.get(7)?,
                        avg_execution_ms: row.get(8)?,
                        llm_flagged_count: row.get(9)?,
                        quality_score: row.get(10)?,
                        last_execution_at: row.get(11)?,
                    })
                })
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))
        })
        .await
    }

    pub fn adjust_tool_ranking(&self, tool_scores: &mut [(String, f64)]) {
        tool_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    }
}
