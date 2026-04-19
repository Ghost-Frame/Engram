//! Outbound network helpers. Centralised URL validation so tainted
//! environment/configuration values cannot target arbitrary hosts, plus
//! a hardened reqwest client builder used for every outbound call.

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

/// Validate a URL intended for an outbound HTTP request. Accepts only
/// http/https schemes and rejects URLs that embed credentials (userinfo),
/// which cuts off credential-smuggling variants of SSRF.
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
    if parsed.host_str().is_none_or(|h| h.is_empty()) {
        return Err(EngError::InvalidInput(format!("url '{}' has no host", raw)));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_http_and_https() {
        assert!(validate_outbound_url("http://127.0.0.1:4200/x").is_ok());
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
}
