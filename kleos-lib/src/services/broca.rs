//! Broca service: action logging, template-based narration, and LLM narration.
//!
//! The primary data store is `broca_actions`. `log_action` inserts a row and
//! optionally auto-generates a `narrative` using `narrate_from_template` when
//! the caller does not supply one. The template table mirrors the JavaScript
//! narrator in `Ghost-Frame/broca/src/narrator.ts`.
//!
//! For actions that have no stored narrative and no matching template,
//! `llm_narrate` calls a configured LLM endpoint (OpenAI-compatible or a
//! generic `{prompt, system}` proxy) to produce a short English sentence.
//! `get_or_narrate_action` combines the fetch-and-persist flow for use by
//! the HTTP handlers.

use crate::db::Database;
use crate::services::axon::publish_internal;
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

/// Per-category count breakdown used inside stats responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatBreakdown {
    pub name: String,
    pub count: i64,
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
    #[serde(default)]
    pub by_service: Vec<StatBreakdown>,
    #[serde(default)]
    pub by_agent: Vec<StatBreakdown>,
    #[serde(default)]
    pub by_action: Vec<StatBreakdown>,
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
    (
        "task.created",
        "{{agent}} started a new task: \"{{title}}\" in {{project}}",
    ),
    ("task.updated", "\"{{title}}\" status is now {{status}}"),
    ("task.completed", "\"{{title}}\" was completed by {{agent}}"),
    ("task.blocked", "\"{{title}}\" is blocked: {{reason}}"),
    (
        "task.blocked_on_human",
        "\"{{title}}\" is waiting for human approval: {{summary}}",
    ),
    (
        "task.feedback",
        "Human feedback on \"{{title}}\": \"{{feedback}}\"",
    ),
    ("task.output", "Output submitted for \"{{title}}\""),
    ("task.plan", "A plan was generated for \"{{title}}\""),
    // ---- Loom / workflows ----
    (
        "workflow.run.created",
        "{{agent}} started the \"{{workflow}}\" workflow",
    ),
    (
        "workflow.run.completed",
        "The \"{{workflow}}\" workflow finished successfully",
    ),
    (
        "workflow.run.failed",
        "The \"{{workflow}}\" workflow failed on step \"{{failed_step}}\": {{error}}",
    ),
    (
        "workflow.run.cancelled",
        "The \"{{workflow}}\" workflow was cancelled",
    ),
    (
        "workflow.step.started",
        "Step \"{{step}}\" started in the \"{{workflow}}\" workflow",
    ),
    (
        "workflow.step.completed",
        "Step \"{{step}}\" finished in the \"{{workflow}}\" workflow",
    ),
    (
        "workflow.step.failed",
        "Step \"{{step}}\" failed in the \"{{workflow}}\" workflow: {{error}}",
    ),
    // ---- Soma / agents ----
    ("agent.registered", "{{name}} came online as a {{type}}"),
    ("agent.deregistered", "{{name}} went offline"),
    ("agent.online", "{{agent}} is online"),
    ("agent.offline", "{{agent}} went offline"),
    ("agent.heartbeat", "{{agent}} checked in"),
    ("agent.error", "{{agent}} reported an error: {{error}}"),
    // ---- Kleos / memory ----
    ("memory.stored", "{{source}} stored a memory ({{category}})"),
    (
        "memory.searched",
        "{{agent}} searched memory for \"{{query}}\"",
    ),
    ("memory.linked", "Two memories were linked together"),
    ("memory.forgotten", "A memory was removed"),
    // ---- Thymus / evaluations ----
    (
        "evaluation.completed",
        "{{agent}}'s work on \"{{subject}}\" was evaluated using the {{rubric}} rubric",
    ),
    (
        "metric.recorded",
        "{{agent}} recorded {{metric}}: {{value}}",
    ),
    // ---- Axon / system ----
    ("system.started", "{{service}} started up"),
    ("system.stopped", "{{service}} shut down"),
    ("deploy.started", "Deployment started for {{service}}"),
    ("deploy.succeeded", "{{service}} deployed successfully"),
    (
        "deploy.failed",
        "Deployment failed for {{service}}: {{error}}",
    ),
    ("deploy.rolled_back", "{{service}} was rolled back"),
    ("alert.triggered", "Alert triggered: {{message}}"),
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
    let mut entry = get_action(db, id, user_id).await?;

    if let Ok(axon_id) = publish_internal(
        db,
        "system",
        "broca",
        "broca.action.logged",
        serde_json::json!({
            "action_id": entry.id,
            "agent": &entry.agent,
            "service": &entry.service,
            "action": &entry.action,
        }),
    )
    .await
    {
        let action_id = entry.id;
        let _ = db
            .write(move |conn| {
                conn.execute(
                    "UPDATE broca_actions SET axon_event_id = ?1 WHERE id = ?2",
                    rusqlite::params![axon_id, action_id],
                )
                .map_err(rusqlite_to_eng_error)?;
                Ok(())
            })
            .await;
        entry.axon_event_id = Some(axon_id);
    }

    Ok(entry)
}

