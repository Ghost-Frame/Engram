/// Parsed representation of an SSH command target.
#[derive(Debug, Clone)]
pub struct SshTarget {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
}

/// True when a command token invokes ssh: matched by basename so absolute
/// invocations (`/usr/bin/ssh`, `/opt/homebrew/bin/ssh`) are recognized.
/// Exact-token matching let any pathed invocation bypass SSRF detection
/// entirely, which is the security-relevant consumer of this parser.
fn is_ssh_token(token: &str) -> bool {
    token == "ssh" || token.rsplit('/').next() == Some("ssh")
}

/// Parse an SSH command string to extract the target host, user, and port.
/// Used for SSRF detection and server map lookups. Leading environment
/// wrappers (`env`, `VAR=value` assignments) before the ssh token are
/// skipped by the token scan itself.
pub fn parse_ssh_target(command: &str) -> Option<SshTarget> {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    let ssh_pos = tokens.iter().position(|&t| is_ssh_token(t))?;

    let mut host_raw: Option<&str> = None;
    let mut port: Option<u16> = None;
    let mut i = ssh_pos + 1;

    while i < tokens.len() {
        let t = tokens[i];
        if t == "-p" || t == "-P" {
            i += 1;
            if i < tokens.len() {
                port = tokens[i].parse::<u16>().ok();
            }
        } else if t.starts_with('-') {
            // Skip flags that take an argument
            if matches!(t, "-i" | "-l" | "-o" | "-L" | "-R" | "-D" | "-J" | "-W") {
                i += 1;
            }
        } else if !t.contains('=') {
            host_raw = Some(t);
            break;
        }
        i += 1;
    }

    let host_raw = host_raw?;
    let (user, host) = if let Some(pos) = host_raw.rfind('@') {
        (
            Some(host_raw[..pos].to_string()),
            host_raw[pos + 1..].to_string(),
        )
    } else {
        (None, host_raw.to_string())
    };

    Some(SshTarget { user, host, port })
}

/// Generate enrichment context for a systemctl command.
/// Returns a human-readable description of the action and service name if parseable.
/// Used to inject context into gate responses.
pub fn check_systemctl_command(command: &str) -> Option<String> {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    let systemctl_pos = tokens.iter().position(|&t| t == "systemctl")?;

    let action = tokens.get(systemctl_pos + 1).copied().unwrap_or("");
    let service = tokens
        .iter()
        .skip(systemctl_pos + 2)
        .find(|&&t| !t.starts_with('-'));

    let service = service.copied()?;

    Some(format!(
        "systemctl {} {} - verify restart order and service dependencies before proceeding",
        action, service
    ))
}

/// Detect `{{secret:...}}` or `{{secret-raw:...}}` placeholders in a string.
pub fn has_secret_placeholders(input: &str) -> bool {
    input.contains("{{secret:") || input.contains("{{secret-raw:")
}
