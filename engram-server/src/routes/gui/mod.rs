use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, Request, StatusCode},
    middleware::Next,
    response::{Html, IntoResponse, Response},
    routing::{get, patch, post},
    Form, Json, Router,
};
use hmac::{Hmac, Mac};
use secrecy::{ExposeSecret, SecretString};
use serde_json::{json, Value};
use sha2::Sha256;
use std::path::PathBuf;
use std::sync::OnceLock;
use subtle::ConstantTimeEq;
use tokio::fs;

use crate::error::AppError;
use crate::state::AppState;
use engram_lib::auth::{self, Scope};
use engram_lib::memory;

mod types;
use types::{BulkArchiveBody, CreateMemoryBody, LoginForm, UpdateMemoryBody};

type HmacSha256 = Hmac<Sha256>;

const GUI_COOKIE_MAX_AGE: i64 = 7 * 24 * 60 * 60; // 7 days
const COOKIE_NAME: &str = "engram_auth";

/// Process-lifetime cache for the HMAC secret. Avoids file I/O on every
/// GUI request and eliminates the TOCTOU window between read-check and write.
static HMAC_SECRET_CACHE: OnceLock<SecretString> = OnceLock::new();

// SPA routes that serve index.html
const SPA_ROUTES: &[&str] = &[
    "/",
    "/gui",
    "/graph",
    "/search",
    "/inbox",
    "/timeline",
    "/entities",
    "/projects",
];

