//! Mechanical pre-write screening. agent-forge cannot invoke a semantic
//! gatekeeper agent, so this module catches the categories a regex can catch
//! and the caller is told when a semantic pass is still required.

use crate::tools::ToolError;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Whether a dotted-quad token is an RFC1918 private address. Parsing beats a
/// regex here because the range checks are exact rather than approximated.
fn is_private_ipv4(token: &str) -> bool {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    let octets: Vec<u8> = match parts.iter().map(|p| p.parse::<u8>()).collect() {
        Ok(v) => v,
        Err(_) => return false,
    };
    match octets[0] {
        10 => true,
        192 => octets[1] == 168,
        172 => (16..=31).contains(&octets[1]),
        _ => false,
    }
}

/// Strip leading and trailing dots from a candidate run so a sentence-final
/// address like "10.0.0.1." still parses. Interior dots survive because they
/// separate the octets.
fn trim_token(token: &str) -> &str {
    token.trim_matches('.')
}

/// Whether a token begins with a concrete platform user-home path. Placeholder
/// segments remain publishable so documentation can describe portable examples
/// such as `/home/<user>/project` without embedding one machine's identity.
fn is_concrete_home_path(token: &str) -> bool {
    let normalized = token.replace('\\', "/");
    let lowered = normalized.to_ascii_lowercase();
    let uri_path = lowered.strip_prefix("file:").map(|lowered_path| {
        let raw_path = &normalized["file:".len()..];
        let localhost_prefix = "//localhost/";
        if lowered_path.starts_with(localhost_prefix) {
            &raw_path[localhost_prefix.len()..]
        } else {
            raw_path.trim_start_matches('/')
        }
    });
    let is_absolute = uri_path.is_some() || normalized.starts_with('/');
    let candidate = uri_path.unwrap_or_else(|| normalized.trim_start_matches('/'));
    let lowered_candidate = candidate.to_ascii_lowercase();
    let remainder = if is_absolute && lowered_candidate.starts_with("home/") {
        &candidate[5..]
    } else if is_absolute && lowered_candidate.starts_with("users/") {
        &candidate[6..]
    } else if is_absolute && (lowered_candidate == "root" || lowered_candidate.starts_with("root/"))
    {
        return true;
    } else if lowered_candidate.len() >= 9
        && lowered_candidate.as_bytes()[0].is_ascii_alphabetic()
        && &lowered_candidate[1..9] == ":/users/"
    {
        &candidate[9..]
    } else {
        return false;
    };

    let user = remainder.split('/').next().unwrap_or_default();
    !user.is_empty()
        && !user.starts_with('<')
        && !user.starts_with('$')
        && !user.starts_with('{')
        && !user.starts_with('%')
}

/// Scan emitted content for material that must never reach a public repository.
/// Returns one human-readable finding per detection. An empty result means the
/// mechanical checks passed; it does not mean the content is safe, which is why
/// callers still require a semantic pass on public repositories.
///
/// KNOWN FALSE POSITIVE, accepted deliberately: a four-part numeric run whose
/// first segment is 10 is reported as a private address even when it is really
/// a version or a section number, so "assembly version 10.20.30.1" and "see
/// Section 10.5.0.1" both trip the gate. Distinguishing them mechanically would
/// require guessing at surrounding prose, and this module is a reliable floor
/// rather than a clever one. The failure is in the safe direction: emission is
/// refused, the finding names the exact string, and an operator resolves it in
/// a moment.
pub fn scan_for_leaks(content: &str) -> Vec<String> {
    let mut findings = Vec::new();

    // Split on anything that cannot appear inside a dotted quad, so an address
    // is found wherever it sits. Splitting on whitespace and trimming only the
    // ends misses every form where the punctuation is INSIDE the token:
    // `host=10.0.0.1`, `10.0.0.1:8080`, `git@10.0.0.1:org/repo`, a comma
    // separated list, or a markdown link. Those are ordinary content for an
    // infrastructure document, so missing them would make this gate unreliable
    // at the one category it most exists to catch.
    for run in content.split(|c: char| !c.is_ascii_digit() && c != '.') {
        let token = trim_token(run);
        if is_private_ipv4(token) {
            findings.push(format!("private address: {}", token));
        }
    }

    for token in content.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '=' | '(' | ')' | '[' | ']' | '"' | '\'' | '`' | ',' | ';'
            )
    }) {
        if is_concrete_home_path(token) {
            findings.push("absolute home path".to_string());
            break;
        }
    }

    if content.contains("PRIVATE KEY-----") {
        findings.push("private key block".to_string());
    }

    let lowered = content.to_lowercase();
    for marker in ["api_key", "apikey", "password", "secret_key", "token"] {
        for (idx, _) in lowered.match_indices(marker) {
            // Require a word boundary before the marker so compound identifiers
            // such as `trim_token` or `detokenize` do not masquerade as one.
            let preceded_by_word_char = idx > 0
                && lowered[..idx]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
            if preceded_by_word_char {
                continue;
            }

            let rest = &lowered[idx + marker.len()..];
            let rest = rest.trim_start();
            if rest.starts_with('=') || rest.starts_with(':') {
                let value = rest.trim_start_matches(['=', ':']).trim_start();
                // A bare mention such as "password: none recorded" is prose, not
                // a credential. Require a value with some length and no spaces.
                let candidate: String = value.chars().take_while(|c| !c.is_whitespace()).collect();
                // `token` is the only marker that appears constantly in ordinary
                // prose about sessions and auth, so it alone requires the value
                // to LOOK generated -- long, or mixing letters and digits. A
                // gate that refuses its own documentation gets switched off, and
                // a gate that is switched off protects nothing.
                //
                // The other markers keep the plain length rule. Applying the
                // shape check to them would clear `password=letmein` and
                // `api_key=abcdefghijk`, and a short all-lowercase password is
                // precisely the shape real leaks take. Suppressing prose noise
                // must not cost detection of the weakest secrets.
                let credential_shaped = if marker == "token" {
                    let has_digit = candidate.chars().any(|c| c.is_ascii_digit());
                    let has_alpha = candidate.chars().any(|c| c.is_ascii_alphabetic());
                    candidate.len() >= 12 || (has_digit && has_alpha)
                } else {
                    true
                };
                if candidate.len() >= 6 && credential_shaped {
                    findings.push(format!("credential-shaped assignment: {}", marker));
                    break;
                }
            }
        }
    }

    findings.sort();
    findings.dedup();
    findings
}

