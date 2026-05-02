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
use kleos_cred::storage::get_secret;
use kleos_cred::CredError;

use crate::auth::Auth;
use crate::handlers::AppError;
use crate::state::AppState;

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
    let placeholders = find_placeholders(&body.text);
    let mut result = body.text.clone();
    let mut offset: isize = 0;
    let mut substitutions = 0;

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
            continue;
        }

        let secret_result = get_secret(
            &state.db,
            auth.user_id(),
            &category,
            &name,
            state.master_key.as_ref(),
        )
        .await;

        match secret_result {
            Ok((_row, data)) => {
                let value = match field {
                    Some(ref f) => data.get_field(f).unwrap_or_default(),
                    None => data.primary_value(),
                };

                let adj_start = (start as isize + offset) as usize;
                let adj_end = (end as isize + offset) as usize;
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
    kleos_lib::webhooks::resolve_and_validate_url(&req.url)
        .await
        .map_err(|e| CredError::InvalidInput(format!("proxy target URL rejected: {}", e)))?;

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

    let (_row, data) = get_secret(
        &state.db,
        auth.user_id(),
        &req.secret_category,
        &req.secret_name,
        state.master_key.as_ref(),
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
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| CredError::InvalidInput(format!("client build failed: {}", e)))?;
    let mut builder = client.request(method, &req.url);

    if let Some(headers) = &req.headers {
        for (name, value) in headers {
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

    let (_row, data) = get_secret(
        &state.db,
        auth.user_id(),
        &req.category,
        &req.name,
        state.master_key.as_ref(),
    )
    .await?;

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