/// Query `broca_actions` with optional filters for agent, service, action
/// type, and an ISO-8601 lower bound on `created_at`. Results are returned
/// newest-first with pagination via `limit`/`offset`.
///
/// When `since` is `Some`, only rows whose `created_at` is lexicographically
/// >= the supplied string are returned. The comparison is a SQLite string
/// comparison; it is correct when both the stored `created_at` and the `since`
/// value are normalized ISO-8601 strings (e.g. `"2026-05-14T00:00:00Z"`).
///
/// `user_id` is always applied as a WHERE filter so cross-tenant reads on
/// the monolith path return no rows; on tenant-sharded paths the predicate
/// is a no-op. Regression tests `query_is_scoped_by_user` and
/// `get_stats_is_scoped_by_user` guard the behavior.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip(db), fields(agent = ?agent, service = ?service, action = ?action, since = ?since, limit, offset, user_id))]
pub async fn query_actions(
    db: &Database,
    agent: Option<&str>,
    service: Option<&str>,
    action: Option<&str>,
    since: Option<&str>,
    limit: usize,
    offset: usize,
    user_id: i64,
) -> Result<Vec<ActionEntry>> {
    let mut sql = format!("SELECT {ACTION_COLUMNS} FROM broca_actions WHERE user_id = ?1");
    let mut params_vec: Vec<rusqlite::types::Value> =
        vec![rusqlite::types::Value::Integer(user_id)];
    let mut param_idx = 2usize;

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
    if let Some(since_val) = since {
        sql.push_str(&format!(" AND created_at >= ?{}", param_idx));
        params_vec.push(rusqlite::types::Value::Text(since_val.to_string()));
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

/// Fetch a single [`ActionEntry`] by primary key, scoped to `user_id`.
///
/// Returns [`EngError::NotFound`] when no row with the given `id` exists or
/// the row belongs to another tenant.
#[tracing::instrument(skip(db), fields(action_id = id, user_id))]
pub async fn get_action(db: &Database, id: i64, user_id: i64) -> Result<ActionEntry> {
    let sql = format!("SELECT {ACTION_COLUMNS} FROM broca_actions WHERE id = ?1 AND user_id = ?2");

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![id, user_id])
            .map_err(rusqlite_to_eng_error)?;
        let row = rows
            .next()
            .map_err(rusqlite_to_eng_error)?
            .ok_or_else(|| EngError::NotFound(format!("action {}", id)))?;
        row_to_action_entry(row)
    })
    .await
}

/// Return aggregate [`BrocaStats`] scoped to `user_id`.
///
/// Filters by `user_id` so every tenant sees its own counts on both the
/// monolith and sharded paths. Without the predicate the monolith path
/// would return a workspace-wide aggregate.
#[tracing::instrument(skip(db), fields(user_id))]
pub async fn get_stats(db: &Database, user_id: i64) -> Result<BrocaStats> {
    db.read(move |conn| {
        let (total_actions, agents, services) = conn
            .query_row(
                "SELECT COUNT(*), COUNT(DISTINCT agent), COUNT(DISTINCT service)
                 FROM broca_actions WHERE user_id = ?1",
                rusqlite::params![user_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .map_err(rusqlite_to_eng_error)?;

        // by_service
        let mut by_service = Vec::new();
        let mut stmt = conn
            .prepare(
                "SELECT service, COUNT(*) as cnt FROM broca_actions \
                 WHERE user_id = ?1 GROUP BY service ORDER BY cnt DESC LIMIT 20",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![user_id])
            .map_err(rusqlite_to_eng_error)?;
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            by_service.push(StatBreakdown {
                name: row.get(0).map_err(rusqlite_to_eng_error)?,
                count: row.get(1).map_err(rusqlite_to_eng_error)?,
            });
        }

        // by_agent
        let mut by_agent = Vec::new();
        let mut stmt = conn
            .prepare(
                "SELECT agent, COUNT(*) as cnt FROM broca_actions \
                 WHERE user_id = ?1 GROUP BY agent ORDER BY cnt DESC LIMIT 20",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![user_id])
            .map_err(rusqlite_to_eng_error)?;
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            by_agent.push(StatBreakdown {
                name: row.get(0).map_err(rusqlite_to_eng_error)?,
                count: row.get(1).map_err(rusqlite_to_eng_error)?,
            });
        }

        // by_action
        let mut by_action = Vec::new();
        let mut stmt = conn
            .prepare(
                "SELECT action, COUNT(*) as cnt FROM broca_actions \
                 WHERE user_id = ?1 GROUP BY action ORDER BY cnt DESC LIMIT 20",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![user_id])
            .map_err(rusqlite_to_eng_error)?;
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            by_action.push(StatBreakdown {
                name: row.get(0).map_err(rusqlite_to_eng_error)?,
                count: row.get(1).map_err(rusqlite_to_eng_error)?,
            });
        }

        Ok(BrocaStats {
            total_actions,
            agents,
            services,
            by_service,
            by_agent,
            by_action,
        })
    })
    .await
}

