use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use rusqlite::params;
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::{Auth, ResolvedDb};
use crate::state::AppState;
use kleos_lib::gate::{
    check_command_with_context, check_ssh_dns_rebind, cleanup_expired_approvals, complete_gate,
    mark_gate_timed_out, parse_ssh_target, read_gate_decision, respond_to_gate, store_gate_request,
    GateCheckRequest, GateCheckResult, PendingApproval, APPROVAL_TIMEOUT_SECS,
    TOOLS_REQUIRING_APPROVAL,
};

mod types;
use types::{CompleteBody, GuardBody, RespondBody};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/gate/check", post(check_handler))
        .route("/gate/respond", post(respond_handler))
        .route("/gate/complete", post(complete_handler))
        // Alias for parity with original kleos
        .route("/guard", post(guard_handler))
}

async fn check_handler(
    ResolvedDb(db): ResolvedDb,
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<GateCheckRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    // Agent allowlist: if this API key is bound to an agent record, the body's
    // declared agent must match that agent's name. Prevents one agent's key
    // being used under another agent's identity.
    if let Some(bound_id) = auth.key.agent_id {
        let expected: Option<String> = db
            .read(move |conn| {
                conn.query_row(
                    "SELECT name FROM agents WHERE id = ?1",
                    params![bound_id],
                    |row| row.get::<_, String>(0),
                )
                .map(Some)
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(kleos_lib::EngError::DatabaseMessage(other.to_string())),
                })
            })
            .await?;
        match expected {
            Some(name) if name == body.agent => {}
            Some(name) => {
                return Err(AppError::from(kleos_lib::EngError::Forbidden(format!(
                    "api key is bound to agent '{}' but request declared agent '{}'",
                    name, body.agent
                ))));
            }
            None => {
                return Err(AppError::from(kleos_lib::EngError::Forbidden(format!(
                    "api key bound to agent id {} which no longer exists",
                    bound_id
                ))));
            }
        }
    }

    let resolved_command = state
        .credd
        .resolve_text(&db, auth.user_id, &body.agent, &body.command)
        .await?;
    let mut resolved_patterns = Vec::new();
    for pattern in &state.config.eidolon.gate.blocked_patterns {
        resolved_patterns.push(
            state
                .credd
                .resolve_text(&db, auth.user_id, &body.agent, pattern)
                .await?,
        );
    }
    let mut result = check_command_with_context(
        &db,
        &body,
        auth.user_id,
        Some(&resolved_command),
        &resolved_patterns,
        &state.config,
    )
    .await?;

    // Brain-grounded gate check: if the brain is loaded, embed the resolved
    // command and ask the Hopfield network for the closest memories. If any
    // high-activation recall contains a prohibition keyword, block the
    // command and cite the rule. This mirrors the eidolon gate's semantic
    // check -- static patterns cannot express every project-specific "never".
    if result.allowed {
        if let Some(reason) = brain_grounded_check(&state, auth.user_id, &resolved_command).await {
            let gate_id = store_gate_request(
                &db,
                auth.user_id,
                &body.agent,
                &body.command,
                body.context.as_deref(),
                "blocked",
                Some(&reason),
            )
            .await?;
            let denied = GateCheckResult {
                allowed: false,
                reason: Some(reason),
                resolved_command: Some(body.command.clone()),
                gate_id,
                requires_approval: false,
                enrichment: None,
            };
            return Ok((StatusCode::CREATED, Json(json!(denied))));
        }
    }

    // DNS rebinding / SSRF defense: if the static check allowed an SSH command,
    // resolve the hostname and reject if any A/AAAA record is internal.
    if result.allowed && (resolved_command.contains("ssh ") || resolved_command.starts_with("ssh"))
    {
        if let Some(target) = parse_ssh_target(&resolved_command) {
            let port = target.port.unwrap_or(22);
            if let Some(block_reason) = check_ssh_dns_rebind(&target.host, port).await {
                let gate_id = store_gate_request(
                    &db,
                    auth.user_id,
                    &body.agent,
                    &body.command,
                    body.context.as_deref(),
                    "blocked",
                    Some(&block_reason),
                )
                .await?;
                result = GateCheckResult {
                    allowed: false,
                    reason: Some(block_reason),
                    resolved_command: Some(body.command.clone()),
                    gate_id,
                    requires_approval: false,
                    enrichment: None,
                };
                return Ok((StatusCode::CREATED, Json(json!(result))));
            }
        }
    }

    // If the command was allowed (not blocked, not pending secrets), and the tool
    // requires human approval, pause here and wait for a decision via /gate/respond.
    if result.allowed && !result.requires_approval {
        let tool_name = body.tool_name.as_deref().unwrap_or("");
        if TOOLS_REQUIRING_APPROVAL.contains(&tool_name) {
            let gate_id = result.gate_id;
            let (tx, rx) = tokio::sync::oneshot::channel::<bool>();

            {
                let mut approvals = state.pending_approvals.lock().await;
                // Prune stale entries while we have the lock.
                cleanup_expired_approvals(&mut approvals);
                approvals.insert(
                    gate_id,
                    (
                        PendingApproval {
                            gate_id,
                            agent: body.agent.clone(),
                            tool_name: tool_name.to_string(),
                            command: body.command.clone(),
                            created_at: std::time::Instant::now(),
                        },
                        tx,
                    ),
                );
            }

            // Notify any watchers (e.g. TUI) that a new approval is pending.
            if let Some(ref notify) = state.approval_notify {
                let _ = notify.send(());
            }

            let wait_outcome =
                tokio::time::timeout(std::time::Duration::from_secs(APPROVAL_TIMEOUT_SECS), rx)
                    .await;

            // SECURITY (SEC-CRIT-2): resolve the outcome against the DB, which
            // is the single source of truth. The oneshot is a wake-up hint;
            // regardless of which branch we hit, we consult the persisted
            // status so the HTTP response always matches what was written.
            {
                let mut approvals = state.pending_approvals.lock().await;
                approvals.remove(&gate_id);
            }

            let approved = match wait_outcome {
                Ok(Ok(decision)) => decision,
                _ => {
                    // Timeout or channel dropped. Atomically CAS the row to
                    // denied-timeout. If the CAS loses, a concurrent
                    // respond_handler already decided; read and honour that.
                    match mark_gate_timed_out(&db, gate_id, auth.user_id).await {
                        Ok(true) => false,
                        Ok(false) => match read_gate_decision(&db, gate_id, auth.user_id).await {
                            Ok(Some(d)) => d.status == "approved",
                            _ => false,
                        },
                        Err(e) => {
                            tracing::error!(
                                "gate: failed to mark gate_id={} timed out: {}",
                                gate_id,
                                e
                            );
                            false
                        }
                    }
                }
            };

            if approved {
                tracing::info!(
                    "gate: APPROVED by user gate_id={} tool={} agent={}",
                    gate_id,
                    tool_name,
                    body.agent
                );
                return Ok((StatusCode::CREATED, Json(json!(result))));
            } else {
                tracing::warn!(
                    "gate: DENIED/TIMEOUT gate_id={} tool={} agent={}",
                    gate_id,
                    tool_name,
                    body.agent
                );
                let denied_result = kleos_lib::gate::GateCheckResult {
                    allowed: false,
                    reason: Some(format!(
                        "{} denied -- approval timed out or rejected",
                        tool_name
                    )),
                    resolved_command: result.resolved_command.clone(),
                    gate_id,
                    requires_approval: false,
                    enrichment: None,
                };
                return Ok((StatusCode::CREATED, Json(json!(denied_result))));
            }
        }
    }

    Ok((StatusCode::CREATED, Json(json!(result))))
}

