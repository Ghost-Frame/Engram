use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use crate::db::Database;
use crate::Result;
use tracing::info;

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

/// Evaluate content against all active guard rules.
///
/// Checks each enabled rule's regex pattern against the content.
/// - Block rules: immediately deny if matched
/// - Flag rules: allow but include in triggered_rules list
/// - Redact rules: replace matches with [REDACTED]
pub async fn evaluate(db: &Database, content: &str) -> Result<GuardResult> {
    let rules = list_rules(db).await?;

    let mut triggered: Vec<String> = Vec::new();
    let mut blocked = false;
    let mut redacted_content = content.to_string();

    for rule in &rules {
        if !rule.enabled {
            continue;
        }

        let re = match Regex::new(&rule.pattern) {
            Ok(r) => r,
            Err(_) => continue, // Skip invalid patterns
        };

        if re.is_match(content) {
            triggered.push(rule.name.clone());

            match rule.action {
                GuardAction::Block => {
                    blocked = true;
                    info!(rule = %rule.name, "guard_blocked");
                }
                GuardAction::Flag => {
                    info!(rule = %rule.name, "guard_flagged");
                }
                GuardAction::Redact => {
                    redacted_content = re.replace_all(&redacted_content, "[REDACTED]").to_string();
                    info!(rule = %rule.name, "guard_redacted");
                }
            }
        }
    }

    let has_redactions = redacted_content != content;

    Ok(GuardResult {
        allowed: !blocked,
        triggered_rules: triggered,
        redacted_content: if has_redactions {
            Some(redacted_content)
        } else {
            None
        },
    })
}

/// Create a new guard rule.
pub async fn create_rule(db: &Database, rule: GuardRule) -> Result<GuardRule> {
    let conn = db.connection();

    // Validate the regex pattern
    if Regex::new(&rule.pattern).is_err() {
        return Err(crate::EngError::InvalidInput(format!(
            "invalid regex pattern: {}",
            rule.pattern
        )));
    }

    let action_str = match rule.action {
        GuardAction::Block => "block",
        GuardAction::Flag => "flag",
        GuardAction::Redact => "redact",
    };

    conn.execute(
        "INSERT INTO audit_log (action, target_type, details, created_at) \
         VALUES ('create_guard_rule', 'guard_rule', ?1, datetime('now'))",
        libsql::params![serde_json::json!({
            "name": rule.name,
            "pattern": rule.pattern,
            "action": action_str,
            "enabled": rule.enabled,
        })
        .to_string()],
    )
    .await?;

    // We store guard rules in the audit_log with a special action type,
    // but ideally they'd have their own table. For now, use a simple approach:
    // store in a temporary table structure using current_state.
    conn.execute(
        "INSERT INTO current_state (agent, key, value, user_id, created_at, updated_at) \
         VALUES ('guard', ?1, ?2, 1, datetime('now'), datetime('now')) \
         ON CONFLICT(agent, key, user_id) DO UPDATE SET \
           value = excluded.value, \
           updated_at = datetime('now')",
        libsql::params![
            format!("rule:{}", rule.name),
            serde_json::json!({
                "name": rule.name,
                "pattern": rule.pattern,
                "action": action_str,
                "enabled": rule.enabled,
            })
            .to_string()
        ],
    )
    .await?;

    info!(name = %rule.name, action = action_str, "guard_rule_created");

    Ok(rule)
}

/// List all guard rules.
pub async fn list_rules(db: &Database) -> Result<Vec<GuardRule>> {
    let conn = db.connection();

    let mut rows = conn
        .query(
            "SELECT key, value, created_at FROM current_state \
             WHERE agent = 'guard' AND key LIKE 'rule:%' \
             ORDER BY created_at ASC",
            (),
        )
        .await?;

    let mut rules = Vec::new();

    while let Some(row) = rows.next().await? {
        let _key: String = row.get(0)?;
        let value: String = row.get(1)?;
        let created_at_str: String = row.get(2)?;

        // Parse the JSON value
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&value) {
            let name = parsed
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let pattern = parsed
                .get("pattern")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let action_str = parsed
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("flag");
            let enabled = parsed
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            let action = match action_str {
                "block" => GuardAction::Block,
                "redact" => GuardAction::Redact,
                _ => GuardAction::Flag,
            };

            let created_at =
                chrono::NaiveDateTime::parse_from_str(&created_at_str, "%Y-%m-%d %H:%M:%S")
                    .map(|dt| DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc))
                    .unwrap_or_else(|_| Utc::now());

            rules.push(GuardRule {
                id: name.clone(),
                name,
                pattern,
                action,
                enabled,
                created_at,
            });
        }
    }

    Ok(rules)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_guard_action_serialization() {
        let action = GuardAction::Block;
        let json = serde_json::to_string(&action).unwrap();
        assert_eq!(json, "\"block\"");

        let deserialized: GuardAction = serde_json::from_str("\"redact\"").unwrap();
        assert_eq!(deserialized, GuardAction::Redact);
    }

    #[test]
    fn test_guard_result_allowed() {
        let result = GuardResult {
            allowed: true,
            triggered_rules: Vec::new(),
            redacted_content: None,
        };
        assert!(result.allowed);
        assert!(result.triggered_rules.is_empty());
    }

    #[test]
    fn test_guard_result_blocked() {
        let result = GuardResult {
            allowed: false,
            triggered_rules: vec!["no_secrets".to_string()],
            redacted_content: None,
        };
        assert!(!result.allowed);
        assert_eq!(result.triggered_rules.len(), 1);
    }

    #[test]
    fn test_regex_redaction() {
        let re = Regex::new(r"\b\d{3}-\d{2}-\d{4}\b").unwrap();
        let content = "My SSN is 123-45-6789 and that's private";
        let redacted = re.replace_all(content, "[REDACTED]").to_string();
        assert!(redacted.contains("[REDACTED]"));
        assert!(!redacted.contains("123-45-6789"));
    }
}
