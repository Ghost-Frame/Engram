use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::Database;
use crate::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardRule {
    pub id: String,
    pub name: String,
    pub pattern: String,
    pub action: GuardAction,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GuardAction {
    Block,
    Flag,
    Redact,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardResult {
    pub allowed: bool,
    pub triggered_rules: Vec<String>,
    pub redacted_content: Option<String>,
}

/// Create the guard_rules table if it does not already exist.
/// Guard rules are not part of the main schema because they are managed
/// entirely by this module.
async fn ensure_guard_rules_table(db: &Database) -> Result<()> {
    db.conn.execute(
        "CREATE TABLE IF NOT EXISTS guard_rules (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            pattern TEXT NOT NULL,
            action TEXT NOT NULL DEFAULT 'flag',
            enabled INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
        (),
    ).await?;
    Ok(())
}

fn parse_action(s: &str) -> GuardAction {
    match s {
        "block" => GuardAction::Block,
        "redact" => GuardAction::Redact,
        _ => GuardAction::Flag,
    }
}

fn action_str(action: &GuardAction) -> &'static str {
    match action {
        GuardAction::Block => "block",
        GuardAction::Flag => "flag",
        GuardAction::Redact => "redact",
    }
}

fn parse_created_at(s: String) -> DateTime<Utc> {
    s.replace(' ', "T")
        .parse::<DateTime<Utc>>()
        .unwrap_or_else(|_| Utc::now())
}

fn row_to_rule(row: &libsql::Row) -> Result<GuardRule> {
    let action_str: String = row.get(3).map_err(|e| crate::EngError::Internal(e.to_string()))?;
    let enabled_i: i64 = row.get(4).map_err(|e| crate::EngError::Internal(e.to_string()))?;
    let created_at_str: String = row.get(5).map_err(|e| crate::EngError::Internal(e.to_string()))?;

    Ok(GuardRule {
        id: row.get(0).map_err(|e| crate::EngError::Internal(e.to_string()))?,
        name: row.get(1).map_err(|e| crate::EngError::Internal(e.to_string()))?,
        pattern: row.get(2).map_err(|e| crate::EngError::Internal(e.to_string()))?,
        action: parse_action(&action_str),
        enabled: enabled_i != 0,
        created_at: parse_created_at(created_at_str),
    })
}

/// Evaluate content against all enabled guard rules.
///
/// Returns a GuardResult indicating:
/// - `allowed`: false if any Block rule matched
/// - `triggered_rules`: names of all matched rules (Block, Flag, or Redact)
/// - `redacted_content`: content with Redact matches replaced, if any Redact rule fired
pub async fn evaluate(db: &Database, content: &str) -> Result<GuardResult> {
    let rules = list_rules(db).await?;

    let mut triggered = Vec::new();
    let mut blocked = false;
    let mut redacted = content.to_string();
    let mut any_redact = false;

    for rule in &rules {
        if !rule.enabled {
            continue;
        }

        // Try regex first; fall back to literal substring match if pattern is invalid.
        let matched = match Regex::new(&rule.pattern) {
            Ok(re) => re.is_match(content),
            Err(_) => content.contains(&rule.pattern),
        };

        if !matched {
            continue;
        }

        triggered.push(rule.name.clone());

        match rule.action {
            GuardAction::Block => {
                blocked = true;
            }
            GuardAction::Flag => {
                // Logged via triggered_rules; request still proceeds.
            }
            GuardAction::Redact => {
                any_redact = true;
                let escaped = regex::escape(&rule.pattern);
                let re = Regex::new(&rule.pattern).unwrap_or_else(|_| Regex::new(&escaped).unwrap());
                redacted = re.replace_all(&redacted, "[REDACTED]").to_string();
            }
        }
    }

    Ok(GuardResult {
        allowed: !blocked,
        triggered_rules: triggered,
        redacted_content: if any_redact { Some(redacted) } else { None },
    })
}

/// Persist a new guard rule. Generates a UUID id if `rule.id` is empty.
pub async fn create_rule(db: &Database, rule: GuardRule) -> Result<GuardRule> {
    ensure_guard_rules_table(db).await?;

    let id = if rule.id.is_empty() {
        Uuid::new_v4().to_string()
    } else {
        rule.id.clone()
    };

    let action = action_str(&rule.action);

    let mut rows = db.conn.query(
        "INSERT INTO guard_rules (id, name, pattern, action, enabled, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         RETURNING id, name, pattern, action, enabled, created_at",
        libsql::params![
            id.clone(),
            rule.name.clone(),
            rule.pattern.clone(),
            action,
            rule.enabled as i64,
            rule.created_at.to_rfc3339(),
        ],
    ).await?;

    let row = rows.next().await?.ok_or_else(|| crate::EngError::Internal("guard_rules insert returned no row".into()))?;
    row_to_rule(&row)
}

/// List all guard rules (enabled and disabled), ordered by creation time.
pub async fn list_rules(db: &Database) -> Result<Vec<GuardRule>> {
    ensure_guard_rules_table(db).await?;

    let mut rows = db.conn.query(
        "SELECT id, name, pattern, action, enabled, created_at FROM guard_rules ORDER BY created_at ASC",
        (),
    ).await?;

    let mut rules = Vec::new();
    while let Some(row) = rows.next().await? {
        match row_to_rule(&row) {
            Ok(rule) => rules.push(rule),
            Err(e) => tracing::warn!("guard_rules row parse error: {}", e),
        }
    }
    Ok(rules)
}