async fn respond_handler(
    ResolvedDb(db): ResolvedDb,
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<RespondBody>,
) -> Result<Json<Value>, AppError> {
    // SECURITY (SEC-CRIT-2): the DB CAS in respond_to_gate is the authoritative
    // transition. Persist first; only on success signal the waiter. If another
    // responder or the timeout path already decided, respond_to_gate returns
    // EngError::Conflict (-> 409) and we must not touch the map or the tx.
    let result = respond_to_gate(
        &db,
        body.gate_id,
        body.approved,
        body.reason.as_deref(),
        auth.user_id,
    )
    .await?;

    // DB win: best-effort wake the waiting check_handler. Failure here is not
    // fatal; the waiter's timeout path reads the persisted decision on fallback.
    {
        let mut approvals = state.pending_approvals.lock().await;
        if let Some((_, tx)) = approvals.remove(&body.gate_id) {
            let _ = tx.send(body.approved);
        }
    }

    Ok(Json(result))
}

async fn complete_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Json(body): Json<CompleteBody>,
) -> Result<Json<Value>, AppError> {
    // kleos_stores enforcement: the agent must have stored at least one
    // memory (i.e. written to kleos) between gate-open and gate-complete.
    // This is how we enforce "store outcomes after completing any task".
    let gate_id = body.gate_id;
    let user_id = auth.user_id;
    let (agent, opened_at): (String, String) = db
        .read(move |conn| {
            conn.query_row(
                "SELECT agent, created_at FROM gate_requests WHERE id = ?1 AND user_id = ?2",
                params![gate_id, user_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    kleos_lib::EngError::NotFound(format!("gate request {} not found", gate_id))
                }
                other => kleos_lib::EngError::DatabaseMessage(other.to_string()),
            })
        })
        .await?;

    let agent_filter = agent.clone();
    let opened_at_filter = opened_at.clone();
    let stored_count: i64 = db
        .read(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM memories
                 WHERE user_id = ?1 AND source = ?2 AND created_at >= ?3",
                params![user_id, agent_filter, opened_at_filter],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await?;

    if stored_count == 0 {
        return Err(AppError::from(kleos_lib::EngError::InvalidInput(format!(
            "gate {} cannot be completed: agent '{}' has not stored any memories \
             since the gate was opened at {}. Store the outcome first.",
            gate_id, agent, opened_at
        ))));
    }

    complete_gate(
        &db,
        body.gate_id,
        &body.output,
        &body.known_secrets,
        auth.user_id,
    )
    .await?;
    Ok(Json(json!({ "ok": true, "kleos_stores": stored_count })))
}