// MIME types for static assets
fn mime_for_extension(ext: &str) -> &'static str {
    match ext {
        "js" => "application/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "wasm" => "application/wasm",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "html" => "text/html; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Get or create the HMAC secret for cookie signing.
///
/// SECURITY (SEC-HIGH-1): the secret is 32 bytes drawn from `OsRng` and
/// encoded as hex, giving a full 256-bit key. The previous implementation
/// concatenated two UUID v4 strings, which carry only ~122 bits of entropy
/// each and embed fixed version/variant nibbles, so the effective search
/// space for cookie forgery was substantially below the 256-bit strength
/// implied by the Sha256 MAC. The on-disk secret is chmod 0o600 so another
/// user on the box cannot read it and forge GUI auth cookies; if we find
/// an existing file with permissive bits we tighten them in place.
async fn get_hmac_secret(data_dir: &str) -> SecretString {
    // Fast path: return cached value (eliminates file I/O after first call).
    if let Some(cached) = HMAC_SECRET_CACHE.get() {
        return cached.clone();
    }

    let secret = load_or_generate_hmac_secret(data_dir).await;

    // Race is harmless: both concurrent callers compute from the same file.
    // The loser ignores its result and uses the winner's cached value.
    let _ = HMAC_SECRET_CACHE.set(secret.clone());
    HMAC_SECRET_CACHE.get().cloned().unwrap_or(secret)
}

/// Load the HMAC secret from env, disk, or generate a new one.
/// Uses an atomic rename (write tmp + rename) to avoid partial-write corruption.
async fn load_or_generate_hmac_secret(data_dir: &str) -> SecretString {
    if let Ok(secret) = std::env::var("ENGRAM_HMAC_SECRET") {
        // SECURITY (SEC-LOW-6): reject HMAC secrets shorter than 32 chars
        // to prevent weak signing keys.
        if secret.len() < 32 {
            tracing::error!(
                len = secret.len(),
                "ENGRAM_HMAC_SECRET is too short (minimum 32 characters); ignoring"
            );
        } else {
            return SecretString::new(secret);
        }
    }

    let secret_path = PathBuf::from(data_dir).join(".hmac_secret");

    // Try to read existing secret
    if let Ok(secret) = fs::read_to_string(&secret_path).await {
        tighten_secret_perms(&secret_path).await;
        return SecretString::new(secret);
    }

    // Generate new secret: 32 bytes from OsRng, hex encoded.
    let secret = {
        use rand::Rng;
        let mut raw = [0u8; 32];
        rand::rng().fill(&mut raw);
        let mut out = String::with_capacity(64);
        for byte in raw {
            use std::fmt::Write;
            let _ = write!(&mut out, "{:02x}", byte);
        }
        SecretString::new(out)
    };

    // Ensure data dir exists (6.8: surface genuine failures via warn log).
    if let Err(e) = fs::create_dir_all(data_dir).await {
        tracing::warn!(path = data_dir, error = %e, "failed to create hmac secret data dir");
    }

    // Atomic write: write to .tmp then rename to avoid TOCTOU partial-write.
    // rename(2) is atomic on POSIX; on Windows this falls back to a replace.
    let tmp_path = secret_path.with_extension("tmp");
    if fs::write(&tmp_path, secret.expose_secret()).await.is_ok() {
        if let Err(e) = fs::rename(&tmp_path, &secret_path).await {
            tracing::warn!(error = %e, "failed to rename hmac secret tmp file");
        }
    }
    tighten_secret_perms(&secret_path).await;
    tracing::info!(path = ?secret_path, "generated HMAC secret");

    secret
}

#[cfg(unix)]
async fn tighten_secret_perms(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::metadata(path).await {
        let mut perms = meta.permissions();
        if perms.mode() & 0o777 != 0o600 {
            perms.set_mode(0o600);
            if let Err(e) = fs::set_permissions(path, perms).await {
                tracing::warn!(path = ?path, error = %e, "failed to chmod 0o600 on hmac secret");
            }
        }
    }
}

#[cfg(not(unix))]
async fn tighten_secret_perms(_path: &std::path::Path) {}

/// Authenticated GUI session resolved from a signed cookie.
#[derive(Debug, Clone)]
pub struct GuiSession {
    pub user_id: i64,
    pub key_id: i64,
    pub scopes: Vec<Scope>,
}

impl GuiSession {
    pub fn has_scope(&self, scope: &Scope) -> bool {
        self.scopes.contains(scope) || self.scopes.contains(&Scope::Admin)
    }
}

fn encode_scopes(scopes: &[Scope]) -> String {
    scopes
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn decode_scopes(raw: &str) -> Vec<Scope> {
    raw.split(',')
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<Scope>().ok())
        .collect()
}

/// Sign user_id, key_id, timestamp, and scopes to create a cookie value.
/// Format: {user_id}:{key_id}:{timestamp}:{scopes}.{hmac}
fn sign_cookie(
    user_id: i64,
    key_id: i64,
    ts: i64,
    scopes: &[Scope],
    secret: &SecretString,
) -> String {
    let payload = format!("{}:{}:{}:{}", user_id, key_id, ts, encode_scopes(scopes));
    let mut mac = HmacSha256::new_from_slice(secret.expose_secret().as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(payload.as_bytes());
    let result = mac.finalize();
    let hex = hex::encode(result.into_bytes());
    format!("{}.{}", payload, hex)
}

/// Verify a cookie value and return the resolved session payload.
/// Cookie format: {user_id}:{key_id}:{timestamp}:{scopes}.{hmac}
fn verify_cookie(cookie: &str, secret: &SecretString) -> Option<GuiSession> {
    let dot_idx = cookie.find('.')?;
    let payload = &cookie[..dot_idx];
    let sig = &cookie[dot_idx + 1..];

    // SECURITY: verify HMAC BEFORE parsing payload fields. This means a
    // forged cookie can never select a user via the field parser alone.
    let mut mac = HmacSha256::new_from_slice(secret.expose_secret().as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(payload.as_bytes());
    let expected = hex::encode(mac.finalize().into_bytes());
    if expected.as_bytes().ct_eq(sig.as_bytes()).unwrap_u8() != 1 {
        return None;
    }

    // Parse payload: user_id:key_id:timestamp[:scopes]
    let mut parts = payload.splitn(4, ':');
    let user_id: i64 = parts.next()?.parse().ok()?;
    let key_id: i64 = parts.next()?.parse().ok()?;
    let ts: i64 = parts.next()?.parse().ok()?;
    let scopes = parts.next().map(decode_scopes).unwrap_or_default();

    // Check expiration
    let now = chrono::Utc::now().timestamp();
    if now - ts > GUI_COOKIE_MAX_AGE {
        return None;
    }

    Some(GuiSession {
        user_id,
        key_id,
        scopes,
    })
}

/// Extract the engram_auth cookie from headers
fn get_auth_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(|s| s.trim())
        .find(|s| s.starts_with(&format!("{}=", COOKIE_NAME)))?
        .strip_prefix(&format!("{}=", COOKIE_NAME))
        .map(|s| s.to_string())
}

/// Resolve the GUI session (user_id + scopes) from the cookie, if any.
#[tracing::instrument(skip(state, headers), fields(layer = "gui.session"))]
pub async fn get_gui_session(state: &AppState, headers: &HeaderMap) -> Option<GuiSession> {
    let cookie = get_auth_cookie(headers)?;
    let secret = get_hmac_secret(&state.config.data_dir).await;
    let session = verify_cookie(&cookie, &secret)?;
    let active_key = engram_lib::auth::get_active_key_by_id(&state.db, session.key_id)
        .await
        .ok()?;
    if active_key.user_id != session.user_id {
        return None;
    }
    Some(session)
}

/// Check if a request is authenticated via GUI cookie and return the user_id.
#[tracing::instrument(skip(state, headers), fields(layer = "gui.user_id"))]
pub async fn get_gui_user_id(state: &AppState, headers: &HeaderMap) -> Option<i64> {
    get_gui_session(state, headers).await.map(|s| s.user_id)
}

/// Require an authenticated GUI session with a specific scope.
///
/// SECURITY: write handlers must call this instead of `get_gui_user_id` so a
/// read-only API key cannot be used to create, update, or delete data via the
/// GUI cookie path.
async fn require_gui_scope(
    state: &AppState,
    headers: &HeaderMap,
    scope: Scope,
) -> Result<i64, AppError> {
    // SECURITY: enforce safe mode on GUI write paths (GUI routes bypass the
    // normal safe_mode_middleware because they are merged outside api_routes).
    if scope == Scope::Write && state.safe_mode.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(AppError::from(engram_lib::EngError::Internal(
            "server is in safe mode; writes are temporarily disabled".into(),
        )));
    }
    let session = get_gui_session(state, headers)
        .await
        .ok_or_else(|| AppError::from(engram_lib::EngError::Auth("GUI auth required".into())))?;
    if !session.has_scope(&scope) {
        return Err(AppError::from(engram_lib::EngError::Auth(format!(
            "GUI session missing required scope: {}",
            scope
        ))));
    }
    Ok(session.user_id)
}

/// Check if a request is authenticated via GUI cookie (bool convenience wrapper)
#[tracing::instrument(skip(state, headers), fields(layer = "gui.authenticated"))]
pub async fn is_gui_authenticated(state: &AppState, headers: &HeaderMap) -> bool {
    get_gui_user_id(state, headers).await.is_some()
}

/// Determine cookie attributes based on config, not client-supplied headers.
///
/// SECURITY (SEC-MED-6): X-Forwarded-Proto is trivially spoofable when no
/// trusted reverse proxy strips it. Use the `ENGRAM_SECURE_COOKIES` env var
/// (set to "1" or "true") when the server is behind TLS.
fn cookie_attributes(_headers: &HeaderMap) -> &'static str {
    static SECURE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let secure = *SECURE.get_or_init(|| {
        std::env::var("ENGRAM_SECURE_COOKIES")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    });

    if secure {
        "Path=/; HttpOnly; Secure; SameSite=Strict"
    } else {
        "Path=/; HttpOnly; SameSite=Lax"
    }
}

