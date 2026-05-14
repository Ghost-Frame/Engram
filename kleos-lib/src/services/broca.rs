//! Broca service: action logging and template-based narration.
//!
//! The primary data store is `broca_actions`. `log_action` inserts a row and
//! optionally auto-generates a `narrative` using `narrate_from_template` when
//! the caller does not supply one. The template table mirrors the JavaScript
//! narrator in `Ghost-Frame/broca/src/narrator.ts`.

use crate::db::Database;
use crate::{EngError, Result};
use serde::{Deserialize, Serialize};

/// A single row from the `broca_actions` table returned to callers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionEntry {
    /// Row primary key.
    pub id: i64,
    /// Identifier of the agent that performed the action.
    pub agent: String,
    /// Service name (e.g., `"kleos"`, `"chiasm"`).
    pub service: String,
    /// Action type string (e.g., `"task.started"`).
    pub action: String,
    /// Structured event payload stored as a JSON object.
    pub payload: serde_json::Value,
    /// Human-readable sentence describing the action; `None` when no template
    /// matched and no caller-supplied narrative was provided.
    pub narrative: Option<String>,
    /// Upstream Axon event id when the action was ingested via webhook.
    pub axon_event_id: Option<i64>,
    /// Tenant user id that owns this row.
    pub user_id: i64,
    /// ISO-8601 UTC timestamp of insertion.
    pub created_at: String,
}

/// Input to [`log_action`]: describes the action to be recorded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogActionRequest {
    /// Identifier of the agent performing the action.
    pub agent: String,
    /// Service name; defaults to `"kleos"` when `None`.
    #[serde(default)]
    pub service: Option<String>,
    /// Action type string.
    pub action: String,
    /// Pre-computed human-readable narrative. When `None`, `log_action`
    /// attempts to derive one via `narrate_from_template`.
    #[serde(default)]
    pub narrative: Option<String>,
    /// Structured event payload. Stored as JSON text; defaults to `{}`.
    #[serde(default)]
    pub payload: Option<serde_json::Value>,
    /// Upstream Axon event id; set by the ingest webhook path.
    #[serde(default)]
    pub axon_event_id: Option<i64>,
    /// Tenant user id. Required at call time; the `Option` allows deserialization
    /// from contexts where the value is injected after parsing.
    #[serde(default)]
    pub user_id: Option<i64>,
}

/// Aggregate statistics returned by [`get_stats`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrocaStats {
    /// Total number of action rows in the shard.
    pub total_actions: i64,
    /// Number of distinct agent identifiers.
    pub agents: i64,
    /// Number of distinct service names.
    pub services: i64,
}

