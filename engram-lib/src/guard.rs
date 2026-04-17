use crate::db::Database;
use crate::{EngError, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::info;

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

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
#[tracing::instrument(skip(db, content), fields(content_len = content.len()))]
pub async fn evaluate(db: &Database, content: &str) -> Result<GuardResult> {
    let rules = list_rules(db).await?;

    let mut triggered: Vec<String> = Vec::new();
    let mut blocked = false;
    let mut redacted_content = content.to_string();

    for rule in &rules {
        if !rule.enabled {
            continue;
        }

        // SECURITY/DoS: bound pattern length and compile with explicit size
        // limits so a hostile rule cannot blow up regex compilation or match
        // cost. Any pattern that exceeds these limits is skipped.
        const MAX_PATTERN_CHARS: usize = 4_096;
        if rule.pattern.len() > MAX_PATTERN_CHARS {
            tracing::warn!(
                rule = %rule.name,
                len = rule.pattern.len(),
                "guard pattern exceeds length cap; skipping"
            );
            continue;
        }
        let re = match regex::RegexBuilder::new(&rule.pattern)
            .size_limit(1 << 20)
            .dfa_size_limit(1 << 20)
            .build()
        {
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
    // SECURITY: reject over-long patterns at create time so they can never
    // reach the hot evaluate() path and starve CPU.
    const MAX_PATTERN_CHARS: usize = 4_096;
    if rule.pattern.len() > MAX_PATTERN_CHARS {
        return Err(crate::EngError::InvalidInput(format!(
            "guard pattern exceeds {} character cap",
            MAX_PATTERN_CHARS
        )));
    }
    // Validate the regex pattern (with bounded compile size).
    if regex::RegexBuilder::new(&rule.pattern)
        .size_limit(1 << 20)
        .dfa_size_limit(1 << 20)
        .build()
        .is_err()
    {
        return Err(crate::EngError::InvalidInput(format!(
            "invalid regex pattern: {}",
            rule.pattern
        )));
    }

    let action_str = match rule.action {
        GuardAction::Block => "block",
        GuardAction::Flag => "flag",
        GuardAction::Redact => "redact",
    }
    .to_string();

    let audit_details = serde_json::json!({
        "name": rule.name,
        "pattern": rule.pattern,
        "action": action_str,
        "enabled": rule.enabled,
    })
    .to_string();

    let state_key = format!("rule:{}", rule.name);
    let state_value = serde_json::json!({
        "name": rule.name,
        "pattern": rule.pattern,
        "action": action_str,
        "enabled": rule.enabled,
    })
    .to_string();

    let rule_name_log = rule.name.clone();
    let action_str_log = action_str.clone();

    db.write(move |conn| {
        conn.execute(
            "INSERT INTO audit_log (action, target_type, details, created_at) \
             VALUES ('create_guard_rule', 'guard_rule', ?1, datetime('now'))",
            rusqlite::params![audit_details],
        )
        .map_err(rusqlite_to_eng_error)?;

        conn.execute(
            "INSERT INTO current_state (agent, key, value, user_id, created_at, updated_at) \
             VALUES ('guard', ?1, ?2, 1, datetime('now'), datetime('now')) \
             ON CONFLICT(agent, key, user_id) DO UPDATE SET \
               value = excluded.value, \
               updated_at = datetime('now')",
            rusqlite::params![state_key, state_value],
        )
        .map_err(rusqlite_to_eng_error)?;

        Ok(())
    })
    .await?;

    info!(name = %rule_name_log, action = action_str_log, "guard_rule_created");

    Ok(rule)
}

/// List all guard rules.
pub async fn list_rules(db: &Database) -> Result<Vec<GuardRule>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT key, value, created_at FROM current_state \
                 WHERE agent = 'guard' AND key LIKE 'rule:%' \
                 ORDER BY created_at ASC",
            )
            .map_err(rusqlite_to_eng_error)?;

        let rows = stmt
            .query_map([], |row| {
                let _key: String = row.get(0)?;
                let value: String = row.get(1)?;
                let created_at_str: String = row.get(2)?;
                Ok((value, created_at_str))
            })
            .map_err(rusqlite_to_eng_error)?;

        let mut rules = Vec::new();

        for row_result in rows {
            let (value, created_at_str) = row_result.map_err(rusqlite_to_eng_error)?;

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
    })
    .await
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
        let re = regex::Regex::new(r"\b\d{3}-\d{2}-\d{4}\b").unwrap();
        let content = "My SSN is 123-45-6789 and that's private";
        let redacted = re.replace_all(content, "[REDACTED]").to_string();
        assert!(redacted.contains("[REDACTED]"));
        assert!(!redacted.contains("123-45-6789"));
    }
}