/// POST /gui/auth - authenticate with API key
async fn gui_auth(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Response {
    // SECURITY (SEC-MED-7): gui_password is used as a feature flag (Some = GUI
    // enabled, None = disabled). It does NOT gate a password prompt -- the actual
    // authentication uses the API key submitted in the form.
    if state.config.gui_password.is_none() {
        return (StatusCode::FORBIDDEN, "GUI authentication not configured").into_response();
    }

    // Validate the API key
    let auth_ctx = match auth::validate_key(&state.db, &form.api_key).await {
        Ok(ctx) => ctx,
        Err(_) => {
            return (StatusCode::UNAUTHORIZED, "Invalid API key").into_response();
        }
    };

    // Generate signed cookie with user_id and the key's scopes so write
    // handlers can re-check them without another DB round trip.
    let ts = chrono::Utc::now().timestamp();
    let secret = get_hmac_secret(&state.config.data_dir).await;
    let cookie_value = sign_cookie(
        auth_ctx.user_id,
        auth_ctx.key.id,
        ts,
        &auth_ctx.key.scopes,
        &secret,
    );

    let attrs = cookie_attributes(&headers);
    let cookie = format!(
        "{}={}; Max-Age={}; {}",
        COOKIE_NAME, cookie_value, GUI_COOKIE_MAX_AGE, attrs
    );

    Response::builder()
        .status(StatusCode::OK)
        .header(header::SET_COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(format!(
            r#"{{"ok":true,"user_id":{}}}"#,
            auth_ctx.user_id
        )))
        .unwrap()
}

/// GET /gui/logout - clear auth cookie
async fn gui_logout(headers: HeaderMap) -> Response {
    let attrs = cookie_attributes(&headers);
    let cookie = format!("{}=; Max-Age=0; {}", COOKIE_NAME, attrs);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::SET_COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"ok":true}"#))
        .unwrap()
}

