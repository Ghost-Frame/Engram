use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::memory::{self, types::StoreRequest};
use crate::services::axon::{publish_event, PublishEventRequest};
use crate::services::soma::{get_agent_by_name, heartbeat, register_agent, RegisterAgentRequest};
use crate::{EngError, Result};

// -- Types --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityReport {
    pub agent: String,
    pub action: String,
    pub summary: String,
    pub project: Option<String>,
    #[serde(alias = "metadata")]
    pub details: Option<serde_json::Value>,
}

// -- Validation --

pub fn validate_activity_report(report: &ActivityReport) -> Result<()> {
    if report.agent.is_empty() {
        return Err(EngError::InvalidInput("agent cannot be empty".to_string()));
    }
    if report.agent.len() > 100 {
        return Err(EngError::InvalidInput(
            "agent exceeds maximum length of 100 characters".to_string(),
        ));
    }
    if report.action.is_empty() {
        return Err(EngError::InvalidInput("action cannot be empty".to_string()));
    }
    if report.action.len() > 100 {
        return Err(EngError::InvalidInput(
            "action exceeds maximum length of 100 characters".to_string(),
        ));
    }
    if report.summary.is_empty() {
        return Err(EngError::InvalidInput(
            "summary cannot be empty".to_string(),
        ));
    }
    if report.summary.len() > 10000 {
        return Err(EngError::InvalidInput(
            "summary exceeds maximum length of 10000 characters".to_string(),
        ));
    }
    Ok(())
}

// -- Channel and importance helpers --

pub fn action_to_channel(action: &str) -> &'static str {
    if action == "task.blocked" || action == "error.raised" {
        "alerts"
    } else if action.starts_with("task.") {
        "tasks"
    } else if action.starts_with("drift.") || action.starts_with("session.") {
        "quality"
    } else {
        "system"
    }
}

fn action_to_importance(action: &str) -> i32 {
    match action {
        "task.completed" => 6,
        "task.blocked" | "error.raised" => 7,
        _ => 4,
    }
}

fn action_to_category(action: &str) -> &'static str {
    if action.starts_with("task.") {
        "task"
    } else {
        "activity"
    }
}

// -- Core function --

pub async fn process_activity(db: &Database, report: &ActivityReport, user_id: i64) -> Result<i64> {
    validate_activity_report(report)?;

    // Build memory content -- include project if provided
    let content = if let Some(ref project) = report.project {
        format!(
            "[{}] [{}] [{}] {}",
            report.agent, project, report.action, report.summary
        )
    } else {
        format!("[{}] [{}] {}", report.agent, report.action, report.summary)
    };

    let category = action_to_category(&report.action).to_string();
    let importance = action_to_importance(&report.action);

    let store_result = memory::store(
        db,
        StoreRequest {
            content,
            category,
            source: report.agent.clone(),
            importance,
            user_id: Some(user_id),
            tags: None,
            embedding: None,
            session_id: None,
            is_static: None,
            space_id: None,
            parent_memory_id: None,
        },
    )
    .await?;

    // Upsert agent in soma then heartbeat
    let agent_id = match get_agent_by_name(db, user_id, &report.agent).await {
        Ok(a) => a.id,
        Err(crate::EngError::NotFound(_)) => {
            register_agent(
                db,
                RegisterAgentRequest {
                    user_id: Some(user_id),
                    name: report.agent.clone(),
                    category: Some("cli".to_string()),
                    description: None,
                },
            )
            .await?
            .id
        }
        Err(e) => return Err(e),
    };
    heartbeat(db, agent_id, user_id).await?;

    // Publish Axon event
    let channel = action_to_channel(&report.action).to_string();
    let mut payload = serde_json::json!({
        "agent": report.agent,
        "action": report.action,
        "summary": report.summary,
    });
    if let Some(ref project) = report.project {
        payload["project"] = serde_json::Value::String(project.clone());
    }
    if let Some(ref details) = report.details {
        payload["details"] = details.clone();
    }

    publish_event(
        db,
        PublishEventRequest {
            channel,
            action: report.action.clone(),
            payload: Some(payload),
            source: Some("activity".to_string()),
            agent: Some(report.agent.clone()),
            user_id: Some(user_id),
        },
    )
    .await?;

    Ok(store_result.id)
}

