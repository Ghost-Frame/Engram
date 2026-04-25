use crate::db::Database;
use crate::{EngError, Result};
use serde::{Deserialize, Serialize};

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
    pub user_id: i64,
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
    pub user_id: Option<i64>,
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
    pub user_id: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordDriftEventRequest {
    pub agent: String,
    pub session_id: Option<String>,
    pub drift_type: String,
    pub severity: Option<String>,
    pub signal: String,
    pub user_id: Option<i64>,
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
// Error helper
// ---------------------------------------------------------------------------

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

// ---------------------------------------------------------------------------
// Row helpers
// ---------------------------------------------------------------------------

fn row_to_rubric(row: &rusqlite::Row<'_>) -> Result<Rubric> {
    let criteria_str: String = row.get(3).map_err(rusqlite_to_eng_error)?;
    let criteria: serde_json::Value =
        serde_json::from_str(&criteria_str).unwrap_or(serde_json::Value::Array(vec![]));
    Ok(Rubric {
        id: row.get(0).map_err(rusqlite_to_eng_error)?,
        name: row.get(1).map_err(rusqlite_to_eng_error)?,
        description: row.get(2).map_err(rusqlite_to_eng_error)?,
        criteria,
        user_id: 1,
        created_at: row.get(4).map_err(rusqlite_to_eng_error)?,
        updated_at: row.get(5).map_err(rusqlite_to_eng_error)?,
    })
}

fn row_to_evaluation(row: &rusqlite::Row<'_>) -> Result<Evaluation> {
    let input_str: String = row.get(4).map_err(rusqlite_to_eng_error)?;
    let output_str: String = row.get(5).map_err(rusqlite_to_eng_error)?;
    let scores_str: String = row.get(6).map_err(rusqlite_to_eng_error)?;

    let input: serde_json::Value = serde_json::from_str(&input_str)
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    let output: serde_json::Value = serde_json::from_str(&output_str)
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    let scores: serde_json::Value = serde_json::from_str(&scores_str)
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

    Ok(Evaluation {
        id: row.get(0).map_err(rusqlite_to_eng_error)?,
        rubric_id: row.get(1).map_err(rusqlite_to_eng_error)?,
        agent: row.get(2).map_err(rusqlite_to_eng_error)?,
        subject: row.get(3).map_err(rusqlite_to_eng_error)?,
        input,
        output,
        scores,
        overall_score: row.get(7).map_err(rusqlite_to_eng_error)?,
        notes: row.get(8).map_err(rusqlite_to_eng_error)?,
        evaluator: row.get(9).map_err(rusqlite_to_eng_error)?,
        user_id: 1,
        created_at: row.get(10).map_err(rusqlite_to_eng_error)?,
    })
}

fn row_to_metric(row: &rusqlite::Row<'_>) -> Result<QualityMetric> {
    let tags_str: String = row.get(4).map_err(rusqlite_to_eng_error)?;
    let tags: serde_json::Value = serde_json::from_str(&tags_str)
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    Ok(QualityMetric {
        id: row.get(0).map_err(rusqlite_to_eng_error)?,
        agent: row.get(1).map_err(rusqlite_to_eng_error)?,
        metric: row.get(2).map_err(rusqlite_to_eng_error)?,
        value: row.get(3).map_err(rusqlite_to_eng_error)?,
        tags,
        user_id: 1,
        recorded_at: row.get(5).map_err(rusqlite_to_eng_error)?,
    })
}

fn row_to_session_quality(row: &rusqlite::Row<'_>) -> Result<SessionQuality> {
    let rules_followed_str: String = row.get(4).map_err(rusqlite_to_eng_error)?;
    let rules_drifted_str: String = row.get(5).map_err(rusqlite_to_eng_error)?;

    let rules_followed: serde_json::Value =
        serde_json::from_str(&rules_followed_str).unwrap_or(serde_json::Value::Array(vec![]));
    let rules_drifted: serde_json::Value =
        serde_json::from_str(&rules_drifted_str).unwrap_or(serde_json::Value::Array(vec![]));

    Ok(SessionQuality {
        id: row.get(0).map_err(rusqlite_to_eng_error)?,
        session_id: row.get(1).map_err(rusqlite_to_eng_error)?,
        agent: row.get(2).map_err(rusqlite_to_eng_error)?,
        turn_count: row.get(3).map_err(rusqlite_to_eng_error)?,
        rules_followed,
        rules_drifted,
        personality_score: row.get(6).map_err(rusqlite_to_eng_error)?,
        rule_compliance_rate: row.get(7).map_err(rusqlite_to_eng_error)?,
        user_id: 1,
        created_at: row.get(8).map_err(rusqlite_to_eng_error)?,
    })
}

fn row_to_drift_event(row: &rusqlite::Row<'_>) -> Result<DriftEvent> {
    Ok(DriftEvent {
        id: row.get(0).map_err(rusqlite_to_eng_error)?,
        agent: row.get(1).map_err(rusqlite_to_eng_error)?,
        session_id: row.get(2).map_err(rusqlite_to_eng_error)?,
        drift_type: row.get(3).map_err(rusqlite_to_eng_error)?,
        severity: row.get(4).map_err(rusqlite_to_eng_error)?,
        signal: row.get(5).map_err(rusqlite_to_eng_error)?,
        user_id: 1,
        created_at: row.get(6).map_err(rusqlite_to_eng_error)?,
    })
}

// ---------------------------------------------------------------------------
// Rubric functions
// ---------------------------------------------------------------------------

#[tracing::instrument(skip(db, req))]
pub async fn create_rubric(db: &Database, req: CreateRubricRequest) -> Result<Rubric> {
    let _user_id = req.user_id;
    let criteria_json = serde_json::to_string(&req.criteria)?;

    let id = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO rubrics (name, description, criteria)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![req.name, req.description, criteria_json],
            )
            .map_err(rusqlite_to_eng_error)?;
            Ok(conn.last_insert_rowid())
        })
        .await?;

    get_rubric(db, id, 1).await
}