// ---------------------------------------------------------------------------
// LLM narration
// ---------------------------------------------------------------------------

/// Shared HTTP client for LLM narration calls. Allocated once at first use.
/// 60-second timeout mirrors the standalone's `AbortSignal.timeout(60000)`.
static BROCA_LLM_CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
    crate::net::safe_client_builder()
        .timeout(std::time::Duration::from_secs(60))
        .pool_max_idle_per_host(2)
        .build()
        // safe_client_builder only fails if the TLS backend is broken, which
        // is a startup-fatal condition. `expect` is acceptable here.
        .expect("BROCA_LLM_CLIENT build failed")
});

/// Resolve the LLM endpoint URL.
///
/// Checks `BROCA_LLM_URL` first, then falls back to `LLM_URL`. Returns `None`
/// when neither is set, signalling the caller to use the template fallback.
fn broca_llm_url() -> Option<String> {
    std::env::var("BROCA_LLM_URL")
        .or_else(|_| std::env::var("LLM_URL"))
        .ok()
        .filter(|s| !s.is_empty())
}

/// Resolve the LLM bearer token.
///
/// Checks `BROCA_LLM_API_KEY` first, then falls back to `LLM_API_KEY`.
fn broca_llm_api_key() -> Option<String> {
    std::env::var("BROCA_LLM_API_KEY")
        .or_else(|_| std::env::var("LLM_API_KEY"))
        .ok()
        .filter(|s| !s.is_empty())
}

/// Resolve the model name to request.
///
/// Checks `BROCA_LLM_MODEL` first, then falls back to `LLM_MODEL`, then uses
/// the hardcoded default `"qwen2.5:14b"` matching the standalone's default.
fn broca_llm_model() -> String {
    std::env::var("BROCA_LLM_MODEL")
        .or_else(|_| std::env::var("LLM_MODEL"))
        .unwrap_or_else(|_| "qwen2.5:14b".to_string())
}

/// OpenAI-compatible chat completions request body.
#[derive(Debug, serde::Serialize)]
struct OpenAiRequest {
    /// Model identifier passed through to the backend.
    model: String,
    /// Ordered list of chat messages (system + user).
    messages: Vec<OpenAiMessage>,
    /// Sampling temperature; 0.3 matches the standalone.
    temperature: f64,
    /// Disable streaming so the response arrives as a single JSON object.
    stream: bool,
}

/// A single message in an OpenAI-compatible chat request.
#[derive(Debug, serde::Serialize)]
struct OpenAiMessage {
    /// Role: `"system"` or `"user"`.
    role: String,
    /// Message text.
    content: String,
}

/// Generic `{prompt, system}` LLM request body.
///
/// Used when `LLM_URL` does not contain `/v1/chat` or `/chat/completions` and
/// is not an Ollama port. Matches the Kleos `/llm` internal endpoint.
#[derive(Debug, serde::Serialize)]
struct GenericLlmRequest {
    /// User message / prompt.
    prompt: String,
    /// System instruction.
    system: String,
}

