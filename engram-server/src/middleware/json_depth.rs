use axum::{
    body::{to_bytes, Body},
    extract::Request,
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

/// SECURITY/DoS: serde_json does not enforce a recursion cap during
/// deserialization. A crafted payload with thousands of nested array or
/// object openings can blow the stack long before hitting our 2 MiB body
/// limit, taking the whole server down. This middleware counts opening
/// brackets in the raw body before the JSON extractor sees it and rejects
/// anything deeper than `MAX_JSON_DEPTH`.
const MAX_JSON_DEPTH: u32 = 64;

/// Upper bound on the body we will buffer for depth validation. Matches the
/// server-wide DefaultBodyLimit so we never grow beyond what the app would
/// have accepted anyway.
const MAX_BUFFER_BYTES: usize = 2 * 1024 * 1024;

pub async fn json_depth_middleware(request: Request, next: Next) -> Response {
    let is_json = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_ascii_lowercase().starts_with("application/json"))
        .unwrap_or(false);

    if !is_json {
        return next.run(request).await;
    }

    let (parts, body) = request.into_parts();
    let bytes = match to_bytes(body, MAX_BUFFER_BYTES).await {
        Ok(b) => b,
        Err(_) => {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                "request body exceeds maximum size",
            )
                .into_response();
        }
    };

    if !within_depth_limit(&bytes, MAX_JSON_DEPTH) {
        return (
            StatusCode::BAD_REQUEST,
            "json payload nesting exceeds maximum depth",
        )
            .into_response();
    }

    let request = Request::from_parts(parts, Body::from(bytes));
    next.run(request).await
}

/// Scan the raw bytes and return false if any `{`/`[` nesting level exceeds
/// `max_depth`. Respects JSON string escaping so brackets inside a string
/// literal are ignored.
fn within_depth_limit(bytes: &[u8], max_depth: u32) -> bool {
    let mut depth: u32 = 0;
    let mut in_string = false;
    let mut escape = false;
    for &b in bytes {
        if in_string {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' | b'[' => {
                depth += 1;
                if depth > max_depth {
                    return false;
                }
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_shallow_json() {
        let body = br#"{"a":1,"b":[1,2,{"c":3}]}"#;
        assert!(within_depth_limit(body, 64));
    }

    #[test]
    fn rejects_deeply_nested_arrays() {
        let body = "[".repeat(100);
        assert!(!within_depth_limit(body.as_bytes(), 64));
    }

    #[test]
    fn ignores_brackets_inside_strings() {
        let body = br#"{"x":"[[[[[[[[[["}"#;
        assert!(within_depth_limit(body, 4));
    }

    #[test]
    fn handles_escaped_quotes_in_strings() {
        let body = br#"{"x":"\"[[[[\""}"#;
        assert!(within_depth_limit(body, 4));
    }
}