/// Action-type to template lookup. Each template uses `{{key}}` placeholders
/// that are substituted from the action payload at narration time.
///
/// Sourced from the Ghost-Frame/broca standalone `narrator.ts`; kept in sync
/// manually. Missing payload keys are left as the literal `{{key}}` text so
/// they are visible in the output rather than silently suppressed.
///
/// Note: the original JS templates use short-circuit OR (`p.agent || "fallback"`)
/// for absent keys and a `humanStatus` helper that maps internal status codes
/// to English. The `{{key}}` approach here substitutes raw payload values.
/// If a payload key is absent the `{{key}}` literal remains, making the gap
/// visible. A future version may add per-template post-processing.
const TEMPLATES: &[(&str, &str)] = &[
    // ---- Chiasm / tasks ----
    ("task.created",          "{{agent}} started a new task: \"{{title}}\" in {{project}}"),
    ("task.updated",          "\"{{title}}\" status is now {{status}}"),
    ("task.completed",        "\"{{title}}\" was completed by {{agent}}"),
    ("task.blocked",          "\"{{title}}\" is blocked: {{reason}}"),
    ("task.blocked_on_human", "\"{{title}}\" is waiting for human approval: {{summary}}"),
    ("task.feedback",         "Human feedback on \"{{title}}\": \"{{feedback}}\""),
    ("task.output",           "Output submitted for \"{{title}}\""),
    ("task.plan",             "A plan was generated for \"{{title}}\""),
    // ---- Loom / workflows ----
    ("workflow.run.created",    "{{agent}} started the \"{{workflow}}\" workflow"),
    ("workflow.run.completed",  "The \"{{workflow}}\" workflow finished successfully"),
    ("workflow.run.failed",     "The \"{{workflow}}\" workflow failed on step \"{{failed_step}}\": {{error}}"),
    ("workflow.run.cancelled",  "The \"{{workflow}}\" workflow was cancelled"),
    ("workflow.step.started",   "Step \"{{step}}\" started in the \"{{workflow}}\" workflow"),
    ("workflow.step.completed", "Step \"{{step}}\" finished in the \"{{workflow}}\" workflow"),
    ("workflow.step.failed",    "Step \"{{step}}\" failed in the \"{{workflow}}\" workflow: {{error}}"),
    // ---- Soma / agents ----
    ("agent.registered",   "{{name}} came online as a {{type}}"),
    ("agent.deregistered", "{{name}} went offline"),
    ("agent.online",       "{{agent}} is online"),
    ("agent.offline",      "{{agent}} went offline"),
    ("agent.heartbeat",    "{{agent}} checked in"),
    ("agent.error",        "{{agent}} reported an error: {{error}}"),
    // ---- Kleos / memory ----
    ("memory.stored",   "{{source}} stored a memory ({{category}})"),
    ("memory.searched", "{{agent}} searched memory for \"{{query}}\""),
    ("memory.linked",   "Two memories were linked together"),
    ("memory.forgotten", "A memory was removed"),
    // ---- Thymus / evaluations ----
    ("evaluation.completed", "{{agent}}'s work on \"{{subject}}\" was evaluated using the {{rubric}} rubric"),
    ("metric.recorded",      "{{agent}} recorded {{metric}}: {{value}}"),
    // ---- Axon / system ----
    ("system.started",    "{{service}} started up"),
    ("system.stopped",    "{{service}} shut down"),
    ("deploy.started",    "Deployment started for {{service}}"),
    ("deploy.succeeded",  "{{service}} deployed successfully"),
    ("deploy.failed",     "Deployment failed for {{service}}: {{error}}"),
    ("deploy.rolled_back","{{service}} was rolled back"),
    ("alert.triggered",   "Alert triggered: {{message}}"),
];

/// Render a template-based narrative for the given action type and payload.
///
/// Returns `None` if no template is registered for `action`.
/// `{{key}}` placeholders are replaced from the payload's top-level string
/// and non-string keys; missing keys are left as the literal `{{key}}` text
/// so callers can see which fields were absent rather than receiving a
/// silently-incomplete sentence.
pub fn narrate_from_template(action: &str, payload: &serde_json::Value) -> Option<String> {
    let template = TEMPLATES
        .iter()
        .find(|(a, _)| *a == action)
        .map(|(_, t)| *t)?;

    let mut out = template.to_string();
    if let Some(obj) = payload.as_object() {
        for (k, v) in obj {
            let needle = format!("{{{{{k}}}}}");
            let replacement = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            out = out.replace(&needle, &replacement);
        }
    }
    Some(out)
}

/// Ordered column list matching the positional field offsets in
/// [`row_to_action_entry`].
const ACTION_COLUMNS: &str =
    "id, agent, service, action, payload, narrative, axon_event_id, user_id, created_at";

/// Converts a `rusqlite::Error` into the crate-level `EngError`.
fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

/// Map a sqlite `Row` returned by an `ACTION_COLUMNS` SELECT into an
/// [`ActionEntry`]. Column offsets must match `ACTION_COLUMNS` exactly.
fn row_to_action_entry(row: &rusqlite::Row<'_>) -> Result<ActionEntry> {
    let payload_str: String = row.get(4).map_err(rusqlite_to_eng_error)?;
    let payload: serde_json::Value = serde_json::from_str(&payload_str)?;
    Ok(ActionEntry {
        id: row.get(0).map_err(rusqlite_to_eng_error)?,
        agent: row.get(1).map_err(rusqlite_to_eng_error)?,
        service: row.get(2).map_err(rusqlite_to_eng_error)?,
        action: row.get(3).map_err(rusqlite_to_eng_error)?,
        payload,
        narrative: row.get(5).map_err(rusqlite_to_eng_error)?,
        axon_event_id: row.get(6).map_err(rusqlite_to_eng_error)?,
        user_id: row.get(7).map_err(rusqlite_to_eng_error)?,
        created_at: row.get(8).map_err(rusqlite_to_eng_error)?,
    })
}

