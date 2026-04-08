use serde::{Deserialize, Serialize};
use crate::db::Database;
use crate::{EngError, Result};

// ---------------------------------------------------------------------------
// Rubric types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rubric {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub criteria: serde_json::Value, // JSON array of criteria objects
    pub user_id: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRubricRequest {
    pub name: String,
    pub description: Option<String>,
    pub criteria: serde_json::Value,
    pub user_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateRubricRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub criteria: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Evaluation types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evaluation {
    pub id: i64,
    pub rubric_id: i64,
    pub agent: String,
    pub subject: String,
    pub input: serde_json::Value,
    pub output: serde_json::Value,
    pub scores: serde_json::Value,
    pub overall_score: f64,
    pub notes: Option<String>,
    pub evaluator: String,
    pub user_id: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluateRequest {
    pub rubric_id: i64,
    pub agent: String,
    pub subject: String,
    pub input: Option<serde_json::Value>,
    pub output: Option<serde_json::Value>,
    pub scores: serde_json::Value, // { "criterion_name": score_number }
    pub notes: Option<String>,
    pub evaluator: String,
    pub user_id: Option<i64>,
}

// ---------------------------------------------------------------------------
// Metric types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityMetric {
    pub id: i64,
    pub agent: String,
    pub metric: String,
    pub value: f64,
    pub tags: serde_json::Value,
    pub user_id: i64,
    pub recorded_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordMetricRequest {
    pub agent: String,
    pub metric: String,
    pub value: f64,
    pub tags: Option<serde_json::Value>,
    pub user_id: Option<i64>,
}

// ---------------------------------------------------------------------------
// Session quality
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionQuality {
    pub id: i64,
    pub session_id: String,
    pub agent: String,
    pub turn_count: i32,
    pub rules_followed: serde_json::Value,
    pub rules_drifted: serde_json::Value,
    pub personality_score: Option<f64>,
    pub rule_compliance_rate: Option<f64>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordSessionQualityRequest {
    pub session_id: String,
    pub agent: String,
    pub turn_count: Option<i32>,
    pub rules_followed: Option<Vec<String>>,
    pub rules_drifted: Option<Vec<String>>,
    pub personality_score: Option<f64>,
    pub rule_compliance_rate: Option<f64>,
}

// ---------------------------------------------------------------------------
// Drift events
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriftEvent {
    pub id: i64,
    pub agent: String,
    pub session_id: Option<String>,
    pub drift_type: String,
    pub severity: String,
    pub signal: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordDriftEventRequest {
    pub agent: String,
    pub session_id: Option<String>,
    pub drift_type: String,
    pub severity: Option<String>,
    pub signal: String,
}

// ---------------------------------------------------------------------------
// Agent scores summary
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentScores {
    pub agent: String,
    pub overall_avg: f64,
    pub evaluation_count: i64,
    pub by_criterion: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThymusStats {
    pub rubrics: i64,
    pub evaluations: i64,
    pub metrics: i64,
    pub agent_count: i64,
}

// ---------------------------------------------------------------------------
// Drift summary
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriftSummaryEntry {
    pub drift_type: String,
    pub severity: String,
    pub count: i64,
}

// ---------------------------------------------------------------------------
// Row helpers
// ---------------------------------------------------------------------------

fn row_to_rubric(row: &libsql::Row) -> Result<Rubric> {
    let criteria_str: String = row.get(3)?;
    let criteria: serde_json::Value = serde_json::from_str(&criteria_str)
        .unwrap_or(serde_json::Value::Array(vec![]));
    Ok(Rubric {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        criteria,
        user_id: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn row_to_evaluation(row: &libsql::Row) -> Result<Evaluation> {
    let input_str: String = row.get(4)?;
    let output_str: String = row.get(5)?;
    let scores_str: String = row.get(6)?;

    let input: serde_json::Value = serde_json::from_str(&input_str)
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    let output: serde_json::Value = serde_json::from_str(&output_str)
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    let scores: serde_json::Value = serde_json::from_str(&scores_str)
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

    Ok(Evaluation {
        id: row.get(0)?,
        rubric_id: row.get(1)?,
        agent: row.get(2)?,
        subject: row.get(3)?,
        input,
        output,
        scores,
        overall_score: row.get(7)?,
        notes: row.get(8)?,
        evaluator: row.get(9)?,
        user_id: row.get(10)?,
        created_at: row.get(11)?,
    })
}

fn row_to_metric(row: &libsql::Row) -> Result<QualityMetric> {
    let tags_str: String = row.get(4)?;
    let tags: serde_json::Value = serde_json::from_str(&tags_str)
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    Ok(QualityMetric {
        id: row.get(0)?,
        agent: row.get(1)?,
        metric: row.get(2)?,
        value: row.get(3)?,
        tags,
        user_id: row.get(5)?,
        recorded_at: row.get(6)?,
    })
}

fn row_to_session_quality(row: &libsql::Row) -> Result<SessionQuality> {
    let rules_followed_str: String = row.get(4)?;
    let rules_drifted_str: String = row.get(5)?;

    let rules_followed: serde_json::Value = serde_json::from_str(&rules_followed_str)
        .unwrap_or(serde_json::Value::Array(vec![]));
    let rules_drifted: serde_json::Value = serde_json::from_str(&rules_drifted_str)
        .unwrap_or(serde_json::Value::Array(vec![]));

    Ok(SessionQuality {
        id: row.get(0)?,
        session_id: row.get(1)?,
        agent: row.get(2)?,
        turn_count: row.get(3)?,
        rules_followed,
        rules_drifted,
        personality_score: row.get(6)?,
        rule_compliance_rate: row.get(7)?,
        created_at: row.get(8)?,
    })
}

fn row_to_drift_event(row: &libsql::Row) -> Result<DriftEvent> {
    Ok(DriftEvent {
        id: row.get(0)?,
        agent: row.get(1)?,
        session_id: row.get(2)?,
        drift_type: row.get(3)?,
        severity: row.get(4)?,
        signal: row.get(5)?,
        created_at: row.get(6)?,
    })
}

// ---------------------------------------------------------------------------
// Rubric functions
// ---------------------------------------------------------------------------

pub async fn create_rubric(db: &Database, req: CreateRubricRequest) -> Result<Rubric> {
    let conn = &db.conn;
    let user_id = req.user_id.unwrap_or(1);
    let criteria_json = serde_json::to_string(&req.criteria)?;

    conn.execute(
        "INSERT INTO rubrics (name, description, criteria, user_id)
         VALUES (?1, ?2, ?3, ?4)",
        libsql::params![req.name, req.description, criteria_json, user_id],
    )
    .await?;

    let mut rows = conn.query("SELECT last_insert_rowid()", ()).await?;
    let id_row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::Internal("no rowid".into()))?;
    let id: i64 = id_row.get(0)?;

    get_rubric(db, id).await
}

pub async fn get_rubric(db: &Database, id: i64) -> Result<Rubric> {
    let conn = &db.conn;
    let mut rows = conn
        .query(
            "SELECT id, name, description, criteria, user_id, created_at, updated_at
             FROM rubrics WHERE id = ?1",
            libsql::params![id],
        )
        .await?;

    let row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::NotFound(format!("rubric {}", id)))?;

    row_to_rubric(&row)
}

pub async fn list_rubrics(db: &Database, user_id: i64) -> Result<Vec<Rubric>> {
    let conn = &db.conn;
    let mut rows = conn
        .query(
            "SELECT id, name, description, criteria, user_id, created_at, updated_at
             FROM rubrics WHERE user_id = ?1 ORDER BY created_at DESC",
            libsql::params![user_id],
        )
        .await?;

    let mut results = Vec::new();
    while let Some(row) = rows.next().await? {
        results.push(row_to_rubric(&row)?);
    }
    Ok(results)
}

pub async fn update_rubric(db: &Database, id: i64, req: UpdateRubricRequest) -> Result<Rubric> {
    let conn = &db.conn;

    // Verify it exists
    get_rubric(db, id).await?;

    let mut sets: Vec<String> = Vec::new();
    let mut params_vec: Vec<libsql::Value> = Vec::new();
    let mut idx = 1usize;

    if let Some(v) = req.name {
        sets.push(format!("name = ?{}", idx));
        params_vec.push(libsql::Value::Text(v));
        idx += 1;
    }
    if let Some(v) = req.description {
        sets.push(format!("description = ?{}", idx));
        params_vec.push(libsql::Value::Text(v));
        idx += 1;
    }
    if let Some(v) = req.criteria {
        let j = serde_json::to_string(&v)?;
        sets.push(format!("criteria = ?{}", idx));
        params_vec.push(libsql::Value::Text(j));
        idx += 1;
    }

    sets.push("updated_at = datetime('now')".to_string());

    if sets.is_empty() {
        return get_rubric(db, id).await;
    }

    let sql = format!("UPDATE rubrics SET {} WHERE id = ?{}", sets.join(", "), idx);
    params_vec.push(libsql::Value::Integer(id));

    conn.execute(&sql, libsql::params_from_iter(params_vec)).await?;

    get_rubric(db, id).await
}

pub async fn delete_rubric(db: &Database, id: i64, user_id: i64) -> Result<bool> {
    let conn = &db.conn;
    conn.execute(
        "DELETE FROM rubrics WHERE id = ?1 AND user_id = ?2",
        libsql::params![id, user_id],
    )
    .await?;
    Ok(true)
}

// ---------------------------------------------------------------------------
// Evaluation functions
// ---------------------------------------------------------------------------

/// Compute weighted overall score from rubric criteria and provided scores.
/// Each criterion: { name, weight, scale_min, scale_max }
/// normalized = (score - scale_min) / (scale_max - scale_min)
/// overall = sum(normalized * weight) / total_weight
fn compute_weighted_score(
    criteria: &serde_json::Value,
    scores: &serde_json::Value,
) -> Result<f64> {
    let criteria_arr = criteria
        .as_array()
        .ok_or_else(|| EngError::InvalidInput("rubric criteria must be an array".into()))?;

    if criteria_arr.is_empty() {
        return Err(EngError::InvalidInput("rubric has no criteria".into()));
    }

    let scores_obj = scores
        .as_object()
        .ok_or_else(|| EngError::InvalidInput("scores must be a JSON object".into()))?;

    let mut weighted_sum = 0.0f64;
    let mut total_weight = 0.0f64;

    for criterion in criteria_arr {
        let name = criterion
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| EngError::InvalidInput("criterion missing 'name'".into()))?;

        let weight = criterion
            .get("weight")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0);

        let scale_min = criterion
            .get("scale_min")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let scale_max = criterion
            .get("scale_max")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0);

        let raw_score = scores_obj
            .get(name)
            .and_then(|v| v.as_f64())
            .ok_or_else(|| EngError::InvalidInput(format!("missing score for criterion '{}'", name)))?;

        let range = scale_max - scale_min;
        let normalized = if range == 0.0 {
            0.0
        } else {
            (raw_score - scale_min) / range
        };

        weighted_sum += normalized * weight;
        total_weight += weight;
    }

    if total_weight == 0.0 {
        return Err(EngError::InvalidInput("total weight is zero".into()));
    }

    Ok(weighted_sum / total_weight)
}

pub async fn evaluate(db: &Database, req: EvaluateRequest) -> Result<Evaluation> {
    let conn = &db.conn;
    let user_id = req.user_id.unwrap_or(1);

    // Fetch rubric to get criteria
    let rubric = get_rubric(db, req.rubric_id).await?;

    // Compute weighted overall score
    let overall_score = compute_weighted_score(&rubric.criteria, &req.scores)?;

    let input = req.input.unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    let output = req.output.unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    let input_json = serde_json::to_string(&input)?;
    let output_json = serde_json::to_string(&output)?;
    let scores_json = serde_json::to_string(&req.scores)?;

    conn.execute(
        "INSERT INTO evaluations (rubric_id, agent, subject, input, output, scores, overall_score, notes, evaluator, user_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        libsql::params![
            req.rubric_id,
            req.agent,
            req.subject,
            input_json,
            output_json,
            scores_json,
            overall_score,
            req.notes,
            req.evaluator,
            user_id,
        ],
    )
    .await?;

    let mut rows = conn.query("SELECT last_insert_rowid()", ()).await?;
    let id_row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::Internal("no rowid".into()))?;
    let id: i64 = id_row.get(0)?;

    get_evaluation(db, id).await
}

pub async fn get_evaluation(db: &Database, id: i64) -> Result<Evaluation> {
    let conn = &db.conn;
    let mut rows = conn
        .query(
            "SELECT id, rubric_id, agent, subject, input, output, scores, overall_score,
                    notes, evaluator, user_id, created_at
             FROM evaluations WHERE id = ?1",
            libsql::params![id],
        )
        .await?;

    let row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::NotFound(format!("evaluation {}", id)))?;

    row_to_evaluation(&row)
}