/// Generate a narrative for a stored action via LLM.
///
/// Used as a fallback when no template matched at ingest. Returns a short,
/// human-readable past-tense sentence describing what the agent did.
///
/// This function is **infallible**: every error path (no URL configured,
/// network failure, unexpected response shape) logs a warning via
/// [`tracing::warn!`] and returns the template fallback string
/// `"{agent} performed {action}"` instead of propagating an error.
/// Callers can therefore use the return value directly without `?`.
///
/// Endpoint detection:
/// - URLs containing `/v1/chat`, `/chat/completions`, or port `11434` (Ollama)
///   are treated as OpenAI-compatible; `/v1/chat/completions` is appended to
///   raw Ollama base URLs if not already present.
/// - All other URLs are treated as generic `{prompt, system}` endpoints.
///
/// Env vars (first match wins):
/// - URL:   `BROCA_LLM_URL` -> `LLM_URL`
/// - Key:   `BROCA_LLM_API_KEY` -> `LLM_API_KEY`
/// - Model: `BROCA_LLM_MODEL` -> `LLM_MODEL` -> `"qwen2.5:14b"`
#[tracing::instrument(skip(payload), fields(agent, service, action))]
pub async fn llm_narrate(
    agent: &str,
    service: &str,
    action: &str,
    payload: &serde_json::Value,
) -> String {
    let fallback = format!("{agent} performed {action}");

    let Some(url_base) = broca_llm_url() else {
        tracing::debug!("BROCA_LLM_URL/LLM_URL not set; using fallback narrative");
        return fallback;
    };

    let model = broca_llm_model();

    let system = "You translate technical agent actions into plain English. One sentence only.";
    let user_prompt = format!(
        "Convert this agent action into a single plain English sentence a non-technical person \
         would understand. Be concise and natural. No technical jargon, no IDs, no JSON terms.\n\n\
         Agent: {agent}\n\
         Service: {service}\n\
         Action: {action}\n\
         Details: {payload}\n\n\
         Respond with only the sentence, nothing else.",
        payload = serde_json::to_string_pretty(payload).unwrap_or_else(|_| payload.to_string()),
    );

    // Detect endpoint style -- mirrors narrator.ts detection logic.
    let is_ollama_or_openai_compat = url_base.contains("11434")
        || url_base.contains("ollama")
        || url_base.contains("/v1/chat")
        || url_base.contains("/chat/completions");

    let result = if is_ollama_or_openai_compat {
        // OpenAI-compat path: ensure the URL ends with /v1/chat/completions.
        let url = if url_base.contains("/v1/chat/completions")
            || url_base.contains("/chat/completions")
        {
            url_base.clone()
        } else {
            // Strip trailing slash and append the OpenAI completions path.
            format!("{}/v1/chat/completions", url_base.trim_end_matches('/'))
        };

        let body = OpenAiRequest {
            model,
            messages: vec![
                OpenAiMessage {
                    role: "system".to_string(),
                    content: system.to_string(),
                },
                OpenAiMessage {
                    role: "user".to_string(),
                    content: user_prompt,
                },
            ],
            temperature: 0.3,
            stream: false,
        };

        call_llm_endpoint(&url, body, broca_llm_api_key()).await
    } else {
        // Generic `{prompt, system}` endpoint.
        let body = GenericLlmRequest {
            prompt: user_prompt,
            system: system.to_string(),
        };
        call_llm_endpoint(&url_base, body, broca_llm_api_key()).await
    };

    match result {
        Ok(raw) => {
            // Strip whitespace and cap at 280 Unicode scalar values.
            let trimmed = raw.trim();
            let capped = if trimmed.chars().count() > 280 {
                // Find the byte offset immediately after the 280th char so the
                // slice is a valid UTF-8 boundary.
                let end = trimmed
                    .char_indices()
                    .nth(280)
                    .map(|(i, _)| i)
                    .unwrap_or(trimmed.len());
                trimmed[..end].to_string()
            } else {
                trimmed.to_string()
            };
            if capped.is_empty() {
                fallback
            } else {
                capped
            }
        }
        Err(e) => {
            tracing::warn!("LLM narration failed: {}; using fallback", e);
            fallback
        }
    }
}

/// Send a serializable body to `url` and extract the text content from the
/// response.
///
/// Parses the response body into a raw [`serde_json::Value`] first, then
/// attempts extraction in priority order:
/// 1. `choices[0].message.content` -- OpenAI-compatible completions shape.
/// 2. `result` -- Generic single-field shape.
/// 3. `text` -- alternate local-proxy shape.
/// 4. `content` -- alternate local-proxy shape.
///
/// This permissive extraction tolerates generic `{prompt, system}` endpoints that do not
/// conform to the OpenAI schema without failing the typed deserialize step.
///
/// Returns an `Err` on network failure or when no recognisable text field is
/// present, so the caller can decide whether to fall back gracefully.
pub(crate) async fn call_llm_endpoint<B: serde::Serialize>(
    url: &str,
    body: B,
    api_key: Option<String>,
) -> std::result::Result<String, String> {
    let mut req = BROCA_LLM_CLIENT.post(url).json(&body);
    if let Some(key) = api_key {
        req = req.bearer_auth(key);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("LLM request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body_text = resp
            .text()
            .await
            .unwrap_or_else(|e| format!("<body read error: {e}>"));
        return Err(format!("LLM returned {status}: {body_text}"));
    }

    // Parse into a generic Value to tolerate both OpenAI-compat and
    // generic response shapes without a rigid typed deserialize.
    let val: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("LLM response parse error: {e}"))?;

    // OpenAI-compat: choices[0].message.content
    let from_choices = val
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|s| s.as_str())
        .map(str::to_owned);

    // Generic / local-proxy fallback fields.
    let from_flat = from_choices.or_else(|| {
        ["result", "text", "content"]
            .iter()
            .find_map(|key| val.get(key).and_then(|v| v.as_str()).map(str::to_owned))
    });

    from_flat.ok_or_else(|| "LLM response contained no recognisable text field".to_string())
}