#[tracing::instrument(skip(db))]
pub async fn get_rubric(db: &Database, id: i64, _user_id: i64) -> Result<Rubric> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, name, description, criteria, created_at, updated_at
                 FROM rubrics WHERE id = ?1",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![id])
            .map_err(rusqlite_to_eng_error)?;
        let row = rows
            .next()
            .map_err(rusqlite_to_eng_error)?
            .ok_or_else(|| EngError::NotFound(format!("rubric {}", id)))?;
        row_to_rubric(row)
    })
    .await
}

#[tracing::instrument(skip(db))]
pub async fn list_rubrics(db: &Database, _user_id: i64) -> Result<Vec<Rubric>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, name, description, criteria, created_at, updated_at
                 FROM rubrics ORDER BY created_at DESC",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query([])
            .map_err(rusqlite_to_eng_error)?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            results.push(row_to_rubric(row)?);
        }
        Ok(results)
    })
    .await
}

#[tracing::instrument(skip(db, req))]
pub async fn update_rubric(
    db: &Database,
    id: i64,
    req: UpdateRubricRequest,
    _user_id: i64,
) -> Result<Rubric> {
    // Verify existence
    get_rubric(db, id, 1).await?;

    let mut sets: Vec<String> = Vec::new();
    let mut params_vec: Vec<rusqlite::types::Value> = Vec::new();
    let mut idx = 1usize;

    if let Some(v) = req.name {
        sets.push(format!("name = ?{}", idx));
        params_vec.push(rusqlite::types::Value::Text(v));
        idx += 1;
    }
    if let Some(v) = req.description {
        sets.push(format!("description = ?{}", idx));
        params_vec.push(rusqlite::types::Value::Text(v));
        idx += 1;
    }
    if let Some(v) = req.criteria {
        let j = serde_json::to_string(&v)?;
        sets.push(format!("criteria = ?{}", idx));
        params_vec.push(rusqlite::types::Value::Text(j));
        idx += 1;
    }

    sets.push("updated_at = datetime('now')".to_string());

    if sets.is_empty() {
        return get_rubric(db, id, 1).await;
    }

    let sql = format!(
        "UPDATE rubrics SET {} WHERE id = ?{}",
        sets.join(", "),
        idx
    );
    params_vec.push(rusqlite::types::Value::Integer(id));

    db.write(move |conn| {
        conn.execute(&sql, rusqlite::params_from_iter(params_vec))
            .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await?;

    get_rubric(db, id, 1).await
}

#[tracing::instrument(skip(db))]
pub async fn delete_rubric(db: &Database, id: i64, _user_id: i64) -> Result<bool> {
    db.write(move |conn| {
        conn.execute(
            "DELETE FROM rubrics WHERE id = ?1",
            rusqlite::params![id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(true)
    })
    .await
}

// ---------------------------------------------------------------------------
// Evaluation functions
// ---------------------------------------------------------------------------

/// Compute weighted overall score from rubric criteria and provided scores.
/// Each criterion: { name, weight, scale_min, scale_max }
/// normalized = (score - scale_min) / (scale_max - scale_min)
/// overall = sum(normalized * weight) / total_weight
fn compute_weighted_score(criteria: &serde_json::Value, scores: &serde_json::Value) -> Result<f64> {
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
            .ok_or_else(|| {
                EngError::InvalidInput(format!("missing score for criterion '{}'", name))
            })?;

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

#[tracing::instrument(skip(db, req), fields(rubric_id = req.rubric_id))]
pub async fn evaluate(db: &Database, req: EvaluateRequest) -> Result<Evaluation> {
    let _user_id = req.user_id;

    // Fetch rubric to get criteria
    let rubric = get_rubric(db, req.rubric_id, 1).await?;

    // Compute weighted overall score
    let overall_score = compute_weighted_score(&rubric.criteria, &req.scores)?;

    let input = req
        .input
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    let output = req
        .output
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    let input_json = serde_json::to_string(&input)?;
    let output_json = serde_json::to_string(&output)?;
    let scores_json = serde_json::to_string(&req.scores)?;

    let id = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO evaluations (rubric_id, agent, subject, input, output, scores, overall_score, notes, evaluator)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    req.rubric_id,
                    req.agent,
                    req.subject,
                    input_json,
                    output_json,
                    scores_json,
                    overall_score,
                    req.notes,
                    req.evaluator,
                ],
            )
            .map_err(rusqlite_to_eng_error)?;
            Ok(conn.last_insert_rowid())
        })
        .await?;

    get_evaluation(db, id, 1).await
}

#[tracing::instrument(skip(db))]
pub async fn get_evaluation(db: &Database, id: i64, _user_id: i64) -> Result<Evaluation> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, rubric_id, agent, subject, input, output, scores, overall_score,
                        notes, evaluator, created_at
                 FROM evaluations WHERE id = ?1",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![id])
            .map_err(rusqlite_to_eng_error)?;
        let row = rows
            .next()
            .map_err(rusqlite_to_eng_error)?
            .ok_or_else(|| EngError::NotFound(format!("evaluation {}", id)))?;
        row_to_evaluation(row)
    })
    .await
}

