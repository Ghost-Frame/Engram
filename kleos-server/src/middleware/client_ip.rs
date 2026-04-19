//! Shared client-IP resolver used by rate-limit and audit middleware.
//!
//! SECURITY: X-Forwarded-For is attacker-controlled unless the TCP peer is a
//! proxy we deployed ourselves. Any middleware that keys on client IP --
//! rate limiting, audit, geoip, abuse detection -- must go through this
//! module so the trust boundary lives in one place.

use axum::extract::{ConnectInfo, Request};
use std::net::SocketAddr;

/// Resolve the caller's IP address for logging or rate-limit keying.
///
/// 1. Read the real TCP peer address from `ConnectInfo`.
/// 2. If (and only if) the peer matches one of the configured
///    `trusted_proxies`, honour the first hop of `X-Forwarded-For` (or
///    `X-Real-IP` as a fallback). An empty `trusted_proxies` list means
///    "no proxies are trusted", so XFF/XRI are ignored even if present.
/// 3. If `ConnectInfo` is missing (should not happen once the server is
///    installed with `into_make_service_with_connect_info`), return `None`
///    -- callers must NOT fall back to untrusted headers in that case.
pub fn client_ip(request: &Request, trusted_proxies: &[String]) -> Option<String> {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip().to_string())?;

    let peer_is_trusted =
        !trusted_proxies.is_empty() && trusted_proxies.iter().any(|tp| tp == &peer);

    if !peer_is_trusted {
        return Some(peer);
    }

    let headers = request.headers();
    let forwarded = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(String::from);

    if let Some(hop) = forwarded {
        return Some(hop);
    }

    let real_ip = headers
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(String::from);

    real_ip.or(Some(peer))
}

/// Return a rate-limit key of the form `ip:<resolved>`. Emits `ip:unknown`
/// and a warning when `ConnectInfo` is unavailable, so the limiter still
/// buckets all such requests together rather than bypassing them.
pub fn client_ip_key(request: &Request, trusted_proxies: &[String]) -> String {
    match client_ip(request, trusted_proxies) {
        Some(ip) => format!("ip:{}", ip),
        None => {
            tracing::warn!(
                "ConnectInfo<SocketAddr> not available; rate-limit key will be \"ip:unknown\""
            );
            "ip:unknown".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::Request as HttpRequest;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn build_request(peer: Option<SocketAddr>, headers: &[(&str, &str)]) -> Request {
        let mut builder = HttpRequest::builder().uri("/");
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let mut req = builder.body(Body::empty()).unwrap();
        if let Some(sa) = peer {
            req.extensions_mut().insert(ConnectInfo(sa));
        }
        req
    }

    #[test]
    fn direct_client_uses_peer_ip_and_ignores_xff() {
        let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9)), 40000);
        let req = build_request(Some(peer), &[("x-forwarded-for", "1.2.3.4")]);
        assert_eq!(client_ip(&req, &[]), Some("203.0.113.9".to_string()));
        assert_eq!(
            client_ip(&req, &["10.0.0.1".into()]),
            Some("203.0.113.9".to_string())
        );
    }

    #[test]
    fn trusted_proxy_honours_first_xff_hop() {
        let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 40000);
        let req = build_request(Some(peer), &[("x-forwarded-for", "198.51.100.7, 10.0.0.5")]);
        assert_eq!(
            client_ip(&req, &["10.0.0.1".into()]),
            Some("198.51.100.7".to_string())
        );
    }

    #[test]
    fn trusted_proxy_falls_back_to_x_real_ip_then_peer() {
        let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 40000);
        let req_xri = build_request(Some(peer), &[("x-real-ip", "198.51.100.42")]);
        assert_eq!(
            client_ip(&req_xri, &["10.0.0.1".into()]),
            Some("198.51.100.42".to_string())
        );

        let req_plain = build_request(Some(peer), &[]);
        assert_eq!(
            client_ip(&req_plain, &["10.0.0.1".into()]),
            Some("10.0.0.1".to_string())
        );
    }

    #[test]
    fn missing_connect_info_returns_none() {
        let req = build_request(None, &[("x-forwarded-for", "1.2.3.4")]);
        assert!(client_ip(&req, &["10.0.0.1".into()]).is_none());
        assert_eq!(client_ip_key(&req, &["10.0.0.1".into()]), "ip:unknown");
    }
}