/// Fetch the action by id, scoped to the given tenant. If it already has a
/// stored narrative, return it unchanged. Otherwise call [`llm_narrate`],
/// persist the result via UPDATE, and return the freshly-generated narrative.
///
/// Returns `Ok(None)` when no action with `action_id` owned by `user_id`
/// exists, so the HTTP handler can translate that to a 404 without this
/// function knowing about HTTP semantics. Actions belonging to other tenants
/// are indistinguishable from missing actions -- they return `Ok(None)`.
#[tracing::instrument(skip(db), fields(action_id, user_id))]
pub async fn get_or_narrate_action(
    db: &Database,
    action_id: i64,
    user_id: i64,
) -> Result<Option<String>> {
    // Attempt to load the action; propagate DB errors but convert NotFound to None.
    // The user_id scope ensures cross-tenant reads return None rather than data.
    let entry = match get_action_for_narrate(db, action_id, user_id).await {
        Ok(Some(e)) => e,
        Ok(None) => return Ok(None),
        Err(e) => return Err(e),
    };

    // Fast path: narrative already stored.
    if let Some(ref n) = entry.narrative {
        return Ok(Some(n.clone()));
    }

    // Slow path: call LLM and persist. llm_narrate is infallible -- it returns
    // a fallback string rather than an error, so no ? is needed here.
    let narrative = llm_narrate(&entry.agent, &entry.service, &entry.action, &entry.payload).await;
    let narrative_clone = narrative.clone();

    db.write(move |conn| {
        conn.execute(
            "UPDATE broca_actions SET narrative = ?1 WHERE id = ?2",
            rusqlite::params![narrative_clone, action_id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await?;

    Ok(Some(narrative))
}

/// Internal helper: fetch a single [`ActionEntry`] by id, scoped to the given
/// tenant. Returns `Ok(None)` when absent or owned by a different tenant, so
/// [`get_or_narrate_action`] can distinguish "not found / not yours" from a
/// real DB error without decoding error strings.
///
/// The `AND user_id = ?2` clause enforces tenant isolation: a caller cannot
/// trigger LLM narration or read the narrative for a row they do not own.
async fn get_action_for_narrate(
    db: &Database,
    action_id: i64,
    user_id: i64,
) -> Result<Option<ActionEntry>> {
    let sql = format!("SELECT {ACTION_COLUMNS} FROM broca_actions WHERE id = ?1 AND user_id = ?2");

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![action_id, user_id])
            .map_err(rusqlite_to_eng_error)?;
        match rows.next().map_err(rusqlite_to_eng_error)? {
            Some(row) => row_to_action_entry(row).map(Some),
            None => Ok(None),
        }
    })
    .await
}

// ---------------------------------------------------------------------------
// Natural-language ask pipeline
// ---------------------------------------------------------------------------

/// Query plan extracted by the LLM from the user's question.
///
/// All fields are optional; absent fields mean "no filter" for that dimension.
/// Produced by the plan-call phase of [`ask`] and returned verbatim to the
/// caller so they can inspect what the LLM decided to query.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AskPlan {
    /// Filter actions to this agent identifier.
    pub agent: Option<String>,
    /// Filter actions to this service name.
    pub service: Option<String>,
    /// ISO-8601 lower bound on `created_at`, e.g. `"2026-05-14T00:00:00Z"`.
    /// Compared as a string against the SQLite-stored `created_at`, so the
    /// format must match the stored format. The LLM is instructed to return
    /// this shape; the keyword heuristic does not emit `since`.
    pub since: Option<String>,
    /// Maximum number of action rows to retrieve. Clamped to 50; defaults to 20.
    pub limit: Option<u32>,
}

/// A single action row in the `raw` array returned by [`ask`].
///
/// Intentionally narrower than [`ActionEntry`]: only the fields the caller
/// needs for display and citation are included. The heavy `payload` blob is
/// excluded to keep response sizes small.
#[derive(Debug, Clone, Serialize)]
pub struct AskRow {
    /// Action primary key.
    pub id: i64,
    /// Agent that performed the action.
    pub agent: String,
    /// Service the action was logged against.
    pub service: String,
    /// Action type string.
    pub action: String,
    /// Human-readable narrative; `None` when none was generated at log time.
    pub narrative: Option<String>,
    /// ISO-8601 UTC timestamp of insertion.
    pub created_at: String,
}

/// Final response for `POST /broca/ask`.
///
/// Contains the synthesized plain-English `answer`, the `plan` the LLM (or
/// heuristic) produced, and the `raw` action rows that were passed to the
/// summarizer. Callers can use `plan` and `raw` for debugging; `answer` is
/// the primary user-facing output.
#[derive(Debug, Clone, Serialize)]
pub struct AskResult {
    /// Synthesized 1-3 sentence answer with action-id citations.
    pub answer: String,
    /// Query plan extracted from the question.
    pub plan: AskPlan,
    /// Matched action rows that were used to produce `answer`.
    pub raw: Vec<AskRow>,
}

