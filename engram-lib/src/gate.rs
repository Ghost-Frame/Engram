use crate::db::Database;
use crate::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateCheckRequest {
    pub command: String,
    pub agent: String,
    pub context: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateCheckResult {
    pub allowed: bool,
    pub reason: Option<String>,
    pub resolved_command: Option<String>,
    pub gate_id: i64,
    pub requires_approval: bool,
}

/// Check a command against blocked patterns and store the gate request in the DB.
pub async fn check_command(
    db: &Database,
    req: &GateCheckRequest,
    user_id: i64,
) -> Result<GateCheckResult> {
    // 1. Check dangerous patterns
    if let Some(reason) = check_dangerous_patterns(&req.command) {
        // Store blocked request
        let gate_id = store_gate_request(
            db,
            user_id,
            &req.agent,
            &req.command,
            req.context.as_deref(),
            "blocked",
            Some(&reason),
        )
        .await?;
        return Ok(GateCheckResult {
            allowed: false,
            reason: Some(reason),
            resolved_command: None,
            gate_id,
            requires_approval: false,
        });
    }

    // 2. Detect secret placeholders (note them but don't resolve - no credd client)
    let has_secrets = has_secret_placeholders(&req.command);

    // 3. Store as pending gate request
    let status = if has_secrets {
        "pending_secrets"
    } else {
        "allowed"
    };
    let reason = if has_secrets {
        Some("Command contains secret placeholders -- resolve before execution")
    } else {
        None
    };
    let gate_id = store_gate_request(
        db,
        user_id,
        &req.agent,
        &req.command,
        req.context.as_deref(),
        status,
        reason,
    )
    .await?;

    Ok(GateCheckResult {
        allowed: !has_secrets,
        reason: reason.map(|r| r.to_string()),
        resolved_command: if !has_secrets {
            Some(req.command.clone())
        } else {
            None
        },
        gate_id,
        requires_approval: has_secrets,
    })
}

/// Update a gate request with approval decision.
pub async fn respond_to_gate(
    db: &Database,
    gate_id: i64,
    approved: bool,
    reason: Option<&str>,
    user_id: i64,
) -> Result<Value> {
    let status = if approved { "approved" } else { "denied" };
    let reason_str = reason.unwrap_or(if approved {
        "approved by user"
    } else {
        "denied by user"
    });

    let rows_affected = db
        .conn
        .execute(
            "UPDATE gate_requests SET status = ?1, reason = ?2, updated_at = datetime('now')
         WHERE id = ?3 AND user_id = ?4",
            libsql::params![status.to_string(), reason_str.to_string(), gate_id, user_id],
        )
        .await?;

    if rows_affected == 0 {
        return Err(crate::EngError::NotFound(format!(
            "gate request {} not found",
            gate_id
        )));
    }

    // Return the (possibly resolved) command if approved
    if approved {
        let mut rows = db
            .conn
            .query(
                "SELECT command FROM gate_requests WHERE id = ?1 AND user_id = ?2",
                libsql::params![gate_id, user_id],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            let command: String = row.get(0)?;
            return Ok(serde_json::json!({ "ok": true, "approved": true, "command": command }));
        }
    }

    Ok(serde_json::json!({ "ok": true, "approved": approved }))
}

/// Mark a gate request as complete and scrub sensitive data from output.
pub async fn complete_gate(
    db: &Database,
    gate_id: i64,
    output: &str,
    known_secrets: &[String],
    user_id: i64,
) -> Result<()> {
    let scrubbed = scrub_output(output, known_secrets);

    let rows_affected = db.conn.execute(
        "UPDATE gate_requests SET status = 'completed', output = ?1, updated_at = datetime('now')
         WHERE id = ?2 AND user_id = ?3",
        libsql::params![scrubbed, gate_id, user_id],
    ).await?;

    if rows_affected == 0 {
        return Err(crate::EngError::NotFound(format!(
            "gate request {} not found",
            gate_id
        )));
    }

    Ok(())
}

// -- Internal helpers --

async fn store_gate_request(
    db: &Database,
    user_id: i64,
    agent: &str,
    command: &str,
    context: Option<&str>,
    status: &str,
    reason: Option<&str>,
) -> Result<i64> {
    db.conn
        .execute(
            "INSERT INTO gate_requests (user_id, agent, command, context, status, reason)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            libsql::params![
                user_id,
                agent.to_string(),
                command.to_string(),
                context.map(|s| s.to_string()),
                status.to_string(),
                reason.map(|s| s.to_string()),
            ],
        )
        .await?;

    let mut rows = db.conn.query("SELECT last_insert_rowid()", ()).await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| crate::EngError::Internal("no rowid".into()))?;
    Ok(row.get(0)?)
}