/// Resolve GUI build directory
fn resolve_gui_build_dir(state: &AppState) -> Option<PathBuf> {
    if let Some(ref dir) = state.config.gui_build_dir {
        let path = PathBuf::from(dir);
        if path.join("index.html").exists() {
            return Some(path);
        }
    }

    // Try relative to current directory
    let candidates = [
        PathBuf::from("gui/build"),
        PathBuf::from("../engram/gui/build"),
    ];

    candidates
        .into_iter()
        .find(|path| path.join("index.html").exists())
}

/// Serve static file from GUI build
async fn serve_static_file(build_dir: &std::path::Path, file_path: &str) -> Option<Response> {
    // Security: prevent path traversal
    let file_path = file_path.trim_start_matches('/');
    let full_path = build_dir.join(file_path);

    // Ensure path stays within build_dir
    let canonical = match full_path.canonicalize() {
        Ok(p) => p,
        Err(_) => return None,
    };

    let build_canonical = match build_dir.canonicalize() {
        Ok(p) => p,
        Err(_) => return None,
    };

    if !canonical.starts_with(&build_canonical) {
        return None;
    }

    // Read file
    let content = fs::read(&canonical).await.ok()?;

    // Determine MIME type
    let ext = canonical.extension().and_then(|e| e.to_str()).unwrap_or("");
    let mime = mime_for_extension(ext);

    // Cache headers: immutable for _app/immutable/ (hashed filenames),
    // no-store for everything else so reverse proxies (Pangolin) don't cache stale HTML
    let cache = if file_path.contains("/immutable/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-store, no-cache, must-revalidate"
    };

    Some(
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime)
            .header(header::CACHE_CONTROL, cache)
            .body(Body::from(content))
            .unwrap(),
    )
}