#[tracing::instrument(skip(db))]
pub async fn list_evaluations(
    db: &Database,
    _user_id: i64,
    agent: Option<&str>,
    rubric_id: Option<i64>,
    limit: usize,
) -> Result<Vec<Evaluation>> {
    let mut sql = String::from(
        "SELECT id, rubric_id, agent, subject, input, output, scores, overall_score,
                notes, evaluator, created_at
         FROM evaluations WHERE 1=1",
    );
    let mut params_vec: Vec<rusqlite::types::Value> = Vec::new();
    let mut idx = 1usize;

    if let Some(a) = agent {
        sql.push_str(&format!(" AND agent = ?{}", idx));
        params_vec.push(rusqlite::types::Value::Text(a.to_string()));
        idx += 1;
    }
    if let Some(rid) = rubric_id {
        sql.push_str(&format!(" AND rubric_id = ?{}", idx));
        params_vec.push(rusqlite::types::Value::Integer(rid));
        idx += 1;
    }

    sql.push_str(&format!(" ORDER BY created_at DESC LIMIT ?{}", idx));
    params_vec.push(rusqlite::types::Value::Integer(limit as i64));

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params_from_iter(params_vec))
            .map_err(rusqlite_to_eng_error)?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            results.push(row_to_evaluation(row)?);
        }
        Ok(results)
    })
    .await
}

