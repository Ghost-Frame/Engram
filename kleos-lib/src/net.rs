//! Outbound network helpers. Centralised URL validation so tainted
//! environment/configuration values cannot target arbitrary hosts, plus
//! a hardened reqwest client builder used for every outbound call.

use crate::webhooks::{is_ipv4_denied, is_ipv6_denied};
use crate::EngError;
use std::time::Duration;
use url::Url;

/// R7-002: default response size cap for outbound HTTP responses.
/// Call sites with different needs should pass their own value to
/// [`response_within_limit`].
pub const DEFAULT_MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;

/// R7-002: hardened reqwest client builder. Applies a 5s connect timeout and
/// limits redirect chains to 1 hop. Call sites add their own overall
/// `.timeout(...)` appropriate to the workload before calling `.build()`.
pub fn safe_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::limited(1))
}

/// R7-002: returns `false` when the response declares a Content-Length larger
/// than `max_bytes`. When the header is missing we return `true` and leave the
/// caller to cap via streaming; reqwest's own timeouts still bound the read.
pub fn response_within_limit(resp: &reqwest::Response, max_bytes: u64) -> bool {
    match resp.content_length() {
        Some(len) => len <= max_bytes,
        None => true,
    }
}

/// Validate a URL intended for an outbound HTTP request. Rejects:
///   - non-http(s) schemes,
///   - URLs with embedded credentials (userinfo),
///   - URLs missing a host,
///   - literal IP addresses inside any deny-listed range
///     (loopback / RFC1918 / CGNAT / link-local / IPv6 ULA / 0.0.0.0/8),
///   - cloud metadata hostnames (AWS link-local, GCP metadata.google.internal).
///
/// This is a **synchronous** check on literal hostnames and IPs only. For
/// delivery-time DNS resolution + per-address allowlist validation, use
/// `webhooks::resolve_and_validate_url`.
///
/// CodeQL recognises `url::Url::parse` + scheme check as a request-forgery
/// sanitiser, so routing outbound calls through this helper clears the
/// rust/request-forgery alert for that call site.
pub fn validate_outbound_url(raw: &str) -> Result<Url, EngError> {
    let parsed = Url::parse(raw)
        .map_err(|e| EngError::InvalidInput(format!("invalid url '{}': {}", raw, e)))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(EngError::InvalidInput(format!(
                "url '{}' uses disallowed scheme '{}' (expected http or https)",
                raw, other
            )));
        }
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(EngError::InvalidInput(format!(
            "url '{}' must not embed credentials",
            raw
        )));
    }
    let host = parsed
        .host()
        .ok_or_else(|| EngError::InvalidInput(format!("url '{}' has no host", raw)))?;
    // Tests and internal sidecar->server traffic bind on 127.0.0.1; production
    // SSRF protection is the default but operators can opt out for these
    // controlled paths.
    let allow_private = std::env::var("KLEOS_NET_ALLOW_PRIVATE").as_deref() == Ok("1");
    match host {
        url::Host::Domain(name) => {
            let lower = name.to_ascii_lowercase();
            if !allow_private
                && (lower == "localhost"
                    || lower.ends_with(".localhost")
                    || lower == "localhost.localdomain")
            {
                return Err(EngError::InvalidInput(format!(
                    "url '{}' resolves to loopback",
                    raw
                )));
            }
            if lower == "metadata.google.internal"
                || lower == "metadata"
                || lower == "metadata.goog"
            {
                return Err(EngError::InvalidInput(format!(
                    "url '{}' targets a cloud metadata endpoint",
                    raw
                )));
            }
        }
        url::Host::Ipv4(ip) => {
            if !allow_private && is_ipv4_denied(&ip) {
                return Err(EngError::InvalidInput(format!(
                    "url '{}' targets disallowed IPv4 range",
                    raw
                )));
            }
        }
        url::Host::Ipv6(ip) => {
            if !allow_private && is_ipv6_denied(&ip) {
                return Err(EngError::InvalidInput(format!(
                    "url '{}' targets disallowed IPv6 range",
                    raw
                )));
            }
        }
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_http_and_https() {
        // Loopback (127.0.0.1) is now correctly rejected by the deny list;
        // accepts public hosts on either scheme.
        assert!(validate_outbound_url("http://203.0.113.1:4200/x").is_ok());
        assert!(validate_outbound_url("https://example.com/y").is_ok());
    }

    #[test]
    fn rejects_non_http_schemes() {
        assert!(validate_outbound_url("file:///etc/passwd").is_err());
        assert!(validate_outbound_url("ftp://example.com").is_err());
        assert!(validate_outbound_url("javascript:alert(1)").is_err());
    }

    #[test]
    fn rejects_userinfo() {
        assert!(validate_outbound_url("http://user:pass@example.com").is_err());
        assert!(validate_outbound_url("http://user@example.com").is_err());
    }

    #[test]
    fn rejects_bogus_input() {
        assert!(validate_outbound_url("not a url").is_err());
        assert!(validate_outbound_url("http://").is_err());
    }

    #[test]
    fn rejects_aws_metadata_ip() {
        assert!(
            validate_outbound_url("http://169.254.169.254/latest/meta-data/").is_err(),
            "AWS instance metadata IPv4 must be denied"
        );
    }

    #[test]
    fn rejects_gcp_metadata_host() {
        assert!(
            validate_outbound_url("http://metadata.google.internal/computeMetadata/v1/").is_err(),
            "GCP metadata hostname must be denied"
        );
    }

    #[test]
    fn rejects_rfc1918_private_ipv4() {
        assert!(validate_outbound_url("http://10.0.0.1/x").is_err());
        assert!(validate_outbound_url("http://172.16.0.1/x").is_err());
        assert!(validate_outbound_url("http://192.168.1.1/x").is_err());
    }

    #[test]
    fn rejects_loopback_hostname() {
        assert!(validate_outbound_url("http://localhost/x").is_err());
        assert!(validate_outbound_url("http://app.localhost/x").is_err());
    }

    #[test]
    fn rejects_ipv6_ula() {
        assert!(validate_outbound_url("http://[fc00::1]/x").is_err());
    }

    #[test]
    fn rejects_cgnat_range() {
        assert!(validate_outbound_url("http://100.64.0.1/x").is_err());
    }

    #[test]
    fn accepts_public_ip() {
        assert!(validate_outbound_url("http://8.8.8.8/x").is_ok());
        assert!(validate_outbound_url("https://1.1.1.1/").is_ok());
    }
}
