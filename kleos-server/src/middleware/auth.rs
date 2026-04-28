use axum::body::{to_bytes, Body};
use axum::extract::State;
use axum::http::{Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use kleos_lib::auth::{validate_key, ApiKey, AuthContext, IdentityCtx, Scope};
use kleos_lib::auth_piv::{self, AuthTier, CanonicalEnvelope, SignatureAlgo};
use rusqlite::{params, OptionalExtension};
use std::sync::OnceLock;
use tracing::Instrument;

use crate::middleware::client_ip::client_ip;
use crate::state::AppState;

const OPEN_PATHS: &[&str] = &[
    "/health",
    "/live",
    "/ready",
    "/bootstrap",
    "/.well-known/agent-card.json",
    "/.well-known/agent-commerce.json",
    "/llms.txt",
];

const MAX_AUTH_BODY_BUFFER: usize = 2 * 1024 * 1024;

fn requires_write_scope(method: &Method) -> bool {
    matches!(
        method,
        &Method::POST | &Method::PUT | &Method::PATCH | &Method::DELETE
    )
}

fn forbid(msg: &str) -> Response {
    let body = serde_json::json!({ "error": msg });
    axum::response::Response::builder()
        .status(StatusCode::FORBIDDEN)
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap_or_else(|_| {
            axum::response::Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::empty())
                .expect("static 500 response body")
        })
}

fn unauthorized(msg: &str) -> Response {
    let body = serde_json::json!({ "error": msg });
    axum::response::Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap_or_else(|_| {
            axum::response::Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::empty())
                .expect("static 500 response body")
        })
}

fn open_access_context() -> AuthContext {
    AuthContext {
        key: ApiKey {
            id: 0,
            user_id: 1,
            key_prefix: "open".into(),
            name: "open-access".into(),
            scopes: vec![Scope::Read],
            rate_limit: 1000,
            is_active: true,
            agent_id: None,
            last_used_at: None,
            expires_at: None,
            created_at: String::new(),
            hash_version: 1,
        },
        user_id: 1,
        identity: None,
    }
}

fn synthetic_key_for_identity(user_id: i64) -> ApiKey {
    ApiKey {
        id: 0,
        user_id,
        key_prefix: "sig".into(),
        name: "identity-signed".into(),
        scopes: vec![Scope::Read, Scope::Write, Scope::Admin],
        rate_limit: 1000,
        is_active: true,
        agent_id: None,
        last_used_at: None,
        expires_at: None,
        created_at: String::new(),
        hash_version: 0,
    }
}

fn open_access_allowed() -> bool {
    if std::env::var("ENGRAM_OPEN_ACCESS").as_deref() != Ok("1") {
        return false;
    }
    if cfg!(debug_assertions) {
        return true;
    }
    matches!(
        std::env::var("ENGRAM_ALLOW_OPEN_ACCESS_IN_RELEASE").as_deref(),
        Ok("yes-i-am-sure")
    )
}

static REQUIRE_SIG_USERS: OnceLock<Vec<i64>> = OnceLock::new();

fn signature_required_for_user(user_id: i64) -> bool {
    let users = REQUIRE_SIG_USERS.get_or_init(|| {
        std::env::var("KLEOS_REQUIRE_SIGNATURE_FOR_USER")
            .ok()
            .map(|v| {
                v.split(',')
                    .filter_map(|s| s.trim().parse::<i64>().ok())
                    .collect()
            })
            .unwrap_or_default()
    });
    users.contains(&user_id)
}

fn header_str<'a>(req: &'a Request<Body>, name: &str) -> Option<&'a str> {
    req.headers().get(name).and_then(|v| v.to_str().ok())
}

struct SignedHeaders {
    sig_hex: String,
    algo: SignatureAlgo,
    identity_hash: String,
    ts_ms: u64,
    nonce: String,
    key_fp: String,
    host_label: Option<String>,
    agent_label: Option<String>,
    model_label: Option<String>,
}

