use axum::body::{to_bytes, Body};
use axum::extract::State;
use axum::http::{Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use dashmap::DashMap;
use kleos_lib::auth::{validate_key, ApiKey, AuthContext, IdentityCtx, Scope};
use kleos_lib::auth_piv::{self, AuthTier, CanonicalEnvelope, SignatureAlgo};
use kleos_lib::mcp_token;
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
    "/policy/mandatory",
];

const MAX_AUTH_BODY_BUFFER: usize = 2 * 1024 * 1024;

fn requires_write_scope(method: &Method) -> bool {
    matches!(
        method,
        &Method::POST | &Method::PUT | &Method::PATCH | &Method::DELETE
    )
}

fn is_read_only_post(path: &str) -> bool {
    matches!(
        path,
        "/search"
            | "/memories/search"
            | "/search/explain"
            | "/search/faceted"
            | "/recall"
            | "/recall-due"
            | "/tags/search"
            | "/graph/search"
            | "/skills/search"
            | "/messages/search"
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
        act_as: None,
        identity: None,
    }
}

fn synthetic_key_for_identity(user_id: i64) -> ApiKey {
    synthetic_key_for_identity_with_scopes(user_id, None)
}

/// Build the in-memory ApiKey for a signed-envelope request. `scopes_csv` is the
/// `identity_keys.scopes` column -- a comma-separated list using the same scope
/// grammar as `api_keys.scopes` (e.g. "read,write"). When present it is
/// authoritative and parsed with the canonical parser: a value that yields no
/// known scopes is a deny (empty scope set), NEVER an escalation. Only a MISSING
/// value (None) retains the historical admin grant -- that covers pre-v53 rows
/// and the user-1 bootstrap caller, not freshly enrolled keys.
fn synthetic_key_for_identity_with_scopes(user_id: i64, scopes_csv: Option<&str>) -> ApiKey {
    let scopes = match scopes_csv {
        // No stored scopes column value: legacy/bootstrap rows keep admin. New
        // enrollments always populate scopes, so they never reach this arm.
        None => vec![Scope::Read, Scope::Write, Scope::Admin],
        // Stored scopes are authoritative. Empty or all-unknown parses to an
        // empty scope set (deny) -- correct least privilege, not an escalation.
        Some(raw) => kleos_lib::auth::parse_scopes(raw),
    };

    ApiKey {
        id: 0,
        user_id,
        key_prefix: "sig".into(),
        name: "identity-signed".into(),
        scopes,
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
    if kleos_lib::kleos_env("OPEN_ACCESS").as_deref() != Ok("1") {
        return false;
    }
    // Require explicit confirmation in BOTH debug and release builds. The
    // dev-loop convenience of an unconditional debug bypass is not worth the
    // foot-gun if a debug binary ever reaches a non-dev environment (H5).
    matches!(
        kleos_lib::kleos_env("ALLOW_OPEN_ACCESS_IN_RELEASE").as_deref(),
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

/// Debounce map for mcp_tokens.last_used_at updates.
/// Key: jti, Value: last write instant. Writes only if > 60s since last.
static MCP_TOKEN_LAST_USED: std::sync::LazyLock<DashMap<String, std::time::Instant>> =
    std::sync::LazyLock::new(DashMap::new);

/// Validate an MCP direct-auth token (kleos. prefix bearer).
///
/// Verification flow: decode -> expiry check -> identity key lookup ->
/// Ed25519 sig verify -> scope cap -> revocation check (DB) -> build AuthContext.
/// Invalid tokens never touch the revocation table (sig verify gates DB access).
async fn validate_mcp_token(
    state: &AppState,
    raw_token: &str,
    method: &Method,
    path: &str,
) -> Result<AuthContext, String> {
    // Step 1: Decode token (format + version check).
    let decoded = mcp_token::decode(raw_token).map_err(|e| e.to_string())?;
    let payload = &decoded.payload;

    // Step 2: Reject wildcard scopes.
    let token_scopes =
        mcp_token::parse_scopes_strict(&payload.scopes).map_err(|e| e.to_string())?;

    // Step 3: Expiry check (no DB hit).
    mcp_token::check_expiry(payload).map_err(|e| e.to_string())?;

    // Step 4: Look up identity key by kid (fingerprint).
    let kid = payload.kid.clone();
    let key_row = state
        .db
        .read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, user_id, pubkey_pem, scopes
                     FROM identity_keys
                     WHERE pubkey_fingerprint = ?1 AND is_active = 1",
            )?;
            let row = stmt
                .query_row(rusqlite::params![kid], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                })
                .optional()?;
            Ok(row)
        })
        .await
        .map_err(|e| format!("database error: {}", e))?;

    let (_ik_id, ik_user_id, pubkey_pem, ik_scopes_csv) =
        key_row.ok_or_else(|| "identity key not found or revoked".to_string())?;

    // Step 5: Ed25519 signature verification over raw payload bytes.
    let vk = kleos_lib::auth_piv::pem_to_ed25519_verifying_key(&pubkey_pem)
        .map_err(|e| format!("invalid pubkey: {}", e))?;
    mcp_token::verify_signature(&vk, &decoded).map_err(|_| "invalid signature".to_string())?;

    // --- Signature valid past this point ---

    // Step 6: The verified identity key's owner is the authoritative user. The
    // token's `payload.uid` is informational only and deliberately NOT trusted,
    // so a keyless minter (SO_PEERCRED broker, kleos-cli) need not know its
    // server-side user id. `ik_user_id` is used everywhere below as the principal.

    // Step 7: Scope cap -- token scopes must be subset of identity key scopes.
    let ik_scopes = match ik_scopes_csv.as_deref() {
        Some(csv) => kleos_lib::auth::parse_scopes(csv),
        None => vec![Scope::Read, Scope::Write, Scope::Admin],
    };
    mcp_token::scopes_within_cap(&token_scopes, &ik_scopes).map_err(|e| e.to_string())?;

    // Step 8: Revocation check (DB). Fail closed on error. Scoped to the
    // verified key owner (ik_user_id), not the untrusted payload.uid.
    let jti = payload.jti.clone();
    let uid = ik_user_id;
    let revocation_row = state
        .db
        .read(move |conn| {
            Ok(conn
                .query_row(
                    "SELECT id, is_active, name FROM mcp_tokens
                 WHERE jti = ?1 AND user_id = ?2",
                    rusqlite::params![jti, uid],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, bool>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()?)
        })
        .await
        .map_err(|e| format!("revocation check failed (fail closed): {}", e))?;

    let (_token_id, is_active, token_name) =
        revocation_row.ok_or_else(|| "token not registered".to_string())?;

    if !is_active {
        return Err("token revoked".to_string());
    }

    // Step 9: Scope enforcement for this request.
    if requires_write_scope(method)
        && !is_read_only_post(path)
        && !token_scopes.contains(&Scope::Write)
        && !token_scopes.contains(&Scope::Admin)
    {
        return Err("write scope required for this method".to_string());
    }
    if !requires_write_scope(method)
        && !token_scopes.contains(&Scope::Read)
        && !token_scopes.contains(&Scope::Admin)
    {
        return Err("read scope required for this method".to_string());
    }

    // Step 10: Debounced last_used_at update.
    let jti_for_update = payload.jti.clone();
    let should_write = {
        let now = std::time::Instant::now();
        let entry = MCP_TOKEN_LAST_USED.entry(jti_for_update.clone());
        match entry {
            dashmap::mapref::entry::Entry::Occupied(mut e) => {
                if now.duration_since(*e.get()).as_secs() >= 60 {
                    e.insert(now);
                    true
                } else {
                    false
                }
            }
            dashmap::mapref::entry::Entry::Vacant(e) => {
                e.insert(now);
                true
            }
        }
    };
    if should_write {
        let jti_w = jti_for_update;
        let _ = state
            .db
            .write(move |conn| {
                conn.execute(
                    "UPDATE mcp_tokens SET last_used_at = datetime('now') WHERE jti = ?1",
                    rusqlite::params![jti_w],
                )?;
                Ok(())
            })
            .await;
    }

    // Step 11: Build AuthContext (identity = None, same as API keys).
    let key = ApiKey {
        id: 0,
        user_id: ik_user_id,
        key_prefix: "mcp".into(),
        name: token_name,
        scopes: token_scopes,
        rate_limit: 1000,
        is_active: true,
        agent_id: None,
        last_used_at: None,
        expires_at: None,
        created_at: String::new(),
        hash_version: 0,
    };

    Ok(AuthContext {
        key,
        user_id: ik_user_id,
        act_as: None,
        identity: None,
    })
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
        let algo_str = header_str(req, "x-kleos-algo").ok_or("missing X-Kleos-Algo header")?;
        let algo =
            SignatureAlgo::from_header(algo_str).map_err(|_| "unsupported X-Kleos-Algo value")?;
        let identity_hash =
            header_str(req, "x-kleos-identity").ok_or("missing X-Kleos-Identity header")?;
        let ts_str = header_str(req, "x-kleos-ts").ok_or("missing X-Kleos-Ts header")?;
        let ts_ms: u64 = ts_str
            .parse()
            .map_err(|_| "X-Kleos-Ts must be a u64 (unix milliseconds)")?;
        let nonce = header_str(req, "x-kleos-nonce").ok_or("missing X-Kleos-Nonce header")?;
        let key_fp = header_str(req, "x-kleos-key-fp").ok_or("missing X-Kleos-Key-Fp header")?;

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
    scopes_json: Option<String>,
}