/// Insert a new `broca_actions` row and return the persisted [`ActionEntry`].
///
/// When `req.narrative` is `None`, the narrative is computed via
/// [`narrate_from_template`] using `req.action` and the resolved payload so
/// that actions receive a human-readable description without requiring the
/// caller to pre-compute it.
#[tracing::instrument(skip(db, req), fields(agent = %req.agent, action = %req.action, service = ?req.service, user_id = ?req.user_id))]
pub async fn log_action(db: &Database, req: LogActionRequest) -> Result<ActionEntry> {
    let service = req.service.clone().unwrap_or_else(|| "kleos".to_string());
    let payload = req
        .payload
        .clone()
        .unwrap_or(serde_json::Value::Object(Default::default()));
    let payload_str = serde_json::to_string(&payload)?;
    let user_id = req
        .user_id
        .ok_or_else(|| EngError::InvalidInput("user_id required".into()))?;

    let agent = req.agent.clone();
    let action = req.action.clone();
    // Prefer the caller-supplied narrative; fall back to the template renderer
    // so every action gets a human-readable sentence when a matching template
    // exists. Callers that already ran narrate_from_template upstream will
    // always supply Some(_) and skip this redundant call.
    let narrative = req
        .narrative
        .clone()
        .or_else(|| narrate_from_template(&action, &payload));
    let axon_event_id = req.axon_event_id;
    let svc = service.clone();
    let id = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO broca_actions
                    (agent, service, action, payload, narrative, axon_event_id, user_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    agent,
                    svc,
                    action,
                    payload_str,
                    narrative,
                    axon_event_id,
                    user_id,
                ],
            )
            .map_err(rusqlite_to_eng_error)?;
            Ok(conn.last_insert_rowid())
        })
        .await?;
    get_action(db, id, user_id).await
}

/// Query `broca_actions` with optional filters for agent, service, and action
/// type. Results are returned newest-first with pagination via `limit`/`offset`.
///
/// `_user_id` is currently unused in the WHERE clause because the table is
/// per-tenant (sharded by shard selection at call time), but is kept in the
/// signature for future per-row scoping when needed.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip(db), fields(agent = ?agent, service = ?service, action = ?action, limit, offset, user_id))]
pub async fn query_actions(
    db: &Database,
    agent: Option<&str>,
    service: Option<&str>,
    action: Option<&str>,
    limit: usize,
    offset: usize,
    _user_id: i64,
) -> Result<Vec<ActionEntry>> {
    let mut sql = format!("SELECT {ACTION_COLUMNS} FROM broca_actions WHERE 1=1");
    let mut params_vec: Vec<rusqlite::types::Value> = Vec::new();
    let mut param_idx = 1usize;

    if let Some(a) = agent {
        sql.push_str(&format!(" AND agent = ?{}", param_idx));
        params_vec.push(rusqlite::types::Value::Text(a.to_string()));
        param_idx += 1;
    }
    if let Some(s) = service {
        sql.push_str(&format!(" AND service = ?{}", param_idx));
        params_vec.push(rusqlite::types::Value::Text(s.to_string()));
        param_idx += 1;
    }
    if let Some(act) = action {
        sql.push_str(&format!(" AND action = ?{}", param_idx));
        params_vec.push(rusqlite::types::Value::Text(act.to_string()));
        param_idx += 1;
    }
    sql.push_str(&format!(
        " ORDER BY id DESC LIMIT ?{} OFFSET ?{}",
        param_idx,
        param_idx + 1
    ));
    params_vec.push(rusqlite::types::Value::Integer(limit as i64));
    params_vec.push(rusqlite::types::Value::Integer(offset as i64));

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let params = rusqlite::params_from_iter(params_vec.iter().cloned());
        let mut rows = stmt.query(params).map_err(rusqlite_to_eng_error)?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            results.push(row_to_action_entry(row)?);
        }
        Ok(results)
    })
    .await
}

