//! Three-tier resolve handlers.
//!
//! - Substitution: Replace `{{secret:category/name}}` placeholders in text
//! - Proxy: Inject credentials into HTTP request headers/body
//! - Raw: Return decrypted secret data directly

pub use super::types::{
    ProxyRequest, ProxyResponse, RawRequest, ResolveTextRequest, ResolveTextResponse,
};

use axum::{extract::State, Json};
use serde_json::{json, Value};

use kleos_cred::audit::{log_audit, AccessTier, AuditAction};
use kleos_cred::CredError;

use crate::auth::Auth;
use crate::handlers::AppError;
use crate::state::AppState;

/// Response headers that must not be relayed from the upstream service back
/// to the proxy caller. They carry upstream auth/session state (cookies,
/// challenge headers) scoped to the credd<->upstream leg only.
const STRIPPED_RESPONSE_HEADERS: &[&str] = &[
    "set-cookie",
    "set-cookie2",
    "www-authenticate",
    "proxy-authenticate",
    "authorization",
    "proxy-authorization",
];

/// True when an upstream response header should be dropped before forwarding.
/// Comparison is case-insensitive; reqwest already lowercases header names.
fn is_stripped_response_header(name: &str) -> bool {
    STRIPPED_RESPONSE_HEADERS
        .iter()
        .any(|h| name.eq_ignore_ascii_case(h))
}

/// Request headers the proxy must never forward verbatim from the caller. These
/// are hop-by-hop or message-framing headers (RFC 7230 SS6.1 plus length /
/// encoding controls); relaying caller-controlled values enables request
/// smuggling / desync and lets the caller override the `Host` of our pinned
/// connection. `host`, `content-length`, and the auth header are set by the
/// client or our own injection, never by the caller.
const STRIPPED_REQUEST_HEADERS: &[&str] = &[
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authorization",
    "te",
    "trailer",
    "upgrade",
    "expect",
];

/// True when a caller-supplied request header must be dropped before the proxy
/// forwards it upstream. Case-insensitive.
fn is_stripped_request_header(name: &str) -> bool {
    STRIPPED_REQUEST_HEADERS
        .iter()
        .any(|h| name.eq_ignore_ascii_case(h))
}

/// Pattern for secret placeholders: {{secret:category/name}} or {{secret:category/name.field}}
fn find_placeholders(text: &str) -> Vec<(usize, usize, String, String, Option<String>)> {
    let mut results = Vec::new();
    let mut start = 0;

    while let Some(begin) = text[start..].find("{{secret:") {
        let abs_begin = start + begin;
        if let Some(end_rel) = text[abs_begin..].find("}}") {
            let abs_end = abs_begin + end_rel + 2;
            let inner = &text[abs_begin + 9..abs_end - 2]; // Skip "{{secret:" and "}}"

            // Parse category/name or category/name.field
            if let Some(slash_pos) = inner.find('/') {
                let category = &inner[..slash_pos];
                let rest = &inner[slash_pos + 1..];

                let (name, field) = if let Some(dot_pos) = rest.find('.') {
                    (&rest[..dot_pos], Some(rest[dot_pos + 1..].to_string()))
                } else {
                    (rest, None)
                };

                results.push((
                    abs_begin,
                    abs_end,
                    category.to_string(),
                    name.to_string(),
                    field,
                ));
            }

            start = abs_end;
        } else {
            break;
        }
    }

    results
}