#[tracing::instrument(skip_all, fields(middleware = "server.auth"))]
pub async fn auth_middleware(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();
    let req_client_ip =
        client_ip(&request, &state.config.trusted_proxies).unwrap_or_else(|| "unknown".to_string());

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
            Ok(identity_id) => match resolve_identity_by_id(&state, identity_id).await {
                Ok(auth_ctx) => {
                    // M5: enforce signature_required uniformly. A session
                    // token is a delegated credential -- it bypasses the
                    // per-request signature check, so users opted into PIV
                    // (KLEOS_REQUIRE_SIGNATURE_FOR_USER) must not be able to
                    // re-use a session token in lieu of signing.
                    if signature_required_for_user(auth_ctx.user_id) {
                        tracing::warn!(user_id = auth_ctx.user_id,
                            client_ip = %req_client_ip, path = %path,
                            "session auth rejected: signature required for this user");
                        return unauthorized("signature required for this user");
                    }
                    if requires_write_scope(&method)
                        && !is_read_only_post(&path)
                        && !auth_ctx.has_scope(&Scope::Write)
                    {
                        return forbid("write scope required for this method");
                    }
                    if !requires_write_scope(&method) && !auth_ctx.has_scope(&Scope::Read) {
                        return forbid("read scope required for this method");
                    }

                    // Sliding window: roll the session forward so an active
                    // client never hits the TTL boundary. If refresh fails
                    // (hard cap reached) the request still completes -- the
                    // client will be forced through fresh PIV signing on the
                    // next call when verify() finally returns expired.
                    let refreshed_token = match state.session_manager.refresh(&session_token) {
                        Ok(t) => Some(t),
                        Err(e) => {
                            tracing::debug!(
                                client_ip = %req_client_ip, path = %path,
                                "session refresh declined: {e}"
                            );
                            None
                        }
                    };

                    let user_id = auth_ctx.user_id;
                    let mut request = request;
                    request.extensions_mut().insert(auth_ctx);
                    let span = tracing::info_span!("request",
                            user_id = user_id, method = %method, path = %path, tier = "session");
                    let mut response = next.run(request).instrument(span).await;

                    if let Some(token) = refreshed_token {
                        if let Ok(val) = axum::http::HeaderValue::from_str(&token) {
                            response.headers_mut().insert("x-kleos-session-issued", val);
                        }
                    }

                    return response;
                }
                Err(msg) => {
                    tracing::warn!(
                        client_ip = %req_client_ip, path = %path,
                        "session identity lookup failed: {msg}"
                    );
                    return unauthorized("invalid session");
                }
            },
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
                return unauthorized("failed to read request body for signature verification")
            }
        };

        // Look up identity_key by fingerprint
        let key_fp = headers.key_fp.clone();
        let ik_row = match state
            .db
            .read(move |conn| {
                conn.query_row(
                    "SELECT id, user_id, tier, algo, pubkey_pem, scopes
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
                            scopes_json: row.get(5)?,
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

        if let Err(e) =
            state
                .replay_guard
                .check(&headers.identity_hash, &headers.nonce, headers.ts_ms)
        {
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
                Ok(conn
                    .query_row(
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
                    .optional()?)
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
                        )?;
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
                    let expected = auth_piv::identity_hash_hex(&der, &host, &agent, &model);
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
                        ?;
                        let id = conn
                            .query_row(
                                "SELECT id FROM identities WHERE identity_hash = ?1",
                                params![hash_for_select],
                                |row| row.get::<_, i64>(0),
                            )?;
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
                )?;
                Ok(())
            })
            .await;

        let session_token = identity_ctx
            .identity_id
            .map(|id| state.session_manager.mint(id));

        let user_id = ik_row.user_id;
        let auth_ctx = AuthContext {
            key: synthetic_key_for_identity_with_scopes(user_id, ik_row.scopes_json.as_deref()),
            user_id,
            act_as: None,
            identity: Some(identity_ctx),
        };

        if requires_write_scope(&method)
            && !is_read_only_post(&path)
            && !auth_ctx.has_scope(&Scope::Write)
        {
            return forbid("write scope required for this method");
        }
        if !requires_write_scope(&method) && !auth_ctx.has_scope(&Scope::Read) {
            return forbid("read scope required for this method");
        }

        let mut request = Request::from_parts(parts, Body::from(body_bytes));
        request.extensions_mut().insert(auth_ctx);

        let span = tracing::info_span!("request",
            user_id = user_id, method = %method, path = %path,
            tier = %auth_tier.as_str());
        let mut response = next.run(request).instrument(span).await;

        if let Some(token) = session_token {
            if let Ok(val) = axum::http::HeaderValue::from_str(&token) {
                response.headers_mut().insert("x-kleos-session-issued", val);
            }
        }

        return response;
    }

    // ---------------------------------------------------------------
    // Path 3: Bearer token
    // ---------------------------------------------------------------
    let token = header_str(&request, "authorization")
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string());

    let mut request = request;
    if let Some(raw_key) = token {
        // --- Path 3a: MCP direct-auth token (kleos. prefix) ---
        if raw_key.starts_with(mcp_token::TOKEN_PREFIX) {
            match validate_mcp_token(&state, &raw_key, &method, &path).await {
                Ok(auth_ctx) => {
                    let user_id = auth_ctx.user_id;
                    request.extensions_mut().insert(auth_ctx);
                    let span = tracing::info_span!("request",
                        user_id = user_id, method = %method, path = %path,
                        tier = "mcp-token");
                    return next.run(request).instrument(span).await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, client_ip = %req_client_ip,
                        path = %path, method = %method, "mcp token auth failed");
                    return unauthorized(&e);
                }
            }
        }

        // --- Path 3b: API key bearer (existing) ---
        match validate_key(&state.db, &raw_key).await {
            Ok(auth_ctx) => {
                if signature_required_for_user(auth_ctx.user_id) {
                    tracing::warn!(user_id = auth_ctx.user_id,
                        client_ip = %req_client_ip, path = %path,
                        "bearer auth rejected: signature required for this user");
                    return unauthorized("signature required for this user");
                }

                if requires_write_scope(&method)
                    && !is_read_only_post(&path)
                    && !auth_ctx.has_scope(&Scope::Write)
                {
                    return forbid("write scope required for this method");
                }
                if !requires_write_scope(&method) && !auth_ctx.has_scope(&Scope::Read) {
                    return forbid("read scope required for this method");
                }
                let user_id = auth_ctx.user_id;
                request.extensions_mut().insert(auth_ctx);
                let span = tracing::info_span!("request",
                    user_id = user_id, method = %method, path = %path,
                    tier = "bearer");
                return next.run(request).instrument(span).await;
            }
            Err(e) => {
                tracing::warn!(error = %e, client_ip = %req_client_ip,
                    path = %path, method = %method, "bearer auth failed");
            }
        }
    }

    // ---------------------------------------------------------------
    // Path 3c: GUI session cookie (read-only).
    //
    // An EventSource (SSE) cannot send an Authorization header, so the
    // realtime stream (/axon/stream) authenticates via the HMAC-signed,
    // SameSite=Strict GUI cookie that EventSource does send same-origin.
    // `get_gui_session` re-checks the underlying key is still active, so a
    // revoked key's stale cookie stops working. Restricted to safe
    // (non-mutating) methods so a cookie alone can never write: CSRF is
    // already blocked by SameSite=Strict, and this is defense in depth.
    // ---------------------------------------------------------------
    if !requires_write_scope(&method) {
        if let Some(session) = crate::routes::gui::get_gui_session(&state, request.headers()).await
        {
            if session.has_scope(&Scope::Read) {
                let scopes_csv = kleos_lib::auth::scopes_to_string(&session.scopes);
                let auth_ctx = AuthContext {
                    key: synthetic_key_for_identity_with_scopes(session.user_id, Some(&scopes_csv)),
                    user_id: session.user_id,
                    act_as: None,
                    identity: None,
                };
                let user_id = auth_ctx.user_id;
                request.extensions_mut().insert(auth_ctx);
                let span = tracing::info_span!("request",
                    user_id = user_id, method = %method, path = %path,
                    tier = "gui-cookie");
                return next.run(request).instrument(span).await;
            }
        }
    }

    // ---------------------------------------------------------------
    // Path 4: Enrollment proof-of-possession (PIV bootstrap)
    //
    // Only for POST /identity-keys/enroll. The request body carries a
    // self-signature proving the client holds the private key. The
    // middleware verifies that proof and determines the user:
    //   - No identity_keys in DB yet: bootstrap -- assign to owner (user_id=1)
    //   - Keys already exist: reject (must use an enrolled key via Path 2)
    // ---------------------------------------------------------------
    if path == "/identity-keys/enroll" && method == Method::POST {
        let (parts, body) = request.into_parts();
        let body_bytes = match to_bytes(body, MAX_AUTH_BODY_BUFFER).await {
            Ok(b) => b,
            Err(_) => return unauthorized("failed to read enrollment request body"),
        };

        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct EnrollProof {
            algo: String,
            tier: String,
            pubkey_pem: String,
            host_label: String,
            sig_hex: String,
        }

        let proof: EnrollProof = match serde_json::from_slice(&body_bytes) {
            Ok(p) => p,
            Err(_) => return unauthorized("invalid enrollment body"),
        };

        let algo = match auth_piv::SignatureAlgo::from_header(&proof.algo) {
            Ok(a) => a,
            Err(_) => return unauthorized("unsupported algorithm in enrollment"),
        };

        let proof_msg = format!(
            "KLEOS-ENROLL:{}:{}:{}:{}",
            proof.algo, proof.tier, proof.host_label, proof.pubkey_pem,
        );
        if let Err(e) = auth_piv::verify_signature(
            algo,
            &proof.pubkey_pem,
            proof_msg.as_bytes(),
            &proof.sig_hex,
        ) {
            tracing::warn!(client_ip = %req_client_ip,
                "enrollment proof-of-possession failed: {e}");
            return unauthorized("enrollment proof-of-possession verification failed");
        }

        // L2 (benign TOCTOU): this count and the enrollment insert below are
        // not one transaction, so two concurrent first-time bootstraps could
        // both observe count==0. That is harmless: both are assigned the same
        // owner user_id=1, so the worst case is two owner keys enrolled in the
        // narrow first-touch window rather than any privilege escalation.
        let key_count: i64 = match state
            .db
            .read(|conn| {
                Ok(conn.query_row("SELECT COUNT(*) FROM identity_keys", [], |row| row.get(0))?)
            })
            .await
        {
            Ok(c) => c,
            Err(_) => return unauthorized("internal error checking enrollment state"),
        };

        if key_count > 0 {
            tracing::warn!(client_ip = %req_client_ip,
                "enrollment rejected: keys exist, must authenticate with existing identity");
            return unauthorized(
                "identity keys already enrolled; authenticate with an existing \
                 key (X-Kleos-Sig) to enroll additional keys",
            );
        }

        // Optional bootstrap secret: if KLEOS_BOOTSTRAP_SECRET is set, the
        // caller must provide the matching value in X-Bootstrap-Secret.
        // When the env var is unset, bootstrap proceeds unauthenticated
        // (dev-friendly default). When set, the comparison is constant-time
        // to prevent timing-based secret enumeration.
        if let Ok(expected_secret) = std::env::var("KLEOS_BOOTSTRAP_SECRET") {
            use sha2::{Digest, Sha256};
            use subtle::ConstantTimeEq;
            let provided = parts
                .headers
                .get("x-bootstrap-secret")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            // Hash both sides to a fixed 32 bytes before the constant-time
            // compare. ct_eq short-circuits on length mismatch, which would
            // otherwise leak the expected secret's length via timing; hashing
            // first makes both operands always 32 bytes long.
            let expected_digest = Sha256::digest(expected_secret.as_bytes());
            let provided_digest = Sha256::digest(provided.as_bytes());
            if expected_digest
                .as_slice()
                .ct_eq(provided_digest.as_slice())
                .unwrap_u8()
                != 1
            {
                tracing::warn!(client_ip = %req_client_ip,
                    "bootstrap enrollment rejected: invalid bootstrap secret");
                return unauthorized("invalid bootstrap secret");
            }
        }

        tracing::info!(client_ip = %req_client_ip,
            "bootstrap enrollment: first identity key, assigning to owner (user_id=1)");

        let auth_ctx = AuthContext {
            key: synthetic_key_for_identity(1),
            user_id: 1,
            act_as: None,
            identity: None,
        };

        let mut request = Request::from_parts(parts, Body::from(body_bytes));
        request.extensions_mut().insert(auth_ctx);
        let span = tracing::info_span!("request",
            user_id = 1, method = %method, path = %path, tier = "enrollment-bootstrap");
        return next.run(request).instrument(span).await;
    }

    // ---------------------------------------------------------------
    // No auth method succeeded
    // ---------------------------------------------------------------
    if open_access_allowed() {
        if requires_write_scope(&method) && !is_read_only_post(&path) {
            return forbid("ENGRAM_OPEN_ACCESS is read-only; writes require an API key");
        }
        tracing::warn!(path = %path, "ENGRAM_OPEN_ACCESS bypassing authentication");
        request.extensions_mut().insert(open_access_context());
        return next.run(request).await;
    }

    unauthorized("Authentication required. Provide X-Kleos-Sig header or Bearer token.")
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
            Ok(conn.query_row(
                "SELECT i.identity_key_id, i.identity_hash, i.host_label, i.agent_label,
                        i.model_label, ik.user_id, ik.tier, ik.scopes
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
                        row.get::<_, Option<String>>(7)?,
                    ))
                },
            )?)
        })
        .await
        .map_err(|e| e.to_string())?;

    let (ik_id, hash, host, agent, model, user_id, tier_str, scopes_json) = row;
    let tier = match tier_str.as_str() {
        "piv" => AuthTier::Piv,
        _ => AuthTier::Soft,
    };

    Ok(AuthContext {
        key: synthetic_key_for_identity_with_scopes(user_id, scopes_json.as_deref()),
        user_id,
        act_as: None,
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

#[cfg(test)]
mod identity_scope_tests {
    use super::*;

    #[test]
    fn stored_csv_scopes_are_used_verbatim() {
        let key = synthetic_key_for_identity_with_scopes(7, Some("read,write"));
        assert!(key.scopes.contains(&Scope::Read));
        assert!(key.scopes.contains(&Scope::Write));
        assert!(
            !key.scopes.contains(&Scope::Admin),
            "default enrolled scopes must NOT be admin"
        );
    }

    #[test]
    fn empty_scopes_deny_not_admin() {
        let key = synthetic_key_for_identity_with_scopes(7, Some(""));
        assert!(
            !key.scopes.contains(&Scope::Admin),
            "explicitly empty scopes must not escalate to admin"
        );
        assert!(key.scopes.is_empty(), "empty scopes string means deny");
    }

    #[test]
    fn unknown_scopes_deny_not_admin() {
        let key = synthetic_key_for_identity_with_scopes(7, Some("bogus,nonsense"));
        assert!(
            !key.scopes.contains(&Scope::Admin),
            "all-unknown scopes must not escalate to admin"
        );
        assert!(key.scopes.is_empty());
    }

    #[test]
    fn admin_is_granted_only_when_explicitly_stored() {
        let key = synthetic_key_for_identity_with_scopes(7, Some("read,write,admin"));
        assert!(key.scopes.contains(&Scope::Admin));
    }

    #[test]
    fn missing_column_keeps_legacy_admin() {
        // None == no stored scopes (pre-v53 rows / user-1 bootstrap path).
        let key = synthetic_key_for_identity_with_scopes(1, None);
        assert!(key.scopes.contains(&Scope::Admin));
    }
}