/// Run the plan-then-summarize natural-language query pipeline.
///
/// Returns a structured [`AskResult`] regardless of LLM availability.
/// If `BROCA_LLM_URL`/`LLM_URL` is unset, the plan falls back to a
/// keyword-extraction heuristic and the summary falls back to a
/// concatenation of the matched narratives.
///
/// The function therefore never returns `Err` for LLM-related reasons.
/// It propagates `Err` only for database failures, which the HTTP handler
/// translates to 500.
///
/// # Data flow to the configured LLM
///
/// Both LLM calls send tenant data to whatever endpoint is configured
/// in `BROCA_LLM_URL` (or `LLM_URL`):
///
/// - **Plan call:** the user's question text only.
/// - **Summarize call:** the question plus a compact list of matched
///   action rows containing `id`, `agent`, `service`, `action`,
///   `narrative`, and `created_at`. The raw `payload` blob is NOT
///   forwarded.
///
/// `narrative` is template- or LLM-rendered text derived from action
/// payloads at log time. If your action payloads can carry sensitive
/// data, the narratives derived from them may carry it too. Configure
/// `BROCA_LLM_URL` to a local or on-prem endpoint when handling
/// sensitive tenant data; do not point it at a third-party service
/// you do not control.
#[tracing::instrument(skip(db), fields(user_id, question_len = question.len()))]
pub async fn ask(db: &Database, user_id: i64, question: &str) -> Result<AskResult> {
    // --- Step 1: derive the query plan ---
    let plan = ask_plan_call(question).await;
    tracing::debug!(?plan, "ask: query plan resolved");

    // --- Step 2: dispatch to appropriate service ---
    let raw = ask_dispatch(db, &plan, user_id).await?;

    // --- Step 3: summarize ---
    let answer = ask_summarize_call(question, &raw).await;
    tracing::debug!(answer_len = answer.len(), "ask: summary resolved");

    Ok(AskResult { answer, plan, raw })
}

/// Dispatch to the appropriate service based on the plan's service field.
/// Returns a vec of AskRow -- a common format all services are projected into.
async fn ask_dispatch(db: &Database, plan: &AskPlan, user_id: i64) -> Result<Vec<AskRow>> {
    let limit = plan.limit.unwrap_or(20).min(50) as usize;
    let agent = plan.agent.as_deref();

    match plan.service.as_deref() {
        Some("soma") => {
            let agents = crate::services::soma::list_agents(db, user_id, None, None, limit).await?;
            Ok(agents
                .iter()
                .map(|a| AskRow {
                    id: a.id,
                    agent: a.name.clone(),
                    service: "soma".to_string(),
                    action: format!("agent.{}", a.status),
                    narrative: a.description.clone(),
                    created_at: a.created_at.clone(),
                })
                .collect())
        }
        Some("chiasm") => {
            let tasks =
                crate::services::chiasm::list_tasks(db, user_id, None, agent, None, limit, 0)
                    .await?;
            Ok(tasks
                .iter()
                .map(|t| AskRow {
                    id: t.id,
                    agent: t.agent.clone(),
                    service: "chiasm".to_string(),
                    action: format!("task.{}", t.status),
                    narrative: Some(t.title.clone()),
                    created_at: t.created_at.clone(),
                })
                .collect())
        }
        Some("thymus") => {
            let evals = crate::services::thymus::list_evaluations(db, agent, None, limit).await?;
            Ok(evals
                .iter()
                .map(|e| AskRow {
                    id: e.id,
                    agent: e.agent.clone(),
                    service: "thymus".to_string(),
                    action: "evaluation.completed".to_string(),
                    narrative: Some(format!("{} scored {:.2}", e.subject, e.overall_score)),
                    created_at: e.created_at.clone(),
                })
                .collect())
        }
        Some("axon") => {
            let events =
                crate::services::axon::query_events(db, None, None, None, limit, 0, user_id)
                    .await?;
            Ok(events
                .iter()
                .map(|ev| AskRow {
                    id: ev.id,
                    agent: ev.agent.clone().unwrap_or_default(),
                    service: "axon".to_string(),
                    action: ev.action.clone(),
                    narrative: Some(format!("[{}] {}", ev.channel, ev.action)),
                    created_at: ev.created_at.clone(),
                })
                .collect())
        }
        Some("loom") => {
            let runs = crate::services::loom::list_runs(db, None, None, limit).await?;
            Ok(runs
                .iter()
                .map(|r| AskRow {
                    id: r.id,
                    agent: String::new(),
                    service: "loom".to_string(),
                    action: format!("workflow.run.{}", r.status),
                    narrative: Some(format!("run {} (workflow {})", r.id, r.workflow_id)),
                    created_at: r.created_at.clone(),
                })
                .collect())
        }
        _ => {
            // Default: query broca_actions
            let rows = query_actions(
                db,
                agent,
                plan.service.as_deref(),
                None,
                plan.since.as_deref(),
                limit,
                0,
                user_id,
            )
            .await?;
            Ok(rows
                .iter()
                .map(|e| AskRow {
                    id: e.id,
                    agent: e.agent.clone(),
                    service: e.service.clone(),
                    action: e.action.clone(),
                    narrative: e.narrative.clone(),
                    created_at: e.created_at.clone(),
                })
                .collect())
        }
    }
}

/// Map an LLM error string to a coarse `&'static str` category for logging.
///
/// Response bodies returned by some LLM providers can echo parts of the
/// original request (including Authorization headers) back in error text.
/// This function ensures only a short category label reaches the log, never
/// the raw body.
///
/// Categories: `"network"`, `"non-2xx status"`, `"parse"`, `"empty response"`.
fn scrub_llm_error(e: &str) -> &'static str {
    if e.contains("request failed") || e.contains("connection") || e.contains("timeout") {
        "network"
    } else if e.contains("returned") && e.contains(':') {
        // Matches the "LLM returned <STATUS>: <body>" pattern from call_llm_endpoint.
        "non-2xx status"
    } else if e.contains("parse") || e.contains("deserializ") {
        "parse"
    } else {
        "empty response"
    }
}