/// Resolve secret placeholders in text.
#[tracing::instrument(skip_all, fields(handler = "credd.resolve.text"))]
pub async fn resolve_text_handler(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Json(body): Json<ResolveTextRequest>,
) -> Result<Json<ResolveTextResponse>, AppError> {
    // Text substitution returns plaintext secret bytes in the response body,
    // so it is a plaintext tier and requires the same privilege as raw
    // retrieval. Non-raw agents (and bootstrap agents) must use proxy
    // injection, which never returns the secret value to the caller.
    if !auth.can_access_raw() {
        log_audit(
            &state.db,
            auth.user_id(),
            auth.agent_name(),
            AuditAction::Resolve,
            "",
            "",
            Some(AccessTier::Substitution),
            false,
        )
        .await?;
        return Err(CredError::PermissionDenied(
            "text resolve exposes plaintext and requires raw access; use proxy resolve".into(),
        )
        .into());
    }

    let placeholders = find_placeholders(&body.text);
    let mut result = body.text.clone();
    let mut offset: isize = 0;
    let mut substitutions = 0;
    let mut denied_categories: Vec<String> = Vec::new();

    for (start, end, category, name, field) in placeholders {
        if !auth.can_access_category(&category) {
            log_audit(
                &state.db,
                auth.user_id(),
                auth.agent_name(),
                AuditAction::Resolve,
                &category,
                &name,
                Some(AccessTier::Substitution),
                false,
            )
            .await?;
            denied_categories.push(category);
            continue;
        }

        let secret_result =
            super::get_secret_with_fallback(&state, auth.user_id(), &category, &name).await;

        match secret_result {
            Ok((_row, data)) => {
                let value = match field {
                    Some(ref f) => data.get_field(f).unwrap_or_default(),
                    None => data.primary_value(),
                };

                let adj_start_signed = start as isize + offset;
                let adj_end_signed = end as isize + offset;
                if adj_start_signed < 0
                    || adj_end_signed < 0
                    || adj_end_signed as usize > result.len()
                    || !result.is_char_boundary(adj_start_signed as usize)
                    || !result.is_char_boundary(adj_end_signed as usize)
                {
                    continue;
                }
                let adj_start = adj_start_signed as usize;
                let adj_end = adj_end_signed as usize;
                result.replace_range(adj_start..adj_end, &value);
                offset += value.len() as isize - (end - start) as isize;
                substitutions += 1;

                log_audit(
                    &state.db,
                    auth.user_id(),
                    auth.agent_name(),
                    AuditAction::Resolve,
                    &category,
                    &name,
                    Some(AccessTier::Substitution),
                    true,
                )
                .await?;
            }
            Err(_) => {
                log_audit(
                    &state.db,
                    auth.user_id(),
                    auth.agent_name(),
                    AuditAction::Resolve,
                    &category,
                    &name,
                    Some(AccessTier::Substitution),
                    false,
                )
                .await?;
            }
        }
    }

    // Fail the entire request if any placeholder was denied -- never return
    // partially substituted text with unresolved placeholders visible.
    if !denied_categories.is_empty() {
        denied_categories.sort();
        denied_categories.dedup();
        return Err(kleos_cred::CredError::PermissionDenied(format!(
            "access denied for categories: {}",
            denied_categories.join(", ")
        ))
        .into());
    }

    Ok(Json(ResolveTextResponse {
        text: result,
        substitutions,
    }))
}