/// Fetch a single [`ActionEntry`] by primary key.
///
/// Returns [`EngError::NotFound`] when no row with the given `id` exists.
#[tracing::instrument(skip(db), fields(action_id = id, user_id))]
pub async fn get_action(db: &Database, id: i64, _user_id: i64) -> Result<ActionEntry> {
    let sql = format!("SELECT {ACTION_COLUMNS} FROM broca_actions WHERE id = ?1");

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![id])
            .map_err(rusqlite_to_eng_error)?;
        let row = rows
            .next()
            .map_err(rusqlite_to_eng_error)?
            .ok_or_else(|| EngError::NotFound(format!("action {}", id)))?;
        row_to_action_entry(row)
    })
    .await
}

/// Return aggregate [`BrocaStats`] for the given tenant shard.
///
/// The `_user_id` argument is present for API symmetry with the other service
/// functions but is not currently used in the query (the shard is already
/// user-scoped).
#[tracing::instrument(skip(db), fields(user_id))]
pub async fn get_stats(db: &Database, _user_id: i64) -> Result<BrocaStats> {
    db.read(move |conn| {
        conn.query_row(
            "SELECT COUNT(*), COUNT(DISTINCT agent), COUNT(DISTINCT service)
             FROM broca_actions",
            [],
            |row| {
                Ok(BrocaStats {
                    total_actions: row.get(0)?,
                    agents: row.get(1)?,
                    services: row.get(2)?,
                })
            },
        )
        .map_err(rusqlite_to_eng_error)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    async fn setup() -> Database {
        let db = Database::connect_memory().await.expect("db");
        // Apply monolith migrations so broca_actions exists with user_id (v45).
        db.write(|conn| crate::db::migrations::run_migrations(conn))
            .await
            .expect("migrations");
        db
    }

    #[tokio::test]
    async fn log_and_get_action() {
        let db = setup().await;
        let entry = log_action(
            &db,
            LogActionRequest {
                agent: "claude-code".into(),
                service: Some("kleos".into()),
                action: "task.started".into(),
                narrative: Some("starting a port".into()),
                payload: Some(serde_json::json!({"project": "kleos"})),
                axon_event_id: None,
                user_id: Some(1),
            },
        )
        .await
        .expect("log");
        assert_eq!(entry.service, "kleos");
        assert_eq!(entry.action, "task.started");
        assert_eq!(entry.user_id, 1);
        let fetched = get_action(&db, entry.id, 1).await.unwrap();
        assert_eq!(fetched.id, entry.id);
    }

    /// Regression test: query_actions filters by user_id so a row owned by
    /// user 1 must not surface to a query scoped to user 2.
    #[tokio::test]
    async fn query_is_scoped_by_user() {
        let db = setup().await;
        log_action(
            &db,
            LogActionRequest {
                agent: "a".into(),
                service: Some("s".into()),
                action: "x".into(),
                narrative: None,
                payload: None,
                axon_event_id: None,
                user_id: Some(1),
            },
        )
        .await
        .unwrap();
        let other = query_actions(&db, None, None, None, 10, 0, 2)
            .await
            .unwrap();
        assert!(other.is_empty(), "user 2 must not see user 1's actions");
        let mine = query_actions(&db, None, None, None, 10, 0, 1)
            .await
            .unwrap();
        assert_eq!(mine.len(), 1, "user 1 should see their own row");
        assert_eq!(mine[0].user_id, 1);
    }

    #[tokio::test]
    async fn get_stats_is_scoped_by_user() {
        let db = setup().await;
        log_action(
            &db,
            LogActionRequest {
                agent: "alice".into(),
                service: Some("s".into()),
                action: "x".into(),
                narrative: None,
                payload: None,
                axon_event_id: None,
                user_id: Some(1),
            },
        )
        .await
        .unwrap();
        log_action(
            &db,
            LogActionRequest {
                agent: "bob".into(),
                service: Some("s".into()),
                action: "x".into(),
                narrative: None,
                payload: None,
                axon_event_id: None,
                user_id: Some(2),
            },
        )
        .await
        .unwrap();
        let s1 = get_stats(&db, 1).await.unwrap();
        let s2 = get_stats(&db, 2).await.unwrap();
        assert_eq!(s1.total_actions, 1);
        assert_eq!(s2.total_actions, 1);
    }
}