pub async fn list_evaluations(
    db: &Database,
    user_id: i64,
    agent: Option<&str>,
    rubric_id: Option<i64>,
    limit: usize,
) -> Result<Vec<Evaluation>> {
    let conn = &db.conn;

    let mut sql = String::from(
        "SELECT id, rubric_id, agent, subject, input, output, scores, overall_score,
                notes, evaluator, user_id, created_at
         FROM evaluations WHERE user_id = ?1",
    );
    let mut params_vec: Vec<libsql::Value> = vec![libsql::Value::Integer(user_id)];
    let mut idx = 2usize;

    if let Some(a) = agent {
        sql.push_str(&format!(" AND agent = ?{}", idx));
        params_vec.push(libsql::Value::Text(a.to_string()));
        idx += 1;
    }
    if let Some(rid) = rubric_id {
        sql.push_str(&format!(" AND rubric_id = ?{}", idx));
        params_vec.push(libsql::Value::Integer(rid));
        idx += 1;
    }

    sql.push_str(&format!(" ORDER BY created_at DESC LIMIT ?{}", idx));
    params_vec.push(libsql::Value::Integer(limit as i64));

    let mut rows = conn.query(&sql, libsql::params_from_iter(params_vec)).await?;
    let mut results = Vec::new();
    while let Some(row) = rows.next().await? {
        results.push(row_to_evaluation(&row)?);
    }
    Ok(results)
}