/// Produce an [`AskPlan`] for `question` via an LLM call.
///
/// The system prompt instructs the LLM to return a JSON object with the four
/// optional fields (`agent`, `service`, `since`, `limit`) and nothing else.
/// If the LLM is unavailable or returns non-JSON, the function falls back
/// to [`ask_keyword_heuristic`] so the pipeline always makes progress.
async fn ask_plan_call(question: &str) -> AskPlan {
    let Some(url_base) = broca_llm_url() else {
        tracing::debug!("ask: LLM not configured, using keyword heuristic for plan");
        return ask_keyword_heuristic(question);
    };

    let system = "You translate a user question about an agent system into a JSON query plan. \
        Return ONLY a JSON object with these optional fields and nothing else -- no explanation, \
        no markdown, no code fences:\n\
        {\"agent\":\"<agent-name>\",\"service\":\"<service-name>\",\
        \"since\":\"<ISO-8601-datetime>\",\"limit\":<integer 1-50>}\n\n\
        SERVICE CATALOG (set `service` to route to the right data source):\n\
        - broca: action logs, what agents did, activity history (DEFAULT)\n\
        - soma: agent registry, who is online, agent status, capabilities\n\
        - chiasm: task coordination, assignments, task status, blockers\n\
        - thymus: evaluations, quality scores, rubrics, drift detection\n\
        - axon: events, channels, pub/sub activity\n\
        - loom: workflows, runs, step execution, orchestration\n\n\
        Rules:\n\
        - Omit fields that are not implied by the question.\n\
        - Set `service` to the most relevant data source for the question.\n\
        - If the question is about agent activity or \"what did X do\", use broca (default).\n\
        - For time-based questions (\"today\", \"last hour\", \"recent\") omit `since` and use a \
          reasonable limit instead.\n\
        - Default limit is 20. Maximum is 50.\n\
        - If unsure which service, omit `service` (defaults to broca).";

    let model = broca_llm_model();

    // Detect endpoint style -- reuses the same logic as llm_narrate.
    let is_openai_compat = url_base.contains("11434")
        || url_base.contains("ollama")
        || url_base.contains("/v1/chat")
        || url_base.contains("/chat/completions");

    let result: std::result::Result<String, String> = if is_openai_compat {
        let url = if url_base.contains("/v1/chat/completions")
            || url_base.contains("/chat/completions")
        {
            url_base.clone()
        } else {
            format!("{}/v1/chat/completions", url_base.trim_end_matches('/'))
        };
        let body = OpenAiRequest {
            model,
            messages: vec![
                OpenAiMessage {
                    role: "system".to_string(),
                    content: system.to_string(),
                },
                OpenAiMessage {
                    role: "user".to_string(),
                    content: question.to_string(),
                },
            ],
            temperature: 0.2,
            stream: false,
        };
        call_llm_endpoint(&url, body, broca_llm_api_key()).await
    } else {
        let body = GenericLlmRequest {
            prompt: question.to_string(),
            system: system.to_string(),
        };
        call_llm_endpoint(&url_base, body, broca_llm_api_key()).await
    };

    match result {
        Ok(raw_text) => {
            // Extract the first JSON object from the response, tolerating
            // models that wrap output in markdown fences.
            if let Some(json_match) = raw_text
                .find('{')
                .and_then(|start| raw_text.rfind('}').map(|end| &raw_text[start..=end]))
            {
                match serde_json::from_str::<AskPlan>(json_match) {
                    Ok(plan) => {
                        // Clamp limit here so the plan stored in the response
                        // reflects the actual clamped value.
                        AskPlan {
                            limit: plan.limit.map(|l| l.min(50)),
                            ..plan
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            "ask: plan JSON parse failed ({}); falling back to heuristic",
                            e
                        );
                        ask_keyword_heuristic(question)
                    }
                }
            } else {
                tracing::warn!(
                    "ask: LLM returned no JSON object in plan response; using heuristic"
                );
                ask_keyword_heuristic(question)
            }
        }
        Err(e) => {
            // Only log the error category, not the body, which could echo headers.
            tracing::warn!(
                "ask: plan LLM call failed ({}); using heuristic",
                scrub_llm_error(&e)
            );
            ask_keyword_heuristic(question)
        }
    }
}

/// Derive an [`AskPlan`] from `question` without an LLM call.
///
/// Scans the question text for known agent and service name keywords. This
/// heuristic is intentionally simple: it produces a usable plan when the LLM
/// is unavailable rather than a precise one. The limit defaults to 20.
fn ask_keyword_heuristic(question: &str) -> AskPlan {
    let lower = question.to_lowercase();

    // Known service names to look for in the question.
    let known_services = ["kleos", "chiasm", "axon", "loom", "soma", "thymus", "broca"];

    let service = known_services
        .iter()
        .find(|&&s| lower.contains(s))
        .map(|s| s.to_string());

    AskPlan {
        agent: None,
        service,
        since: None,
        limit: Some(20),
    }
}