/// Serve login page HTML
fn login_html() -> &'static str {
    r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Engram Login</title>
    <style>
        body { font-family: system-ui, sans-serif; background: #1a1a2e; color: #eee; display: flex; justify-content: center; align-items: center; min-height: 100vh; margin: 0; }
        .login-box { background: #16213e; padding: 2rem; border-radius: 8px; box-shadow: 0 4px 24px rgba(0,0,0,0.3); max-width: 400px; width: 100%; }
        h1 { margin: 0 0 1.5rem; font-size: 1.5rem; text-align: center; color: #7f5af0; }
        input { width: 100%; padding: 0.75rem; margin-bottom: 1rem; border: 1px solid #2d3a52; border-radius: 4px; background: #0f0f1e; color: #eee; box-sizing: border-box; font-family: monospace; font-size: 0.9rem; }
        button { width: 100%; padding: 0.75rem; background: #7f5af0; color: #fff; border: none; border-radius: 4px; cursor: pointer; font-size: 1rem; }
        button:hover { background: #6b4ed1; }
        .error { color: #ff6b6b; margin-bottom: 1rem; text-align: center; display: none; }
        .hint { color: #888; font-size: 0.8rem; text-align: center; margin-top: 1rem; }
    </style>
</head>
<body>
    <div class="login-box">
        <h1>Engram</h1>
        <div class="error" id="error">Invalid API key</div>
        <form id="login-form">
            <input type="password" name="api_key" placeholder="API Key (engram_...)" autofocus required>
            <button type="submit">Login</button>
        </form>
        <p class="hint">Use your API key to authenticate</p>
    </div>
    <script>
        document.getElementById('login-form').addEventListener('submit', async (e) => {
            e.preventDefault();
            const form = e.target;
            const data = new FormData(form);
            const res = await fetch('/gui/auth', { method: 'POST', body: new URLSearchParams(data) });
            if (res.ok) {
                window.location.href = '/gui';
            } else {
                document.getElementById('error').style.display = 'block';
            }
        });
    </script>
</body>
</html>"#
}

/// GET /_app/* - serve SvelteKit static assets
async fn serve_app_assets(State(state): State<AppState>, Path(path): Path<String>) -> Response {
    // SECURITY: when gui_password is not configured, the GUI is disabled entirely.
    if state.config.gui_password.is_none() {
        return (StatusCode::NOT_FOUND, "GUI not available").into_response();
    }
    let Some(build_dir) = resolve_gui_build_dir(&state) else {
        return (StatusCode::NOT_FOUND, "GUI not available").into_response();
    };

    let file_path = format!("_app/{}", path);
    match serve_static_file(&build_dir, &file_path).await {
        Some(resp) => resp,
        None => (StatusCode::NOT_FOUND, "Not found").into_response(),
    }
}

// Cache resolved build dir to avoid repeated filesystem checks
static CACHED_BUILD_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

fn get_cached_build_dir(state: &AppState) -> Option<PathBuf> {
    CACHED_BUILD_DIR
        .get_or_init(|| resolve_gui_build_dir(state))
        .clone()
}

/// Middleware that intercepts SPA routes when Accept: text/html is present.
/// Must be applied BEFORE the main router so it can intercept before API handlers.
#[tracing::instrument(skip_all, fields(middleware = "gui.spa"))]
pub async fn gui_spa_middleware(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let path = request.uri().path();
    let method = request.method();

    // Only intercept GET requests
    if method != axum::http::Method::GET {
        return next.run(request).await;
    }

    // Check if this is a SPA route AND accepts HTML
    let accepts_html = request
        .headers()
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("text/html"))
        .unwrap_or(false);

    if !accepts_html || !SPA_ROUTES.contains(&path) {
        return next.run(request).await;
    }

    // Check if GUI build is available
    let Some(build_dir) = get_cached_build_dir(&state) else {
        return next.run(request).await;
    };

    // SECURITY: when gui_password is not configured, the GUI is disabled.
    // Return 404 for all SPA routes to avoid serving an unauthenticated app shell.
    if state.config.gui_password.is_none() {
        return next.run(request).await;
    }

    // Check GUI auth if password is configured
    let headers = request.headers();
    if !is_gui_authenticated(&state, headers).await {
        return Html(login_html()).into_response();
    }

    // Serve index.html
    match serve_static_file(&build_dir, "index.html").await {
        Some(resp) => resp,
        None => next.run(request).await,
    }
}

// ---------------------------------------------------------------------------
// GUI Memory CRUD
// ---------------------------------------------------------------------------

async fn gui_create_memory(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateMemoryBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let user_id = require_gui_scope(&state, &headers, Scope::Write).await?;

    let content = body.content.trim();
    if content.is_empty() {
        return Err(AppError::from(engram_lib::EngError::InvalidInput(
            "content is required".into(),
        )));
    }

    let result = memory::store(
        &state.db,
        memory::types::StoreRequest {
            content: content.to_string(),
            category: body.category.unwrap_or_else(|| "general".to_string()),
            source: "gui".to_string(),
            importance: body.importance.unwrap_or(5).clamp(1, 10),
            tags: body.tags,
            embedding: None,
            session_id: None,
            is_static: body.is_static,
            user_id: Some(user_id),
            space_id: None,
            parent_memory_id: None,
        },
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(json!({ "created": true, "id": result.id })),
    ))
}

async fn gui_update_memory(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(body): Json<UpdateMemoryBody>,
) -> Result<Json<Value>, AppError> {
    let user_id = require_gui_scope(&state, &headers, Scope::Write).await?;

    let req = memory::types::UpdateRequest {
        content: body.content.map(|s| s.trim().to_string()),
        category: body.category,
        importance: body.importance.map(|i| i.clamp(1, 10)),
        is_static: body.is_static,
        tags: None,
        status: None,
        embedding: None,
    };

    memory::update(&state.db, id, req, user_id).await?;
    Ok(Json(json!({ "updated": true, "id": id })))
}

async fn gui_delete_memory(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let user_id = require_gui_scope(&state, &headers, Scope::Write).await?;

    memory::delete(&state.db, id, user_id).await?;
    Ok(Json(json!({ "deleted": true, "id": id })))
}

async fn gui_bulk_archive(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<BulkArchiveBody>,
) -> Result<Json<Value>, AppError> {
    let user_id = require_gui_scope(&state, &headers, Scope::Write).await?;

    let mut archived = 0;
    for id in &body.ids {
        if memory::mark_archived(&state.db, *id, user_id).await.is_ok() {
            archived += 1;
        }
    }

    Ok(Json(json!({ "archived": archived })))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/gui/auth", post(gui_auth))
        .route("/gui/logout", get(gui_logout))
        .route("/_app/{*path}", get(serve_app_assets))
        // GUI memory CRUD
        .route("/gui/memories", post(gui_create_memory))
        .route(
            "/gui/memories/{id}",
            patch(gui_update_memory).delete(gui_delete_memory),
        )
        .route("/gui/memories/bulk-archive", post(gui_bulk_archive))
}