/// Refuse to emit content that trips the leak scan, naming the findings in the
/// error. Every emission path shares this so the decision to refuse, and the
/// wording of the refusal, live in exactly one place.
pub fn guard_no_leaks(content: &str) -> Result<(), ToolError> {
    let findings = scan_for_leaks(content);
    if findings.is_empty() {
        return Ok(());
    }
    Err(ToolError::InvalidValue(format!(
        "refusing to emit: leak scan found {}",
        findings.join(", ")
    )))
}

/// Best-effort check of whether the repository at `repo_root` is public.
/// Returns true when visibility cannot be determined, so an unknown repository
/// is screened as if it were public. Failing safe is the whole point. Waits at
/// most `VISIBILITY_TIMEOUT` for `gh` to answer; a call that has not finished by
/// the deadline is killed and also resolves to true, so a stalled network call
/// cannot hang the caller.
pub fn is_public_repo(repo_root: &Path) -> bool {
    // Wait out the subprocess rather than blocking forever. `gh` makes a network
    // call, and this runs on every emitting checkpoint, so a stalled call would
    // hang the agent's critical path. Every failure mode -- missing gh, non-zero
    // exit, timeout -- falls through to `true`, which screens the content. The
    // gate erring toward more screening is the safe direction.
    const VISIBILITY_TIMEOUT: Duration = Duration::from_secs(5);

    let Ok(mut child) = Command::new("gh")
        .args(["repo", "view", "--json", "visibility", "-q", ".visibility"])
        .current_dir(repo_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return true;
    };

    let deadline = Instant::now() + VISIBILITY_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return true;
                }
                let mut out = String::new();
                if let Some(mut stdout) = child.stdout.take() {
                    if stdout.read_to_string(&mut out).is_err() {
                        return true;
                    }
                }
                return out.trim().to_uppercase() != "PRIVATE";
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return true;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => return true,
        }
    }
}