/// Simple guard endpoint that checks if an action conflicts with high-importance static rules.
/// This is a simplified version without LLM integration - it only does keyword matching.
async fn guard_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Json(body): Json<GuardBody>,
) -> Result<Json<Value>, AppError> {
    if body.action.trim().is_empty() {
        return Err(AppError::from(kleos_lib::EngError::InvalidInput(
            "action (string) required - describe what you are about to do".into(),
        )));
    }

    // Search for high-importance static memories that might conflict.
    // ResolvedDb hands us the caller's tenant shard; tenant scoping is
    // implicit. Migration #25 (drop_user_id_memory_core) removed the
    // per-row user_id column, so a WHERE user_id = ? predicate here
    // erroneously errors with "no such column".
    let _ = auth;
    let rules: Vec<Value> = db
        .read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, content, importance FROM memories
                 WHERE is_static = 1 AND importance >= 8 AND is_forgotten = 0
                 ORDER BY importance DESC LIMIT 20",
            )?;
            let rows = stmt.query_map([], |row| {
                let id: i64 = row.get(0)?;
                let content: String = row.get(1)?;
                let importance: i64 = row.get(2)?;
                Ok((id, content, importance))
            })?;
            let mut rules = Vec::new();
            for row in rows {
                let (id, content, importance) = row?;
                rules.push(json!({
                    "id": id,
                    "content": content,
                    "importance": importance,
                }));
            }
            Ok(rules)
        })
        .await?;

    if rules.is_empty() {
        return Ok(Json(json!({
            "signal": "allow",
            "action": body.action,
            "rules": [],
            "message": "No conflicting rules found.",
        })));
    }

    // Simple heuristic: if any rule contains prohibition keywords and the action
    // contains related terms, warn. Without LLM, we can't do semantic matching.
    let action_lower = body.action.to_lowercase();
    let prohibition_keywords = [
        "never",
        "don't",
        "do not",
        "must not",
        "prohibited",
        "forbidden",
    ];

    let mut matched_rules = Vec::new();
    for rule in &rules {
        let content = rule["content"].as_str().unwrap_or("").to_lowercase();
        let has_prohibition = prohibition_keywords.iter().any(|k| content.contains(k));

        // Very basic: check if any significant word from the action appears in the rule
        let action_words: Vec<&str> = action_lower
            .split_whitespace()
            .filter(|w| w.len() > 3)
            .collect();
        let has_overlap = action_words.iter().any(|w| content.contains(w));

        if has_prohibition && has_overlap {
            matched_rules.push(rule.clone());
        }
    }

    let (signal, message) = if matched_rules.is_empty() {
        (
            "allow",
            "No direct conflicts detected. Note: LLM-based semantic matching not available.",
        )
    } else {
        (
            "warn",
            "Potential rule conflicts detected. Review before proceeding.",
        )
    };

    Ok(Json(json!({
        "signal": signal,
        "action": body.action,
        "rules": matched_rules,
        "message": message,
    })))
}

/// Semantic gate check grounded in the user's Hopfield memory. Embeds the
/// command, asks the brain for nearest patterns, and returns a block reason
/// if any high-activation recall contains a prohibition keyword that appears
/// relevant to the command. Returns None if the brain/embedder is unavailable
/// or no matching rule is found.
async fn brain_grounded_check(state: &AppState, _user_id: i64, command: &str) -> Option<String> {
    let brain = state.brain.as_ref()?;
    if !brain.is_ready() {
        return None;
    }
    let embedder = state.current_embedder().await?;

    let options = kleos_lib::services::brain::BrainQueryOptions {
        query: command.to_string(),
        top_k: Some(8),
        beta: None,
        spread_hops: None,
    };
    let result = match brain.query(embedder.as_ref(), command, &options).await {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!(error = %e, "brain_grounded_check: query failed");
            return None;
        }
    };

    const ACTIVATION_THRESHOLD: f64 = 0.6;
    const PROHIBITIONS: &[&str] = &[
        "never",
        "do not",
        "don't",
        "must not",
        "prohibited",
        "forbidden",
        "blocked",
        "banned",
    ];

    let command_lower = command.to_lowercase();
    let command_tokens: Vec<&str> = command_lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 3)
        .collect();

    for mem in &result.activated {
        if mem.activation < ACTIVATION_THRESHOLD {
            continue;
        }
        let content_lower = mem.content.to_lowercase();
        let has_prohibition = PROHIBITIONS.iter().any(|k| content_lower.contains(k));
        if !has_prohibition {
            continue;
        }
        // Require at least one shared token to avoid tripping on rules that
        // happen to contain "never" but talk about something unrelated.
        let overlaps = command_tokens.iter().any(|t| content_lower.contains(t));
        if !overlaps {
            continue;
        }
        return Some(format!(
            "Blocked by brain-grounded rule (memory #{}, activation {:.2}): {}",
            mem.id,
            mem.activation,
            truncate(&mem.content, 200)
        ));
    }
    None
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let end: String = s.chars().take(max).collect();
        format!("{}...", end)
    }
}