/// Synthesize a plain-English answer from `question` and matched `rows`.
///
/// Calls the LLM with the question and a compact JSON summary of `rows`, asking
/// for a 1-3 sentence answer that cites action ids where relevant.
///
/// Fallback (LLM unavailable or error): joins the non-empty narratives from
/// `rows` into a semicolon-delimited string. Returns an empty string when
/// `rows` is empty and the LLM is unavailable.
async fn ask_summarize_call(question: &str, rows: &[AskRow]) -> String {
    // Compact row representation for the prompt: id, agent, action, narrative.
    let compact: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "agent": r.agent,
                "action": r.action,
                "narrative": r.narrative.as_deref().unwrap_or(""),
            })
        })
        .collect();

    let narrative_fallback = || -> String {
        let parts: Vec<&str> = rows
            .iter()
            .filter_map(|r| r.narrative.as_deref())
            .filter(|n| !n.is_empty())
            .collect();
        parts.join("; ")
    };

    let Some(url_base) = broca_llm_url() else {
        tracing::debug!("ask: LLM not configured, using narrative concatenation for summary");
        return narrative_fallback();
    };

    let model = broca_llm_model();
    let system = "You answer questions about an AI agent activity log. Be concise and direct. \
        Cite relevant action ids in parentheses, e.g. (id:42). Answer in 1-3 sentences only.";

    // Truncate rows list at 4 096 bytes to keep prompt size bounded.
    // `floor_char_boundary` would be ideal but is nightly-only; instead find
    // the last char boundary that keeps the byte offset strictly within 4 096.
    // serde_json output is valid UTF-8, so this slice is always well-formed.
    const ROWS_CAP: usize = 4096;
    let rows_json = serde_json::to_string(&compact).unwrap_or_else(|_| "[]".to_string());
    let rows_excerpt: &str = if rows_json.len() > ROWS_CAP {
        // Walk char boundaries; stop when the *next* character would start at
        // or beyond ROWS_CAP, giving us the largest valid prefix within the cap.
        let end = rows_json
            .char_indices()
            .take_while(|(byte_pos, _)| *byte_pos < ROWS_CAP)
            .last()
            .map(|(byte_pos, ch)| byte_pos + ch.len_utf8())
            .unwrap_or(0);
        &rows_json[..end]
    } else {
        &rows_json
    };

    let user_prompt = format!(
        "Question: {question}\n\nMatched action log rows (JSON):\n{rows_excerpt}\n\nAnswer:"
    );

    let is_openai_compat = url_base.contains("11434")
        || url_base.contains("ollama")
        || url_base.contains("/v1/chat")
        || url_base.contains("/chat/completions");

    let result: std::result::Result<String, String> = if is_openai_compat {
        let url = if url_base.contains("/v1/chat/completions")
            || url_base.contains("/chat/completions")
        {
            url_base.clone()
        } else {
            format!("{}/v1/chat/completions", url_base.trim_end_matches('/'))
        };
        let body = OpenAiRequest {
            model,
            messages: vec![
                OpenAiMessage {
                    role: "system".to_string(),
                    content: system.to_string(),
                },
                OpenAiMessage {
                    role: "user".to_string(),
                    content: user_prompt,
                },
            ],
            temperature: 0.3,
            stream: false,
        };
        call_llm_endpoint(&url, body, broca_llm_api_key()).await
    } else {
        let body = GenericLlmRequest {
            prompt: user_prompt,
            system: system.to_string(),
        };
        call_llm_endpoint(&url_base, body, broca_llm_api_key()).await
    };

    match result {
        Ok(text) => {
            let trimmed = text.trim().to_string();
            if trimmed.is_empty() {
                narrative_fallback()
            } else {
                trimmed
            }
        }
        Err(e) => {
            // Only log the error category, not the body, which could echo headers.
            tracing::warn!(
                "ask: summarize LLM call failed ({}); using narrative fallback",
                scrub_llm_error(&e)
            );
            narrative_fallback()
        }
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    /// Create an in-memory `Database` with all migrations applied.
    /// Used by every test in this module.
    async fn setup() -> Database {
        let db = Database::connect_memory().await.expect("db");
        // Apply monolith migrations so broca_actions exists with user_id (v45).
        db.write(|conn| crate::db::migrations::run_migrations(conn))
            .await
            .expect("migrations");
        db
    }

    /// Verify that `log_action` inserts a row and `get_action` retrieves it.
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
        let other = query_actions(&db, None, None, None, None, 10, 0, 2)
            .await
            .unwrap();
        assert!(other.is_empty(), "user 2 must not see user 1's actions");
        let mine = query_actions(&db, None, None, None, None, 10, 0, 1)
            .await
            .unwrap();
        assert_eq!(mine.len(), 1, "user 1 should see their own row");
        assert_eq!(mine[0].user_id, 1);
    }

    /// Verify that `get_stats` counts only the rows owned by each tenant.
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