pub async fn get_agent_scores(
    db: &Database,
    user_id: i64,
    agent: &str,
    rubric_id: Option<i64>,
    since: Option<&str>,
) -> Result<AgentScores> {
    let conn = &db.conn;

    // Build query for overall stats
    let mut sql = String::from(
        "SELECT overall_score, scores FROM evaluations
         WHERE user_id = ?1 AND agent = ?2",
    );
    let mut params_vec: Vec<libsql::Value> = vec![
        libsql::Value::Integer(user_id),
        libsql::Value::Text(agent.to_string()),
    ];
    let mut idx = 3usize;

    if let Some(rid) = rubric_id {
        sql.push_str(&format!(" AND rubric_id = ?{}", idx));
        params_vec.push(libsql::Value::Integer(rid));
        idx += 1;
    }
    if let Some(s) = since {
        sql.push_str(&format!(" AND created_at >= ?{}", idx));
        params_vec.push(libsql::Value::Text(s.to_string()));
    }

    let mut rows = conn.query(&sql, libsql::params_from_iter(params_vec)).await?;

    let mut overall_sum = 0.0f64;
    let mut count = 0i64;
    // Per-criterion accumulators: name -> (sum, min, max, count)
    let mut criterion_stats: std::collections::HashMap<String, (f64, f64, f64, i64)> =
        std::collections::HashMap::new();

    while let Some(row) = rows.next().await? {
        let overall: f64 = row.get(0)?;
        let scores_str: String = row.get(1)?;

        overall_sum += overall;
        count += 1;

        if let Ok(scores_obj) = serde_json::from_str::<serde_json::Value>(&scores_str) {
            if let Some(obj) = scores_obj.as_object() {
                for (k, v) in obj {
                    if let Some(score) = v.as_f64() {
                        let entry = criterion_stats.entry(k.clone()).or_insert((0.0, f64::MAX, f64::MIN, 0));
                        entry.0 += score;
                        entry.1 = entry.1.min(score);
                        entry.2 = entry.2.max(score);
                        entry.3 += 1;
                    }
                }
            }
        }
    }

    let overall_avg = if count == 0 { 0.0 } else { overall_sum / count as f64 };

    // Build by_criterion JSON
    let mut by_criterion = serde_json::Map::new();
    for (k, (sum, min, max, n)) in &criterion_stats {
        let avg = sum / *n as f64;
        by_criterion.insert(
            k.clone(),
            serde_json::json!({
                "avg": avg,
                "min": min,
                "max": max,
                "count": n,
            }),
        );
    }

    Ok(AgentScores {
        agent: agent.to_string(),
        overall_avg,
        evaluation_count: count,
        by_criterion: serde_json::Value::Object(by_criterion),
    })
}

