//! Network-safety helpers shared across the cred binaries.

use anyhow::{Context, Result};

/// Refuse to transmit a secret (the master key or owner key, sent as the credd
/// Bearer token) over a plaintext connection to a non-loopback host. Loopback
/// http and any https endpoint are allowed; remote http is rejected so the
/// credential cannot leak in cleartext to a remote credd.
pub fn guard_credd_transport(url: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(url).context("invalid CREDD_URL")?;
    if parsed.scheme() == "https" {
        return Ok(());
    }
    let host = parsed.host_str().unwrap_or("");
    // host_str() wraps an IPv6 literal in brackets ("[::1]"); strip them so it
    // parses as an IpAddr.
    let host_ip = host.trim_start_matches('[').trim_end_matches(']');
    let is_loopback = host.eq_ignore_ascii_case("localhost")
        || host_ip
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false);
    if is_loopback {
        return Ok(());
    }
    anyhow::bail!(
        "refusing to send credentials over plaintext http to non-loopback host '{host}'; \
         use an https CREDD_URL or a loopback address"
    )
}

#[cfg(test)]
mod tests {
    use super::guard_credd_transport;

    #[test]
    fn allows_loopback_http_and_https() {
        assert!(guard_credd_transport("http://127.0.0.1:4400").is_ok());
        assert!(guard_credd_transport("http://localhost:4400").is_ok());
        assert!(guard_credd_transport("http://[::1]:4400").is_ok());
        assert!(guard_credd_transport("https://credd.example.com").is_ok());
    }

    #[test]
    fn rejects_plaintext_non_loopback() {
        assert!(guard_credd_transport("http://credd.example.com").is_err());
        assert!(guard_credd_transport("http://10.0.0.5:4400").is_err());
    }
}