#[tracing::instrument(skip(db), fields(agent = %agent))]
pub async fn get_agent_scores(
    db: &Database,
    _user_id: i64,
    agent: &str,
    rubric_id: Option<i64>,
    since: Option<&str>,
) -> Result<AgentScores> {
    let mut sql = String::from(
        "SELECT overall_score, scores FROM evaluations
         WHERE agent = ?1",
    );
    let mut params_vec: Vec<rusqlite::types::Value> = vec![
        rusqlite::types::Value::Text(agent.to_string()),
    ];
    let mut idx = 2usize;

    if let Some(rid) = rubric_id {
        sql.push_str(&format!(" AND rubric_id = ?{}", idx));
        params_vec.push(rusqlite::types::Value::Integer(rid));
        idx += 1;
    }
    if let Some(s) = since {
        sql.push_str(&format!(" AND created_at >= ?{}", idx));
        params_vec.push(rusqlite::types::Value::Text(s.to_string()));
    }

    let agent_owned = agent.to_string();

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params_from_iter(params_vec))
            .map_err(rusqlite_to_eng_error)?;

        let mut overall_sum = 0.0f64;
        let mut count = 0i64;
        // Per-criterion accumulators: name -> (sum, min, max, count)
        let mut criterion_stats: std::collections::HashMap<String, (f64, f64, f64, i64)> =
            std::collections::HashMap::new();

        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            let overall: f64 = row.get(0).map_err(rusqlite_to_eng_error)?;
            let scores_str: String = row.get(1).map_err(rusqlite_to_eng_error)?;

            overall_sum += overall;
            count += 1;

            if let Ok(scores_obj) = serde_json::from_str::<serde_json::Value>(&scores_str) {
                if let Some(obj) = scores_obj.as_object() {
                    for (k, v) in obj {
                        if let Some(score) = v.as_f64() {
                            let entry = criterion_stats.entry(k.clone()).or_insert((
                                0.0,
                                f64::MAX,
                                f64::MIN,
                                0,
                            ));
                            entry.0 += score;
                            entry.1 = entry.1.min(score);
                            entry.2 = entry.2.max(score);
                            entry.3 += 1;
                        }
                    }
                }
            }
        }

        let overall_avg = if count == 0 {
            0.0
        } else {
            overall_sum / count as f64
        };

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
            agent: agent_owned,
            overall_avg,
            evaluation_count: count,
            by_criterion: serde_json::Value::Object(by_criterion),
        })
    })
    .await
}

// ---------------------------------------------------------------------------
// Metric functions
// ---------------------------------------------------------------------------

#[tracing::instrument(skip(db, req))]
pub async fn record_metric(db: &Database, req: RecordMetricRequest) -> Result<QualityMetric> {
    let _user_id = req.user_id;
    let tags = req
        .tags
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    let tags_json = serde_json::to_string(&tags)?;

    let id = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO quality_metrics (agent, metric, value, tags)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![req.agent, req.metric, req.value, tags_json],
            )
            .map_err(rusqlite_to_eng_error)?;
            Ok(conn.last_insert_rowid())
        })
        .await?;

    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, agent, metric, value, tags, recorded_at
                 FROM quality_metrics WHERE id = ?1",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![id])
            .map_err(rusqlite_to_eng_error)?;
        let row = rows
            .next()
            .map_err(rusqlite_to_eng_error)?
            .ok_or_else(|| EngError::Internal("metric not found after insert".into()))?;
        row_to_metric(row)
    })
    .await
}

#[tracing::instrument(skip(db))]
pub async fn get_metrics(
    db: &Database,
    _user_id: i64,
    agent: Option<&str>,
    metric: Option<&str>,
    since: Option<&str>,
    limit: usize,
) -> Result<Vec<QualityMetric>> {
    let mut sql = String::from(
        "SELECT id, agent, metric, value, tags, recorded_at
         FROM quality_metrics WHERE 1=1",
    );
    let mut params_vec: Vec<rusqlite::types::Value> = Vec::new();
    let mut idx = 1usize;

    if let Some(a) = agent {
        sql.push_str(&format!(" AND agent = ?{}", idx));
        params_vec.push(rusqlite::types::Value::Text(a.to_string()));
        idx += 1;
    }
    if let Some(m) = metric {
        sql.push_str(&format!(" AND metric = ?{}", idx));
        params_vec.push(rusqlite::types::Value::Text(m.to_string()));
        idx += 1;
    }
    if let Some(s) = since {
        sql.push_str(&format!(" AND recorded_at >= ?{}", idx));
        params_vec.push(rusqlite::types::Value::Text(s.to_string()));
        idx += 1;
    }

    sql.push_str(&format!(" ORDER BY recorded_at DESC LIMIT ?{}", idx));
    params_vec.push(rusqlite::types::Value::Integer(limit as i64));

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params_from_iter(params_vec))
            .map_err(rusqlite_to_eng_error)?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            results.push(row_to_metric(row)?);
        }
        Ok(results)
    })
    .await
}