/// Check a command against static dangerous patterns.
/// Returns Some(reason) if the command is blocked, None if it's allowed.
pub fn check_dangerous_patterns(command: &str) -> Option<String> {
    let cmd_lower = command.to_lowercase();

    // Destructive rm patterns
    if cmd_lower.contains("rm -rf /") {
        let after_parts: Vec<&str> = cmd_lower.splitn(2, "rm -rf /").collect();
        let path_start = after_parts.get(1).unwrap_or(&"");
        let is_tmp_safe = path_start.starts_with("tmp ")
            || path_start.starts_with("tmp/")
            || *path_start == "tmp"
            || path_start.starts_with("tmp\n");
        if !is_tmp_safe {
            return Some("Destructive rm -rf on critical path -- not allowed".to_string());
        }
    }
    if cmd_lower.contains("rm -rf ~/") {
        return Some("Destructive rm -rf on home directory -- not allowed".to_string());
    }
    if cmd_lower.contains("rm -rf /home") {
        return Some("Destructive rm -rf on /home -- not allowed".to_string());
    }

    // Force push to protected branches
    if cmd_lower.contains("git push")
        && cmd_lower.contains("--force")
        && (cmd_lower.contains("main") || cmd_lower.contains("master"))
    {
        return Some("Force push to main/master branch blocked".to_string());
    }

    // Hard reset
    if cmd_lower.contains("git reset --hard") {
        return Some("git reset --hard is destructive -- use git stash instead".to_string());
    }

    // Reboot/shutdown
    if cmd_lower.contains("reboot") || cmd_lower.contains("shutdown") {
        return Some("Reboot/shutdown commands require explicit confirmation".to_string());
    }

    // DROP TABLE
    if cmd_lower.contains("drop table") {
        return Some("DROP TABLE requires manual confirmation".to_string());
    }

    // Disk format
    if cmd_lower.contains("mkfs.") {
        return Some("Disk format command blocked -- requires manual confirmation".to_string());
    }

    // base64 decode piped to shell
    let has_base64_decode =
        cmd_lower.contains("base64 -d") || cmd_lower.contains("base64 --decode");
    let has_shell_pipe = cmd_lower.contains("| sh")
        || cmd_lower.contains("| bash")
        || cmd_lower.contains("|sh")
        || cmd_lower.contains("|bash");
    if has_base64_decode && has_shell_pipe {
        return Some(
            "base64 decode piped to shell blocked -- potential command obfuscation".to_string(),
        );
    }

    // eval with args
    let tokens: Vec<&str> = cmd_lower.split_whitespace().collect();
    for (i, token) in tokens.iter().enumerate() {
        if *token == "eval" && i + 1 < tokens.len() {
            return Some("eval command blocked -- potential command injection vector".to_string());
        }
        // Inline interpreter execution
        let basename = token.rsplit('/').next().unwrap_or(token);
        let is_interpreter = matches!(
            basename,
            "python" | "python3" | "perl" | "ruby" | "node" | "nodejs"
        );
        if is_interpreter {
            if let Some(flag) = tokens.get(i + 1) {
                if *flag == "-c" || *flag == "-e" {
                    return Some(format!(
                        "Inline code execution via {} {} blocked -- use a script file instead",
                        token, flag
                    ));
                }
            }
        }
    }

    None
}

/// Detect `{{secret:...}}` or `{{secret-raw:...}}` placeholders in a string.
pub fn has_secret_placeholders(input: &str) -> bool {
    input.contains("{{secret:") || input.contains("{{secret-raw:")
}

/// Scrub known secret values from output text, replacing with [REDACTED].
pub fn scrub_output(output: &str, known_secrets: &[String]) -> String {
    let mut result = output.to_string();
    for secret in known_secrets {
        if secret.is_empty() {
            continue;
        }
        result = result.replace(secret.as_str(), "[REDACTED]");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gate_blocks_dangerous_commands() {
        assert!(check_dangerous_patterns("rm -rf /").is_some());
        assert!(check_dangerous_patterns("reboot").is_some());
        assert!(check_dangerous_patterns("git reset --hard").is_some());
    }

    #[test]
    fn test_gate_allows_safe_commands() {
        assert!(check_dangerous_patterns("ls -la").is_none());
        assert!(check_dangerous_patterns("cat file.txt").is_none());
        assert!(check_dangerous_patterns("git status").is_none());
    }

    #[test]
    fn test_secret_detection() {
        assert!(has_secret_placeholders("run {{secret:svc/key}}"));
        assert!(!has_secret_placeholders("run normal command"));
    }

    #[test]
    fn test_scrub_output() {
        let known = vec!["my-api-key-12345".to_string()];
        let result = scrub_output("the key is my-api-key-12345 here", &known);
        assert!(result.contains("[REDACTED]"));
        assert!(!result.contains("my-api-key-12345"));
    }

    #[tokio::test]
    async fn test_check_command_stores_gate_request() {
        use crate::db::Database;
        let db = Database::connect_memory().await.expect("in-memory db");
        let req = GateCheckRequest {
            command: "ls -la".to_string(),
            agent: "test-agent".to_string(),
            context: None,
        };
        let result = check_command(&db, &req, 1).await;
        assert!(result.is_ok());
        let res = result.unwrap();
        assert!(res.allowed);
        assert!(res.gate_id > 0);
    }

    #[tokio::test]
    async fn test_check_command_blocks_dangerous() {
        use crate::db::Database;
        let db = Database::connect_memory().await.expect("in-memory db");
        let req = GateCheckRequest {
            command: "rm -rf /".to_string(),
            agent: "test-agent".to_string(),
            context: None,
        };
        let result = check_command(&db, &req, 1).await.unwrap();
        assert!(!result.allowed);
        assert!(result.reason.is_some());
    }
}
