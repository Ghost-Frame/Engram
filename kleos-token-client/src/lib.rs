//! Keyless Kleos bearer fetch via the phylaxd SO_PEERCRED token broker.
//!
//! `POST /phylax/kleos/token` over the credd Unix socket mints a short-lived,
//! identity-signed Kleos bearer, authenticated purely by the kernel-verified
//! peer UID (no key on disk, no `CREDD_AGENT_KEY`). Bearer-only clients
//! (kleos-sh, kr/kw/ke, agent-forge, eidolon-supervisor) call
//! [`resolve_via_phylax_broker`] to obtain a `KLEOS_API_KEY` value without a
//! static per-host key.
//!
//! Intentionally synchronous and dependency-free (std only) so the lightweight
//! tools that link it stay lightweight.

/// Resolve a short-lived Kleos bearer from the phylaxd SO_PEERCRED broker over
/// the credd Unix socket. Returns `None` on any failure (socket absent, broker
/// not running, non-2xx, malformed body) so callers fall through to their next
/// auth source.
///
/// Authenticated purely by the kernel-verified peer UID: no key or
/// `CREDD_AGENT_KEY` is required. The returned token is short-lived (the broker
/// caps it, default 300s) and capped at read,write.
#[cfg(unix)]
pub fn resolve_via_phylax_broker() -> Option<String> {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    let sock = socket_path()?;

    // The broker reads scopes from the JSON body and caps them at read,write.
    let body = br#"{"scopes":"read,write"}"#;
    let request = format!(
        "POST /phylax/kleos/token HTTP/1.1\r\n\
         Host: localhost\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n",
        body.len()
    );

    let mut stream = UnixStream::connect(&sock).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .ok()?;
    stream.write_all(request.as_bytes()).ok()?;
    stream.write_all(body).ok()?;

    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    parse_token_from_http(&response)
}

/// Non-unix stub: the broker is reachable only over a Unix socket.
#[cfg(not(unix))]
pub fn resolve_via_phylax_broker() -> Option<String> {
    None
}

/// Resolve the credd socket path: `CREDD_SOCKET` if set, else
/// `$XDG_RUNTIME_DIR/credd.sock`.
fn socket_path() -> Option<String> {
    if let Ok(s) = std::env::var("CREDD_SOCKET") {
        if !s.is_empty() {
            return Some(s);
        }
    }
    let runtime = std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .filter(|s| !s.is_empty())?;
    Some(format!("{}/credd.sock", runtime.trim_end_matches('/')))
}

/// Extract the `token` field from a raw HTTP/1.1 response (headers + JSON body).
///
/// Returns `None` if the status is not 2xx, the body is missing, or there is no
/// non-empty `token` string. Pure and std-only for unit testing.
fn parse_token_from_http(raw: &str) -> Option<String> {
    // Status line must indicate success (e.g. "HTTP/1.1 200 OK").
    let status_ok = raw
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .map(|code| code.starts_with('2'))
        .unwrap_or(false);
    if !status_ok {
        return None;
    }
    // Body follows the blank line separating headers from content.
    let body = raw
        .split("\r\n\r\n")
        .nth(1)
        .or_else(|| raw.split("\n\n").nth(1))?;
    extract_json_string_field(body, "token")
}

/// Extract a JSON string field value by name without a JSON parser. The token
/// value is base64url + dots (no quotes or escapes), so a quote-delimited scan
/// is safe here.
fn extract_json_string_field(body: &str, field: &str) -> Option<String> {
    let needle = format!("\"{field}\"");
    let after_key = &body[body.find(&needle)? + needle.len()..];
    let after_colon = &after_key[after_key.find(':')? + 1..];
    let value_start = after_colon.trim_start().strip_prefix('"')?;
    let end = value_start.find('"')?;
    let value = &value_start[..end];
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 200 response with a token body yields the token string.
    #[test]
    fn parses_token_from_ok_response() {
        let raw = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\r\n\
                   {\"token\":\"kleos.abc.def\",\"expires_in\":300}";
        assert_eq!(
            parse_token_from_http(raw),
            Some("kleos.abc.def".to_string())
        );
    }

    /// A non-2xx response yields nothing even if a body is present.
    #[test]
    fn rejects_non_2xx_response() {
        let raw = "HTTP/1.1 403 Forbidden\r\n\r\n{\"token\":\"kleos.x.y\"}";
        assert_eq!(parse_token_from_http(raw), None);
    }

    /// An empty token field yields nothing.
    #[test]
    fn rejects_empty_token() {
        let raw = "HTTP/1.1 200 OK\r\n\r\n{\"token\":\"\"}";
        assert_eq!(parse_token_from_http(raw), None);
    }

    /// A body with no token field yields nothing.
    #[test]
    fn rejects_missing_token_field() {
        let raw = "HTTP/1.1 200 OK\r\n\r\n{\"error\":\"nope\"}";
        assert_eq!(parse_token_from_http(raw), None);
    }

    /// The token field appearing after another field is still extracted.
    #[test]
    fn parses_token_not_first_field() {
        let raw = "HTTP/1.1 200 OK\r\n\r\n{\"expires_in\":300,\"token\":\"kleos.q.r\"}";
        assert_eq!(parse_token_from_http(raw), Some("kleos.q.r".to_string()));
    }
}