// ---------------------------------------------------------------------------
// Metric functions
// ---------------------------------------------------------------------------

pub async fn record_metric(db: &Database, req: RecordMetricRequest) -> Result<QualityMetric> {
    let conn = &db.conn;
    let user_id = req.user_id.unwrap_or(1);
    let tags = req.tags.unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    let tags_json = serde_json::to_string(&tags)?;

    conn.execute(
        "INSERT INTO quality_metrics (agent, metric, value, tags, user_id)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        libsql::params![req.agent, req.metric, req.value, tags_json, user_id],
    )
    .await?;

    let mut rows = conn.query("SELECT last_insert_rowid()", ()).await?;
    let id_row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::Internal("no rowid".into()))?;
    let id: i64 = id_row.get(0)?;

    let mut metric_rows = conn
        .query(
            "SELECT id, agent, metric, value, tags, user_id, recorded_at
             FROM quality_metrics WHERE id = ?1",
            libsql::params![id],
        )
        .await?;

    let row = metric_rows
        .next()
        .await?
        .ok_or_else(|| EngError::Internal("metric not found after insert".into()))?;

    row_to_metric(&row)
}

pub async fn get_metrics(
    db: &Database,
    user_id: i64,
    agent: Option<&str>,
    metric: Option<&str>,
    since: Option<&str>,
    limit: usize,
) -> Result<Vec<QualityMetric>> {
    let conn = &db.conn;

    let mut sql = String::from(
        "SELECT id, agent, metric, value, tags, user_id, recorded_at
         FROM quality_metrics WHERE user_id = ?1",
    );
    let mut params_vec: Vec<libsql::Value> = vec![libsql::Value::Integer(user_id)];
    let mut idx = 2usize;

    if let Some(a) = agent {
        sql.push_str(&format!(" AND agent = ?{}", idx));
        params_vec.push(libsql::Value::Text(a.to_string()));
        idx += 1;
    }
    if let Some(m) = metric {
        sql.push_str(&format!(" AND metric = ?{}", idx));
        params_vec.push(libsql::Value::Text(m.to_string()));
        idx += 1;
    }
    if let Some(s) = since {
        sql.push_str(&format!(" AND recorded_at >= ?{}", idx));
        params_vec.push(libsql::Value::Text(s.to_string()));
        idx += 1;
    }

    sql.push_str(&format!(" ORDER BY recorded_at DESC LIMIT ?{}", idx));
    params_vec.push(libsql::Value::Integer(limit as i64));

    let mut rows = conn.query(&sql, libsql::params_from_iter(params_vec)).await?;
    let mut results = Vec::new();
    while let Some(row) = rows.next().await? {
        results.push(row_to_metric(&row)?);
    }
    Ok(results)
}