/// Proxy HTTP request with injected credentials.
///
/// SECURITY: validates the target URL against SSRF deny lists (loopback,
/// RFC1918 private, link-local, cloud metadata) before making the outbound
/// request, which carries injected secret headers.
#[tracing::instrument(skip_all, fields(handler = "credd.resolve.proxy"))]
pub async fn proxy_handler(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Json(req): Json<ProxyRequest>,
) -> Result<Json<ProxyResponse>, AppError> {
    // SECURITY (SSRF-DNS): SSRF validation -- reject requests targeting
    // loopback, private, link-local, and cloud metadata addresses. Resolves
    // DNS so domains pointing at private IPs are also caught. The proxy
    // injects secret headers so an unvalidated URL would let an attacker
    // exfiltrate credentials to internal services.
    // Capture the address the hostname validated to. reqwest re-resolves DNS
    // at request time, so without pinning an attacker could rebind the host to
    // an internal IP between this check and the request (TOCTOU rebinding). We
    // pin reqwest's resolver to this IP when building the client below, keeping
    // the original hostname for TLS SNI and certificate validation. `None`
    // means the URL already held a literal (already-validated) IP.
    let pinned_ip = kleos_lib::webhooks::resolve_and_validate_url(&req.url)
        .await
        .map_err(|e| CredError::InvalidInput(format!("proxy target URL rejected: {}", e)))?;

    // SECURITY (H4): per-category domain binding. When an allowlist is
    // configured, only forward credentials to explicitly permitted domains.
    if let Some(allowlist) = &state.proxy_domain_allowlist {
        let target_host = url::Url::parse(&req.url)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_lowercase()));
        let target_host = target_host.as_deref().unwrap_or("");
        let allowed_domains = allowlist
            .get(&req.secret_category)
            .or_else(|| allowlist.get("*"));
        let permitted = match allowed_domains {
            Some(domains) => domains.iter().any(|pattern| {
                if pattern == "*" {
                    true
                } else if let Some(suffix) = pattern.strip_prefix("*.") {
                    target_host == suffix || target_host.ends_with(&format!(".{}", suffix))
                } else {
                    target_host == pattern
                }
            }),
            None => false,
        };
        if !permitted {
            return Err(CredError::PermissionDenied(format!(
                "proxy target domain '{}' not in allowlist for category '{}'",
                target_host, req.secret_category
            ))
            .into());
        }
    } else if std::env::var("CREDD_PROXY_STRICT").as_deref() == Ok("1") {
        return Err(CredError::PermissionDenied(
            "proxy denied: no domain allowlist configured and CREDD_PROXY_STRICT=1 is set".into(),
        )
        .into());
    }

    if !auth.can_access_category(&req.secret_category) {
        log_audit(
            &state.db,
            auth.user_id(),
            auth.agent_name(),
            AuditAction::Proxy,
            &req.secret_category,
            &req.secret_name,
            Some(AccessTier::Proxy),
            false,
        )
        .await?;
        return Err(CredError::PermissionDenied(format!(
            "no access to category: {}",
            req.secret_category
        ))
        .into());
    }

    let (_row, data) = super::get_secret_with_fallback(
        &state,
        auth.user_id(),
        &req.secret_category,
        &req.secret_name,
    )
    .await?;

    let secret_value = data.primary_value();

    let method = req
        .method
        .as_deref()
        .unwrap_or("GET")
        .parse::<reqwest::Method>()
        .map_err(|e| CredError::InvalidInput(format!("invalid method: {}", e)))?;

    let header_name = req
        .auth_header
        .clone()
        .unwrap_or_else(|| "Authorization".to_string());
    let header_value = match req.auth_scheme.as_deref() {
        Some("") => secret_value,
        Some(scheme) => format!("{} {}", scheme.trim(), secret_value),
        None => format!("Bearer {}", secret_value),
    };

    // SECURITY (SEC-H2): disable redirect following to prevent Authorization
    // header leakage to attacker-controlled hosts via redirect chains.
    let mut client_builder = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none());

    // SECURITY (SSRF-DNS): pin the validated IP so reqwest does not re-resolve
    // the hostname and cannot be steered to an internal address by a rebind.
    if let Some(ip) = pinned_ip {
        let parsed = url::Url::parse(&req.url)
            .map_err(|e| CredError::InvalidInput(format!("invalid proxy URL: {}", e)))?;
        if let Some(host) = parsed.host_str() {
            let port = parsed.port_or_known_default().unwrap_or(443);
            client_builder = client_builder.resolve(host, std::net::SocketAddr::new(ip, port));
        }
    }

    let client = client_builder
        .build()
        .map_err(|e| CredError::InvalidInput(format!("client build failed: {}", e)))?;
    let mut builder = client.request(method, &req.url);

    // SECURITY: forward only safe caller headers. Hop-by-hop and framing
    // headers are dropped to prevent request smuggling/desync and Host override
    // of the pinned connection.
    if let Some(headers) = &req.headers {
        for (name, value) in headers {
            if is_stripped_request_header(name) {
                continue;
            }
            builder = builder.header(name, value);
        }
    }
    builder = builder.header(&header_name, header_value);

    if let Some(body) = &req.body {
        builder = builder.body(body.clone());
    }

    // SECURITY: cap response body to 10 MiB to prevent upstream from OOM-ing credd.
    const MAX_PROXY_RESPONSE: usize = 10 * 1024 * 1024;

    let response = builder
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| CredError::InvalidInput(format!("proxy request failed: {}", e)))?;

    let status = response.status().as_u16();
    let mut headers = std::collections::HashMap::new();
    for (name, value) in response.headers().iter() {
        if is_stripped_response_header(name.as_str()) {
            continue;
        }
        if let Ok(text) = value.to_str() {
            headers.insert(name.to_string(), text.to_string());
        }
    }

    // Check Content-Length hint before reading body.
    if let Some(cl) = response.content_length() {
        if cl as usize > MAX_PROXY_RESPONSE {
            return Err(CredError::InvalidInput(format!(
                "proxy response too large: {} bytes (max {})",
                cl, MAX_PROXY_RESPONSE
            ))
            .into());
        }
    }

    let body = response
        .text()
        .await
        .map_err(|e| CredError::InvalidInput(format!("proxy response read failed: {}", e)))?;

    if body.len() > MAX_PROXY_RESPONSE {
        return Err(CredError::InvalidInput(format!(
            "proxy response body too large: {} bytes (max {})",
            body.len(),
            MAX_PROXY_RESPONSE
        ))
        .into());
    }

    log_audit(
        &state.db,
        auth.user_id(),
        auth.agent_name(),
        AuditAction::Proxy,
        &req.secret_category,
        &req.secret_name,
        Some(AccessTier::Proxy),
        true,
    )
    .await?;

    Ok(Json(ProxyResponse {
        status,
        headers,
        body,
    }))
}

