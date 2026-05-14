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
async fn call_llm_endpoint<B: serde::Serialize>(
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