fn parse_signed_headers(
    req: &Request<Body>,
) -> Option<std::result::Result<SignedHeaders, &'static str>> {
    let sig_hex = header_str(req, "x-kleos-sig")?;

    Some((|| {
        let algo_str =
            header_str(req, "x-kleos-algo").ok_or("missing X-Kleos-Algo header")?;
        let algo = SignatureAlgo::from_header(algo_str)
            .map_err(|_| "unsupported X-Kleos-Algo value")?;
        let identity_hash = header_str(req, "x-kleos-identity")
            .ok_or("missing X-Kleos-Identity header")?;
        let ts_str =
            header_str(req, "x-kleos-ts").ok_or("missing X-Kleos-Ts header")?;
        let ts_ms: u64 = ts_str
            .parse()
            .map_err(|_| "X-Kleos-Ts must be a u64 (unix milliseconds)")?;
        let nonce =
            header_str(req, "x-kleos-nonce").ok_or("missing X-Kleos-Nonce header")?;
        let key_fp = header_str(req, "x-kleos-key-fp")
            .ok_or("missing X-Kleos-Key-Fp header")?;

        Ok(SignedHeaders {
            sig_hex: sig_hex.to_string(),
            algo,
            identity_hash: identity_hash.to_string(),
            ts_ms,
            nonce: nonce.to_string(),
            key_fp: key_fp.to_string(),
            host_label: header_str(req, "x-kleos-host").map(|s| s.to_string()),
            agent_label: header_str(req, "x-kleos-agent").map(|s| s.to_string()),
            model_label: header_str(req, "x-kleos-model").map(|s| s.to_string()),
        })
    })())
}

struct IdentityKeyRow {
    id: i64,
    user_id: i64,
    tier: String,
    algo: String,
    pubkey_pem: String,
}