#[tracing::instrument(skip(db), fields(agent = %agent, metric = %metric))]
pub async fn get_metric_summary(
    db: &Database,
    _user_id: i64,
    agent: &str,
    metric: &str,
    since: Option<&str>,
) -> Result<serde_json::Value> {
    let mut sql = String::from(
        "SELECT AVG(value), MIN(value), MAX(value), COUNT(*)
         FROM quality_metrics WHERE agent = ?1 AND metric = ?2",
    );
    let mut params_vec: Vec<rusqlite::types::Value> = vec![
        rusqlite::types::Value::Text(agent.to_string()),
        rusqlite::types::Value::Text(metric.to_string()),
    ];

    if let Some(s) = since {
        sql.push_str(" AND recorded_at >= ?3");
        params_vec.push(rusqlite::types::Value::Text(s.to_string()));
    }

    let agent_owned = agent.to_string();
    let metric_owned = metric.to_string();

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params_from_iter(params_vec))
            .map_err(rusqlite_to_eng_error)?;
        let row = rows
            .next()
            .map_err(rusqlite_to_eng_error)?
            .ok_or_else(|| EngError::Internal("no summary row".into()))?;

        let avg: Option<f64> = row.get(0).map_err(rusqlite_to_eng_error)?;
        let min: Option<f64> = row.get(1).map_err(rusqlite_to_eng_error)?;
        let max: Option<f64> = row.get(2).map_err(rusqlite_to_eng_error)?;
        let count: i64 = row.get(3).map_err(rusqlite_to_eng_error)?;

        Ok(serde_json::json!({
            "avg": avg,
            "min": min,
            "max": max,
            "count": count,
            "agent": agent_owned,
            "metric": metric_owned,
        }))
    })
    .await
}

// ---------------------------------------------------------------------------
// Session quality functions
// ---------------------------------------------------------------------------

#[tracing::instrument(skip(db, req))]
pub async fn record_session_quality(
    db: &Database,
    req: RecordSessionQualityRequest,
) -> Result<SessionQuality> {
    let _user_id = req.user_id;
    let turn_count = req.turn_count.unwrap_or(0);
    let rules_followed = req.rules_followed.unwrap_or_default();
    let rules_drifted = req.rules_drifted.unwrap_or_default();
    let rules_followed_json = serde_json::to_string(&rules_followed)?;
    let rules_drifted_json = serde_json::to_string(&rules_drifted)?;

    db.write(move |conn| {
        let mut stmt = conn
            .prepare(
                "INSERT INTO session_quality (session_id, agent, turn_count, rules_followed, rules_drifted, personality_score, rule_compliance_rate)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 RETURNING id, session_id, agent, turn_count, rules_followed, rules_drifted,
                           personality_score, rule_compliance_rate, created_at",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![
                req.session_id,
                req.agent,
                turn_count,
                rules_followed_json,
                rules_drifted_json,
                req.personality_score,
                req.rule_compliance_rate,
            ])
            .map_err(rusqlite_to_eng_error)?;
        let row = rows
            .next()
            .map_err(rusqlite_to_eng_error)?
            .ok_or_else(|| EngError::Internal("session_quality RETURNING row was empty".into()))?;
        row_to_session_quality(row)
    })
    .await
}