/// Raw secret access endpoint (returns full secret data).
#[tracing::instrument(skip_all, fields(handler = "credd.resolve.raw"))]
pub async fn raw_handler(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Json(req): Json<RawRequest>,
) -> Result<Json<Value>, AppError> {
    if !auth.can_access_raw() {
        log_audit(
            &state.db,
            auth.user_id(),
            auth.agent_name(),
            AuditAction::Get,
            &req.category,
            &req.name,
            Some(AccessTier::Raw),
            false,
        )
        .await?;
        return Err(CredError::PermissionDenied("raw access not permitted".into()).into());
    }

    if !auth.can_access_category(&req.category) {
        log_audit(
            &state.db,
            auth.user_id(),
            auth.agent_name(),
            AuditAction::Get,
            &req.category,
            &req.name,
            Some(AccessTier::Raw),
            false,
        )
        .await?;
        return Err(CredError::PermissionDenied(format!(
            "no access to category: {}",
            req.category
        ))
        .into());
    }

    let (_row, data) =
        super::get_secret_with_fallback(&state, auth.user_id(), &req.category, &req.name).await?;

    log_audit(
        &state.db,
        auth.user_id(),
        auth.agent_name(),
        AuditAction::Get,
        &req.category,
        &req.name,
        Some(AccessTier::Raw),
        true,
    )
    .await?;

    Ok(Json(json!({
        "category": req.category,
        "name": req.name,
        "value": data,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_placeholders() {
        let text = "key={{secret:aws/api-key}} user={{secret:db/creds.username}}";
        let placeholders = find_placeholders(text);

        assert_eq!(placeholders.len(), 2);
        assert_eq!(placeholders[0].2, "aws");
        assert_eq!(placeholders[0].3, "api-key");
        assert_eq!(placeholders[0].4, None);

        assert_eq!(placeholders[1].2, "db");
        assert_eq!(placeholders[1].3, "creds");
        assert_eq!(placeholders[1].4, Some("username".to_string()));
    }
}