// -- Tests --

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_activity_validates_required_fields() {
        let empty_agent = ActivityReport {
            agent: "".to_string(),
            action: "task.started".to_string(),
            summary: "test".to_string(),
            project: None,
            details: None,
        };
        assert!(validate_activity_report(&empty_agent).is_err());

        let empty_action = ActivityReport {
            agent: "claude-code".to_string(),
            action: "".to_string(),
            summary: "test".to_string(),
            project: None,
            details: None,
        };
        assert!(validate_activity_report(&empty_action).is_err());

        let empty_summary = ActivityReport {
            agent: "claude-code".to_string(),
            action: "task.started".to_string(),
            summary: "".to_string(),
            project: None,
            details: None,
        };
        assert!(validate_activity_report(&empty_summary).is_err());

        let valid = ActivityReport {
            agent: "claude-code".to_string(),
            action: "task.started".to_string(),
            summary: "test summary".to_string(),
            project: None,
            details: None,
        };
        assert!(validate_activity_report(&valid).is_ok());
    }

    #[test]
    fn test_activity_channel_selection() {
        assert_eq!(action_to_channel("task.blocked"), "alerts");
        assert_eq!(action_to_channel("error.raised"), "alerts");
        assert_eq!(action_to_channel("task.started"), "tasks");
        assert_eq!(action_to_channel("task.completed"), "tasks");
        assert_eq!(action_to_channel("task.progress"), "tasks");
        assert_eq!(action_to_channel("drift.detected"), "quality");
        assert_eq!(action_to_channel("session.ended"), "quality");
        assert_eq!(action_to_channel("agent.online"), "system");
        assert_eq!(action_to_channel("deploy.pushed"), "system");
    }

    #[test]
    fn test_activity_importance() {
        assert_eq!(action_to_importance("task.completed"), 6);
        assert_eq!(action_to_importance("task.blocked"), 7);
        assert_eq!(action_to_importance("error.raised"), 7);
        assert_eq!(action_to_importance("task.started"), 4);
        assert_eq!(action_to_importance("agent.online"), 4);
    }

    #[test]
    fn test_activity_category() {
        assert_eq!(action_to_category("task.started"), "task");
        assert_eq!(action_to_category("task.completed"), "task");
        assert_eq!(action_to_category("error.raised"), "activity");
        assert_eq!(action_to_category("agent.online"), "activity");
    }

    #[test]
    fn test_activity_validates_length_limits() {
        let long_agent = ActivityReport {
            agent: "a".repeat(101),
            action: "task.started".to_string(),
            summary: "test".to_string(),
            project: None,
            details: None,
        };
        assert!(validate_activity_report(&long_agent).is_err());

        let long_action = ActivityReport {
            agent: "claude-code".to_string(),
            action: "a".repeat(101),
            summary: "test".to_string(),
            project: None,
            details: None,
        };
        assert!(validate_activity_report(&long_action).is_err());

        let long_summary = ActivityReport {
            agent: "claude-code".to_string(),
            action: "task.started".to_string(),
            summary: "a".repeat(10001),
            project: None,
            details: None,
        };
        assert!(validate_activity_report(&long_summary).is_err());
    }

    #[tokio::test]
    async fn test_activity_fan_out_creates_memory() {
        use crate::db::Database;
        // Create in-memory DB and initialize schema
        let db = Database::connect_memory().await.expect("in-memory db");

        let report = ActivityReport {
            agent: "test-agent".to_string(),
            action: "task.started".to_string(),
            summary: "Testing activity fan-out".to_string(),
            project: Some("test".to_string()),
            details: None,
        };
        let result = process_activity(&db, &report, 1).await;
        assert!(
            result.is_ok(),
            "process_activity should succeed: {:?}",
            result.err()
        );
        assert!(result.unwrap() > 0, "should return a positive memory ID");
    }
}