#[tracing::instrument(skip(db), fields(agent = %agent))]
pub async fn get_session_quality(
    db: &Database,
    _user_id: i64,
    agent: &str,
    since: Option<&str>,
    limit: usize,
) -> Result<Vec<SessionQuality>> {
    // SECURITY: cap the limit so a caller cannot ask for a trillion rows.
    let capped_limit = limit.min(1_000);

    let mut sql = String::from(
        "SELECT id, session_id, agent, turn_count, rules_followed, rules_drifted,
                personality_score, rule_compliance_rate, created_at
         FROM session_quality WHERE agent = ?1",
    );
    let mut params_vec: Vec<rusqlite::types::Value> = vec![
        rusqlite::types::Value::Text(agent.to_string()),
    ];
    let mut idx = 2usize;

    if let Some(s) = since {
        sql.push_str(&format!(" AND created_at >= ?{}", idx));
        params_vec.push(rusqlite::types::Value::Text(s.to_string()));
        idx += 1;
    }

    sql.push_str(&format!(" ORDER BY created_at DESC LIMIT ?{}", idx));
    params_vec.push(rusqlite::types::Value::Integer(capped_limit as i64));

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params_from_iter(params_vec))
            .map_err(rusqlite_to_eng_error)?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            results.push(row_to_session_quality(row)?);
        }
        Ok(results)
    })
    .await
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

#[tracing::instrument(skip(db, req))]
pub async fn record_drift_event(db: &Database, req: RecordDriftEventRequest) -> Result<DriftEvent> {
    let _user_id = req.user_id;

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

    db.write(move |conn| {
        let mut stmt = conn
            .prepare(
                "INSERT INTO behavioral_drift_events (agent, session_id, drift_type, severity, signal)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 RETURNING id, agent, session_id, drift_type, severity, signal, created_at",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![
                req.agent,
                req.session_id,
                req.drift_type,
                severity,
                req.signal,
            ])
            .map_err(rusqlite_to_eng_error)?;
        let row = rows
            .next()
            .map_err(rusqlite_to_eng_error)?
            .ok_or_else(|| EngError::Internal("drift event RETURNING row was empty".into()))?;
        row_to_drift_event(row)
    })
    .await
}

#[tracing::instrument(skip(db), fields(agent = %agent))]
pub async fn get_drift_events(
    db: &Database,
    _user_id: i64,
    agent: &str,
    limit: usize,
) -> Result<Vec<DriftEvent>> {
    let capped = limit.min(1_000);
    let agent_owned = agent.to_string();

    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, agent, session_id, drift_type, severity, signal, created_at
                 FROM behavioral_drift_events WHERE agent = ?1
                 ORDER BY created_at DESC LIMIT ?2",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![agent_owned, capped as i64])
            .map_err(rusqlite_to_eng_error)?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            results.push(row_to_drift_event(row)?);
        }
        Ok(results)
    })
    .await
}

#[tracing::instrument(skip(db), fields(agent = %agent))]
pub async fn get_drift_summary(
    db: &Database,
    _user_id: i64,
    agent: &str,
) -> Result<Vec<DriftSummaryEntry>> {
    let agent_owned = agent.to_string();

    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT drift_type, severity, COUNT(*) as count
                 FROM behavioral_drift_events WHERE agent = ?1
                 GROUP BY drift_type, severity
                 ORDER BY count DESC",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![agent_owned])
            .map_err(rusqlite_to_eng_error)?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            results.push(DriftSummaryEntry {
                drift_type: row.get(0).map_err(rusqlite_to_eng_error)?,
                severity: row.get(1).map_err(rusqlite_to_eng_error)?,
                count: row.get(2).map_err(rusqlite_to_eng_error)?,
            });
        }
        Ok(results)
    })
    .await
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

#[tracing::instrument(skip(db))]
pub async fn get_stats(db: &Database, _user_id: i64) -> Result<ThymusStats> {
    db.read(move |conn| {
        let rubrics: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM rubrics",
                [],
                |row| row.get(0),
            )
            .map_err(rusqlite_to_eng_error)?;

        let evaluations: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM evaluations",
                [],
                |row| row.get(0),
            )
            .map_err(rusqlite_to_eng_error)?;

        let metrics: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM quality_metrics",
                [],
                |row| row.get(0),
            )
            .map_err(rusqlite_to_eng_error)?;

        let agent_count: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT agent) FROM evaluations",
                [],
                |row| row.get(0),
            )
            .map_err(rusqlite_to_eng_error)?;

        Ok(ThymusStats {
            rubrics,
            evaluations,
            metrics,
            agent_count,
        })
    })
    .await
}
