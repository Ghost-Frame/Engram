use std::collections::VecDeque;
use super::{Rule, CheckType, Violation};

const MAX_HISTORY: usize = 10;
const RETRY_THRESHOLD: usize = 3;

pub struct RetryTracker {
    recent_commands: VecDeque<String>,
}

impl RetryTracker {
    pub fn new() -> Self {
        Self {
            recent_commands: VecDeque::with_capacity(MAX_HISTORY),
        }
    }

    pub fn check(&mut self, entry: &serde_json::Value, rules: &[Rule]) -> Vec<Violation> {
        let cmd = match extract_command(entry) {
            Some(c) => c,
            None => return Vec::new(),
        };

        self.recent_commands.push_back(cmd.clone());
        if self.recent_commands.len() > MAX_HISTORY {
            self.recent_commands.pop_front();
        }

        let consecutive = self
            .recent_commands
            .iter()
            .rev()
            .take_while(|c| *c == &cmd)
            .count();

        if consecutive >= RETRY_THRESHOLD {
            let rule = rules.iter().find(|r| matches!(r.check_type, CheckType::RetryLoop));
            if let Some(rule) = rule {
                return vec![Violation {
                    rule_id: rule.id.clone(),
                    severity: rule.severity.clone(),
                    message: format!("{} ({} repeats of: {})", rule.message, consecutive, truncate(&cmd, 80)),
                    context: cmd,
                }];
            }
        }

        Vec::new()
    }
}

fn extract_command(entry: &serde_json::Value) -> Option<String> {
    let obj = entry.as_object()?;
    let input = obj.get("tool_input").or(obj.get("input"))?;
    input.get("command").and_then(|v| v.as_str()).map(|s| s.to_string())
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