pub async fn get_metric_summary(
    db: &Database,
    user_id: i64,
    agent: &str,
    metric: &str,
    since: Option<&str>,
) -> Result<serde_json::Value> {
    let conn = &db.conn;

    let mut sql = String::from(
        "SELECT AVG(value), MIN(value), MAX(value), COUNT(*)
         FROM quality_metrics WHERE user_id = ?1 AND agent = ?2 AND metric = ?3",
    );
    let mut params_vec: Vec<libsql::Value> = vec![
        libsql::Value::Integer(user_id),
        libsql::Value::Text(agent.to_string()),
        libsql::Value::Text(metric.to_string()),
    ];

    if let Some(s) = since {
        sql.push_str(" AND recorded_at >= ?4");
        params_vec.push(libsql::Value::Text(s.to_string()));
    }

    let mut rows = conn.query(&sql, libsql::params_from_iter(params_vec)).await?;

    let row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::Internal("no summary row".into()))?;

    let avg: Option<f64> = row.get(0)?;
    let min: Option<f64> = row.get(1)?;
    let max: Option<f64> = row.get(2)?;
    let count: i64 = row.get(3)?;

    Ok(serde_json::json!({
        "avg": avg,
        "min": min,
        "max": max,
        "count": count,
        "agent": agent,
        "metric": metric,
    }))
}

