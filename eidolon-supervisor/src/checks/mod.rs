pub mod drift;
pub mod retry_loop;
pub mod rule_match;
pub mod scope;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    pub check_type: CheckType,
    pub pattern: String,
    pub severity: Severity,
    pub cooldown_secs: u64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckType {
    RuleMatch,
    RetryLoop,
    ScopeViolation,
    Drift,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug)]
pub struct Violation {
    pub rule_id: String,
    pub severity: Severity,
    pub message: String,
    pub context: String,
}

pub fn default_rules() -> Vec<Rule> {
    vec![
        Rule {
            id: "no-force-push".into(),
            check_type: CheckType::RuleMatch,
            pattern: r"git\s+push\s+.*--force".into(),
            severity: Severity::Critical,
            cooldown_secs: 300,
            message: "Force push detected".into(),
        },
        Rule {
            id: "no-reboot".into(),
            check_type: CheckType::RuleMatch,
            pattern: r"reboot|shutdown|systemctl\s+(reboot|poweroff)".into(),
            severity: Severity::Critical,
            cooldown_secs: 600,
            message: "Reboot or shutdown command detected".into(),
        },
        Rule {
            id: "retry-loop".into(),
            check_type: CheckType::RetryLoop,
            pattern: String::new(),
            severity: Severity::Warning,
            cooldown_secs: 120,
            message: "Agent stuck in retry loop (3+ identical failing commands)".into(),
        },
        Rule {
            id: "em-dash-usage".into(),
            check_type: CheckType::RuleMatch,
            pattern: "\u{2014}".into(),
            severity: Severity::Info,
            cooldown_secs: 60,
            message: "Em dash used in output -- should use -- instead".into(),
        },
    ]
}