#[tracing::instrument(skip_all, fields(middleware = "server.auth"))]
pub async fn auth_middleware(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();
    let req_client_ip = client_ip(&request, &state.config.trusted_proxies)
        .unwrap_or_else(|| "unknown".to_string());

    if OPEN_PATHS
        .iter()
        .any(|p| path == *p || path.starts_with(&format!("{}/", p)))
    {
        return next.run(request).await;
    }

    let method = request.method().clone();
    let query_string = request.uri().query().unwrap_or("").to_string();

    // ---------------------------------------------------------------
    // Path 1: X-Kleos-Session (cached session from prior signed auth)
    // ---------------------------------------------------------------
    if let Some(session_token) = header_str(&request, "x-kleos-session") {
        let session_token = session_token.to_string();
        match state.session_manager.verify(&session_token) {
            Ok(identity_id) => {
                match resolve_identity_by_id(&state, identity_id).await {
                    Ok(auth_ctx) => {
                        let user_id = auth_ctx.user_id;
                        let mut request = request;
                        request.extensions_mut().insert(auth_ctx);
                        let span = tracing::info_span!("request",
                            user_id = user_id, method = %method, path = %path, tier = "session");
                        return next.run(request).instrument(span).await;
                    }
                    Err(msg) => {
                        tracing::warn!(
                            client_ip = %req_client_ip, path = %path,
                            "session identity lookup failed: {msg}"
                        );
                        return unauthorized("invalid session");
                    }
                }
            }
            Err(e) => {
                tracing::debug!("session verification failed: {e}");
            }
        }
    }

    // ---------------------------------------------------------------
    // Path 2: X-Kleos-Sig (full envelope signature verification)
    // ---------------------------------------------------------------
    if let Some(parsed) = parse_signed_headers(&request) {
        let headers = match parsed {
            Ok(h) => h,
            Err(msg) => {
                tracing::warn!(client_ip = %req_client_ip, path = %path,
                    "malformed signature headers: {msg}");
                return unauthorized(msg);
            }
        };

        let (parts, body) = request.into_parts();
        let body_bytes = match to_bytes(body, MAX_AUTH_BODY_BUFFER).await {
            Ok(b) => b,
            Err(_) => {
                return unauthorized(
                    "failed to read request body for signature verification",
                )
            }
        };

        // Look up identity_key by fingerprint
        let key_fp = headers.key_fp.clone();
        let ik_row = match state
            .db
            .read(move |conn| {
                conn.query_row(
                    "SELECT id, user_id, tier, algo, pubkey_pem
                     FROM identity_keys
                     WHERE pubkey_fingerprint = ?1 AND is_active = 1",
                    params![key_fp],
                    |row| {
                        Ok(IdentityKeyRow {
                            id: row.get(0)?,
                            user_id: row.get(1)?,
                            tier: row.get(2)?,
                            algo: row.get(3)?,
                            pubkey_pem: row.get(4)?,
                        })
                    },
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => {
                        kleos_lib::EngError::Auth("unknown key fingerprint".into())
                    }
                    other => kleos_lib::EngError::DatabaseMessage(other.to_string()),
                })
            })
            .await
        {
            Ok(row) => row,
            Err(e) => {
                tracing::warn!(client_ip = %req_client_ip, key_fp = %headers.key_fp,
                    path = %path, "identity key lookup failed: {e}");
                return unauthorized("invalid credentials");
            }
        };

        if ik_row.algo != headers.algo.as_str() {
            tracing::warn!(client_ip = %req_client_ip,
                expected = %ik_row.algo, got = %headers.algo.as_str(),
                "algorithm mismatch");
            return unauthorized("algorithm mismatch");
        }

        let envelope = CanonicalEnvelope::new(
            parts.method.as_str(),
            &path,
            &query_string,
            &body_bytes,
            headers.ts_ms,
            &headers.nonce,
            &headers.identity_hash,
        );
        let envelope_bytes = envelope.build();

        if let Err(e) = auth_piv::verify_signature(
            headers.algo,
            &ik_row.pubkey_pem,
            &envelope_bytes,
            &headers.sig_hex,
        ) {
            tracing::warn!(client_ip = %req_client_ip, path = %path,
                "signature verification failed: {e}");
            return unauthorized("signature verification failed");
        }

        if let Err(e) = state.replay_guard.check(
            &headers.identity_hash,
            &headers.nonce,
            headers.ts_ms,
        ) {
            tracing::warn!(client_ip = %req_client_ip, path = %path,
                "replay check failed: {e}");
            return unauthorized("replay detected or timestamp out of range");
        }

        // Look up or auto-create identity
        let identity_hash_for_lookup = headers.identity_hash.clone();
        let ik_id = ik_row.id;
        let identity_result = state
            .db
            .read(move |conn| {
                conn.query_row(
                    "SELECT id, host_label, agent_label, model_label
                     FROM identities WHERE identity_hash = ?1",
                    params![identity_hash_for_lookup],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .optional()
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
            })
            .await;

        let auth_tier = match ik_row.tier.as_str() {
            "piv" => AuthTier::Piv,
            _ => AuthTier::Soft,
        };

        let identity_ctx = match identity_result {
            Ok(Some((id, host, agent, model))) => {
                let hash = headers.identity_hash.clone();
                let _ = state
                    .db
                    .write(move |conn| {
                        conn.execute(
                            "UPDATE identities SET last_seen_at = datetime('now'), \
                             request_count = request_count + 1 WHERE identity_hash = ?1",
                            params![hash],
                        )
                        .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
                        Ok(())
                    })
                    .await;

                IdentityCtx {
                    identity_id: Some(id),
                    identity_key_id: ik_id,
                    hash: headers.identity_hash.clone(),
                    tier: auth_tier,
                    host,
                    agent,
                    model,
                }
            }
            Ok(None) => {
                let host = headers
                    .host_label
                    .as_deref()
                    .unwrap_or("unknown")
                    .to_string();
                let agent = headers
                    .agent_label
                    .as_deref()
                    .unwrap_or("unknown")
                    .to_string();
                let model = headers
                    .model_label
                    .as_deref()
                    .unwrap_or("unknown")
                    .to_string();

                // Verify claimed identity_hash matches HKDF derivation
                if let Some(der) = decode_pem_der(&ik_row.pubkey_pem) {
                    let expected =
                        auth_piv::identity_hash_hex(&der, &host, &agent, &model);
                    if expected != headers.identity_hash {
                        tracing::warn!(client_ip = %req_client_ip,
                            expected = %expected, got = %headers.identity_hash,
                            "identity hash mismatch during auto-registration");
                        return unauthorized("identity hash mismatch");
                    }
                }

                let hash_for_insert = headers.identity_hash.clone();
                let hash_for_select = headers.identity_hash.clone();
                let host_ins = host.clone();
                let agent_ins = agent.clone();
                let model_ins = model.clone();
                let new_id = state
                    .db
                    .write(move |conn| {
                        conn.execute(
                            "INSERT OR IGNORE INTO identities \
                             (identity_key_id, identity_hash, host_label, agent_label, model_label) \
                             VALUES (?1, ?2, ?3, ?4, ?5)",
                            params![ik_id, hash_for_insert, host_ins, agent_ins, model_ins],
                        )
                        .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
                        let id = conn
                            .query_row(
                                "SELECT id FROM identities WHERE identity_hash = ?1",
                                params![hash_for_select],
                                |row| row.get::<_, i64>(0),
                            )
                            .map_err(|e| {
                                kleos_lib::EngError::DatabaseMessage(e.to_string())
                            })?;
                        Ok(id)
                    })
                    .await;

                let identity_id = match new_id {
                    Ok(id) => {
                        tracing::info!(identity_id = id, host = %host,
                            agent = %agent, model = %model,
                            "auto-registered new identity");
                        Some(id)
                    }
                    Err(e) => {
                        tracing::warn!("identity auto-registration failed: {e}");
                        None
                    }
                };

                IdentityCtx {
                    identity_id,
                    identity_key_id: ik_id,
                    hash: headers.identity_hash.clone(),
                    tier: auth_tier,
                    host,
                    agent,
                    model,
                }
            }
            Err(e) => {
                tracing::warn!("identity lookup failed: {e}");
                IdentityCtx {
                    identity_id: None,
                    identity_key_id: ik_id,
                    hash: headers.identity_hash.clone(),
                    tier: auth_tier,
                    host: "unknown".into(),
                    agent: "unknown".into(),
                    model: "unknown".into(),
                }
            }
        };

        let _ = state
            .db
            .write(move |conn| {
                conn.execute(
                    "UPDATE identity_keys SET last_seen_at = datetime('now') WHERE id = ?1",
                    params![ik_id],
                )
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
                Ok(())
            })
            .await;

        let session_token = identity_ctx
            .identity_id
            .map(|id| state.session_manager.mint(id));

        let user_id = ik_row.user_id;
        let auth_ctx = AuthContext {
            key: synthetic_key_for_identity(user_id),
            user_id,
            identity: Some(identity_ctx),
        };

        let mut request = Request::from_parts(parts, Body::from(body_bytes));
        request.extensions_mut().insert(auth_ctx);

        let span = tracing::info_span!("request",
            user_id = user_id, method = %method, path = %path,
            tier = %auth_tier.as_str());
        let mut response = next.run(request).instrument(span).await;

        if let Some(token) = session_token {
            if let Ok(val) = axum::http::HeaderValue::from_str(&token) {
                response
                    .headers_mut()
                    .insert("x-kleos-session-issued", val);
            }
        }

        return response;
    }

    // ---------------------------------------------------------------
    // Path 3: Bearer token (existing flow)
    // ---------------------------------------------------------------
    let token = header_str(&request, "authorization")
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string());

    let mut request = request;
    match token {
        None => {
            if open_access_allowed() {
                if requires_write_scope(&method) {
                    return forbid(
                        "ENGRAM_OPEN_ACCESS is read-only; writes require an API key",
                    );
                }
                tracing::warn!(path = %path,
                    "ENGRAM_OPEN_ACCESS bypassing authentication");
                request.extensions_mut().insert(open_access_context());
                return next.run(request).await;
            }
            unauthorized(
                "Authentication required. Provide Bearer eg_* token or X-Kleos-Sig header.",
            )
        }
        Some(raw_key) => match validate_key(&state.db, &raw_key).await {
            Ok(auth_ctx) => {
                if signature_required_for_user(auth_ctx.user_id) {
                    tracing::warn!(user_id = auth_ctx.user_id,
                        client_ip = %req_client_ip, path = %path,
                        "bearer auth rejected: signature required for this user");
                    return unauthorized("signature required for this user");
                }

                if requires_write_scope(&method)
                    && !auth_ctx.has_scope(&Scope::Write)
                {
                    return forbid("write scope required for this method");
                }
                if !requires_write_scope(&method)
                    && !auth_ctx.has_scope(&Scope::Read)
                {
                    return forbid("read scope required for this method");
                }
                let user_id = auth_ctx.user_id;
                request.extensions_mut().insert(auth_ctx);
                let span = tracing::info_span!("request",
                    user_id = user_id, method = %method, path = %path,
                    tier = "bearer");
                next.run(request).instrument(span).await
            }
            Err(e) => {
                tracing::warn!(error = %e, client_ip = %req_client_ip,
                    path = %path, method = %method, "authentication failed");
                unauthorized("invalid credentials")
            }
        },
    }
}