// ---------------------------------------------------------------------------
// Session quality functions
// ---------------------------------------------------------------------------

pub async fn record_session_quality(
    db: &Database,
    req: RecordSessionQualityRequest,
) -> Result<SessionQuality> {
    let conn = &db.conn;
    let turn_count = req.turn_count.unwrap_or(0);
    let rules_followed = req.rules_followed.unwrap_or_default();
    let rules_drifted = req.rules_drifted.unwrap_or_default();
    let rules_followed_json = serde_json::to_string(&rules_followed)?;
    let rules_drifted_json = serde_json::to_string(&rules_drifted)?;

    conn.execute(
        "INSERT INTO session_quality (session_id, agent, turn_count, rules_followed, rules_drifted, personality_score, rule_compliance_rate)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        libsql::params![
            req.session_id,
            req.agent,
            turn_count,
            rules_followed_json,
            rules_drifted_json,
            req.personality_score,
            req.rule_compliance_rate,
        ],
    )
    .await?;

    let mut rows = conn.query("SELECT last_insert_rowid()", ()).await?;
    let id_row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::Internal("no rowid".into()))?;
    let id: i64 = id_row.get(0)?;

    let mut sq_rows = conn
        .query(
            "SELECT id, session_id, agent, turn_count, rules_followed, rules_drifted,
                    personality_score, rule_compliance_rate, created_at
             FROM session_quality WHERE id = ?1",
            libsql::params![id],
        )
        .await?;

    let row = sq_rows
        .next()
        .await?
        .ok_or_else(|| EngError::Internal("session_quality not found after insert".into()))?;

    row_to_session_quality(&row)
}

pub async fn get_session_quality(
    db: &Database,
    agent: &str,
    since: Option<&str>,
    limit: usize,
) -> Result<Vec<SessionQuality>> {
    let conn = &db.conn;

    let mut sql = String::from(
        "SELECT id, session_id, agent, turn_count, rules_followed, rules_drifted,
                personality_score, rule_compliance_rate, created_at
         FROM session_quality WHERE agent = ?1",
    );
    let mut params_vec: Vec<libsql::Value> = vec![libsql::Value::Text(agent.to_string())];
    let mut idx = 2usize;

    if let Some(s) = since {
        sql.push_str(&format!(" AND created_at >= ?{}", idx));
        params_vec.push(libsql::Value::Text(s.to_string()));
        idx += 1;
    }

    sql.push_str(&format!(" ORDER BY created_at DESC LIMIT ?{}", idx));
    params_vec.push(libsql::Value::Integer(limit as i64));

    let mut rows = conn.query(&sql, libsql::params_from_iter(params_vec)).await?;
    let mut results = Vec::new();
    while let Some(row) = rows.next().await? {
        results.push(row_to_session_quality(&row)?);
    }
    Ok(results)
}

// ---------------------------------------------------------------------------
// Drift event functions
// ---------------------------------------------------------------------------

const VALID_DRIFT_TYPES: &[&str] = &[
    "priority",
    "framework",
    "interaction",
    "meaning",
    "safety",
    "structural",
];

const VALID_SEVERITIES: &[&str] = &["low", "medium", "high", "critical"];