#[cfg(test)]
/// Tests for the mechanical leak scan.
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Private RFC1918 addresses are flagged in all three ranges.
    #[test]
    fn flags_private_ipv4_ranges() {
        assert!(!scan_for_leaks("host at 10.0.0.1 today").is_empty());
        assert!(!scan_for_leaks("host at 192.168.1.1 today").is_empty());
        assert!(!scan_for_leaks("host at 172.16.0.5 today").is_empty());
    }

    /// A public address is not flagged, so ordinary prose stays publishable.
    #[test]
    fn ignores_public_ipv4() {
        assert!(scan_for_leaks("resolved 8.8.8.8 fine").is_empty());
    }

    /// Concrete Linux, macOS, root, and Windows home paths are private machine
    /// details and are flagged wherever a verification command contains them.
    #[test]
    fn flags_concrete_home_paths() {
        assert!(!scan_for_leaks("test -f /home/alice/project/file").is_empty());
        assert!(!scan_for_leaks("cat /Users/alice/project/file").is_empty());
        assert!(!scan_for_leaks("cat /root/private/file").is_empty());
        assert!(!scan_for_leaks(r"type C:\Users\Alice\project\file").is_empty());
        assert!(!scan_for_leaks("path=/home/alice/project/file").is_empty());
        assert!(!scan_for_leaks("open file:///home/alice/project/file").is_empty());
        assert!(!scan_for_leaks("open file:///Users/alice/project/file").is_empty());
        assert!(!scan_for_leaks("open file:///C:/Users/Alice/project/file").is_empty());
        assert!(!scan_for_leaks("[evidence](file:///home/alice/check)").is_empty());
    }

    /// Portable placeholders, repository-relative paths, and public URL routes
    /// remain publishable because they do not identify a local machine account.
    #[test]
    fn ignores_portable_path_examples() {
        assert!(scan_for_leaks("open docs/agent-forge/record.md").is_empty());
        assert!(scan_for_leaks("see /home/<user>/project").is_empty());
        assert!(scan_for_leaks("see https://example.com/users/alice").is_empty());
    }

    /// Private key material is flagged.
    #[test]
    fn flags_private_key_blocks() {
        assert!(!scan_for_leaks("-----BEGIN OPENSSH PRIVATE KEY-----").is_empty());
    }

    /// Assignments that look like credentials are flagged.
    #[test]
    fn flags_credential_assignments() {
        assert!(!scan_for_leaks("api_key=abc123def").is_empty());
        assert!(!scan_for_leaks("password = hunter2").is_empty());
    }

    /// Clean technical prose produces no findings.
    #[test]
    fn clean_content_produces_no_findings() {
        let md = "# Record: Add a thing\n\n- **why:** it was simpler.\n";
        assert!(scan_for_leaks(md).is_empty());
    }

    /// An address abutted by sentence punctuation is still detected. Prose ends
    /// sentences with a period, and a trailing dot must not hide a leak.
    #[test]
    fn flags_addresses_abutted_by_punctuation() {
        assert!(!scan_for_leaks("it talks to 10.0.0.1.").is_empty());
        assert!(!scan_for_leaks("see (192.168.1.1), the host").is_empty());
    }

    /// An address is caught wherever it is embedded, not only when surrounded by
    /// whitespace. Every form here is ordinary content for an infrastructure
    /// document, and every one of them defeated the original whitespace-split
    /// scan because the punctuation sits inside the token rather than at its
    /// edges. This is the regression that matters most in this module.
    #[test]
    fn flags_addresses_embedded_in_punctuation() {
        assert!(!scan_for_leaks("host=10.0.0.1").is_empty());
        assert!(!scan_for_leaks("connect to 10.0.0.1:4200 now").is_empty());
        assert!(!scan_for_leaks("git@10.0.0.1:org/repo.git").is_empty());
        assert!(!scan_for_leaks("10.0.0.1,10.0.0.2").is_empty());
        assert!(!scan_for_leaks("[server](http://192.168.1.1/path)").is_empty());
    }

    /// Ordinary prose about tokens is not a credential. This module and the
    /// documents it screens discuss session and auth tokens constantly, so a
    /// gate that refused them would be switched off, and a gate that is switched
    /// off protects nothing.
    #[test]
    fn ignores_token_prose() {
        assert!(scan_for_leaks("the session token: expired overnight").is_empty());
        assert!(scan_for_leaks("auth token: revoked by the server").is_empty());
    }

    /// A marker inside a larger identifier is not a credential assignment, even
    /// when the value after it would otherwise pass the shape check.
    ///
    /// The first case is what makes this test worth having. An earlier version
    /// used only prose values, which the shape check alone already rejected, so
    /// deleting the word-boundary guard entirely would not have failed a single
    /// test. `mytoken=abc123def456` passes the shape check, so only the boundary
    /// guard can suppress it -- which means this assertion actually pins that
    /// guard rather than shadowing it.
    #[test]
    fn ignores_marker_inside_an_identifier() {
        assert!(scan_for_leaks("mytoken=abc123def456").is_empty());
        assert!(scan_for_leaks("fn trim_token: helper for the scan").is_empty());
    }

    /// A short all-alphabetic secret is still a secret. Only `token` gets the
    /// generated-shape check, because only `token` is common in prose; applying
    /// that check to every marker would clear a weak password, which is the
    /// shape real leaks most often take.
    #[test]
    fn flags_short_alphabetic_secrets() {
        assert!(!scan_for_leaks("password=letmein").is_empty());
        assert!(!scan_for_leaks("api_key=abcdefghijk").is_empty());
    }

    /// The shared guard passes clean content through.
    #[test]
    fn guard_allows_clean_content() {
        assert!(guard_no_leaks("nothing to see here").is_ok());
    }

    /// The shared guard refuses leaking content and names the findings.
    #[test]
    fn guard_refuses_leaking_content() {
        let err = guard_no_leaks("host 10.0.0.1").unwrap_err();
        assert!(err.to_string().contains("refusing to emit"));
    }

    /// Visibility detection fails safe. A directory that is not a repository at
    /// all cannot be shown to be private, so it must be screened as if public.
    /// The gate erring toward more screening is the safe direction.
    #[test]
    fn unknown_visibility_is_treated_as_public() {
        let dir = tempdir().unwrap();
        assert!(is_public_repo(dir.path()));
    }
}