fn decode_pem_der(pem: &str) -> Option<Vec<u8>> {
    let begin = "-----BEGIN PUBLIC KEY-----";
    let end = "-----END PUBLIC KEY-----";
    let b64: String = pem
        .lines()
        .skip_while(|l| !l.starts_with(begin))
        .skip(1)
        .take_while(|l| !l.starts_with(end))
        .collect::<Vec<_>>()
        .join("");
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(&b64).ok()
}

async fn resolve_identity_by_id(
    state: &AppState,
    identity_id: i64,
) -> std::result::Result<AuthContext, String> {
    let row = state
        .db
        .read(move |conn| {
            conn.query_row(
                "SELECT i.identity_key_id, i.identity_hash, i.host_label, i.agent_label,
                        i.model_label, ik.user_id, ik.tier
                 FROM identities i
                 JOIN identity_keys ik ON ik.id = i.identity_key_id
                 WHERE i.id = ?1 AND i.is_active = 1 AND ik.is_active = 1",
                params![identity_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await
        .map_err(|e| e.to_string())?;

    let (ik_id, hash, host, agent, model, user_id, tier_str) = row;
    let tier = match tier_str.as_str() {
        "piv" => AuthTier::Piv,
        _ => AuthTier::Soft,
    };

    Ok(AuthContext {
        key: synthetic_key_for_identity(user_id),
        user_id,
        identity: Some(IdentityCtx {
            identity_id: Some(identity_id),
            identity_key_id: ik_id,
            hash,
            tier,
            host,
            agent,
            model,
        }),
    })
}