pub async fn record_drift_event(
    db: &Database,
    req: RecordDriftEventRequest,
) -> Result<DriftEvent> {
    // Validate drift_type
    if !VALID_DRIFT_TYPES.contains(&req.drift_type.as_str()) {
        return Err(EngError::InvalidInput(format!(
            "invalid drift_type '{}', must be one of: {}",
            req.drift_type,
            VALID_DRIFT_TYPES.join(", ")
        )));
    }

    let severity = req.severity.unwrap_or_else(|| "low".to_string());

    // Validate severity
    if !VALID_SEVERITIES.contains(&severity.as_str()) {
        return Err(EngError::InvalidInput(format!(
            "invalid severity '{}', must be one of: {}",
            severity,
            VALID_SEVERITIES.join(", ")
        )));
    }

    let conn = &db.conn;

    conn.execute(
        "INSERT INTO behavioral_drift_events (agent, session_id, drift_type, severity, signal)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        libsql::params![req.agent, req.session_id, req.drift_type, severity, req.signal],
    )
    .await?;

    let mut rows = conn.query("SELECT last_insert_rowid()", ()).await?;
    let id_row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::Internal("no rowid".into()))?;
    let id: i64 = id_row.get(0)?;

    let mut drift_rows = conn
        .query(
            "SELECT id, agent, session_id, drift_type, severity, signal, created_at
             FROM behavioral_drift_events WHERE id = ?1",
            libsql::params![id],
        )
        .await?;

    let row = drift_rows
        .next()
        .await?
        .ok_or_else(|| EngError::Internal("drift event not found after insert".into()))?;

    row_to_drift_event(&row)
}

pub async fn get_drift_events(
    db: &Database,
    agent: &str,
    limit: usize,
) -> Result<Vec<DriftEvent>> {
    let conn = &db.conn;
    let mut rows = conn
        .query(
            "SELECT id, agent, session_id, drift_type, severity, signal, created_at
             FROM behavioral_drift_events WHERE agent = ?1
             ORDER BY created_at DESC LIMIT ?2",
            libsql::params![agent, limit as i64],
        )
        .await?;

    let mut results = Vec::new();
    while let Some(row) = rows.next().await? {
        results.push(row_to_drift_event(&row)?);
    }
    Ok(results)
}

pub async fn get_drift_summary(
    db: &Database,
    agent: &str,
) -> Result<Vec<DriftSummaryEntry>> {
    let conn = &db.conn;
    let mut rows = conn
        .query(
            "SELECT drift_type, severity, COUNT(*) as count
             FROM behavioral_drift_events WHERE agent = ?1
             GROUP BY drift_type, severity
             ORDER BY count DESC",
            libsql::params![agent],
        )
        .await?;

    let mut results = Vec::new();
    while let Some(row) = rows.next().await? {
        results.push(DriftSummaryEntry {
            drift_type: row.get(0)?,
            severity: row.get(1)?,
            count: row.get(2)?,
        });
    }
    Ok(results)
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

pub async fn get_stats(db: &Database) -> Result<ThymusStats> {
    let conn = &db.conn;

    let rubrics = {
        let mut rows = conn.query("SELECT COUNT(*) FROM rubrics", ()).await?;
        let row = rows.next().await?.ok_or_else(|| EngError::Internal("no count row".into()))?;
        let n: i64 = row.get(0)?;
        n
    };

    let evaluations = {
        let mut rows = conn.query("SELECT COUNT(*) FROM evaluations", ()).await?;
        let row = rows.next().await?.ok_or_else(|| EngError::Internal("no count row".into()))?;
        let n: i64 = row.get(0)?;
        n
    };

    let metrics = {
        let mut rows = conn.query("SELECT COUNT(*) FROM quality_metrics", ()).await?;
        let row = rows.next().await?.ok_or_else(|| EngError::Internal("no count row".into()))?;
        let n: i64 = row.get(0)?;
        n
    };

    let agent_count = {
        let mut rows = conn
            .query("SELECT COUNT(DISTINCT agent) FROM evaluations", ())
            .await?;
        let row = rows.next().await?.ok_or_else(|| EngError::Internal("no count row".into()))?;
        let n: i64 = row.get(0)?;
        n
    };

    Ok(ThymusStats {
        rubrics,
        evaluations,
        metrics,
        agent_count,
    })
}
