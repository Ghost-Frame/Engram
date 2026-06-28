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
use kleos_lib::auth::{self, Scope};
use kleos_lib::memory;

mod types;
use types::{BulkArchiveBody, CreateMemoryBody, LoginForm, UpdateMemoryBody};

/// HMAC-SHA256 signer used for GUI session cookies.
type HmacSha256 = Hmac<Sha256>;

const GUI_COOKIE_MAX_AGE: i64 = 24 * 60 * 60; // 24 hours
const COOKIE_NAME: &str = "engram_auth";

/// Readable (non-HttpOnly) cookie carrying the CSRF token for the GUI session,
/// and the header the SPA echoes it in on mutating requests.
const CSRF_COOKIE_NAME: &str = "kleos_csrf";
const CSRF_HEADER_NAME: &str = "x-csrf-token";

/// Process-lifetime cache for the HMAC secret. Avoids file I/O on every
/// GUI request and eliminates the TOCTOU window between read-check and write.
static HMAC_SECRET_CACHE: OnceLock<SecretString> = OnceLock::new();

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

/// Validate the configured data_dir before using it to build filesystem
/// paths. Rejects empty strings, relative paths, and any component
/// containing `..` so a tampered config value cannot escape the data dir.
fn sanitize_data_dir(data_dir: &str) -> Option<PathBuf> {
    let path = PathBuf::from(data_dir);
    if data_dir.is_empty() || !path.is_absolute() {
        return None;
    }
    for component in path.components() {
        if matches!(component, std::path::Component::ParentDir) {
            return None;
        }
    }
    Some(path)
}

/// Load the HMAC secret from env, disk, or generate a new one.
/// Uses an atomic rename (write tmp + rename) to avoid partial-write corruption.
async fn load_or_generate_hmac_secret(data_dir: &str) -> SecretString {
    if let Ok(secret) = kleos_lib::kleos_env("HMAC_SECRET") {
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

    let validated_dir = match sanitize_data_dir(data_dir) {
        Some(p) => p,
        None => {
            tracing::error!(
                path = data_dir,
                "refusing to use non-absolute or traversing data_dir; falling back to ephemeral HMAC secret"
            );
            let mut raw = [0u8; 32];
            use rand::rngs::OsRng;
            use rand::TryRngCore;
            OsRng
                .try_fill_bytes(&mut raw)
                .expect("OS CSPRNG must be available");
            let mut out = String::with_capacity(64);
            for byte in raw {
                use std::fmt::Write;
                let _ = write!(&mut out, "{:02x}", byte);
            }
            return SecretString::new(out);
        }
    };
    let secret_path = validated_dir.join(".hmac_secret");

    // Try to read existing secret. H7: validate the format before trusting
    // the file -- a truncated or whitespace-corrupted file would silently
    // weaken the HMAC key. Expect 64 hex chars (32 bytes).
    if let Ok(raw) = fs::read_to_string(&secret_path).await {
        let trimmed = raw.trim();
        let valid = trimmed.len() == 64 && trimmed.chars().all(|c| c.is_ascii_hexdigit());
        if valid {
            tighten_secret_perms(&secret_path).await;
            return SecretString::new(trimmed.to_string());
        }
        tracing::warn!(
            path = ?secret_path,
            len = trimmed.len(),
            "existing hmac secret has invalid format (expected 64 hex chars); regenerating"
        );
    }

    // Generate new secret: 32 bytes from OsRng, hex encoded.
    let secret = {
        use rand::rngs::OsRng;
        use rand::TryRngCore;
        let mut raw = [0u8; 32];
        OsRng
            .try_fill_bytes(&mut raw)
            .expect("OS CSPRNG must be available");
        let mut out = String::with_capacity(64);
        for byte in raw {
            use std::fmt::Write;
            let _ = write!(&mut out, "{:02x}", byte);
        }
        SecretString::new(out)
    };

    // Ensure data dir exists (6.8: surface genuine failures via warn log).
    if let Err(e) = fs::create_dir_all(&validated_dir).await {
        tracing::warn!(path = %validated_dir.display(), error = %e, "failed to create hmac secret data dir");
    }

    // H7 + L8: create the tmp file with mode 0o600 from the start so a
    // multi-user host cannot read the secret in the window between the
    // initial write and the post-rename chmod. fsync before rename to
    // ensure durability of the bytes before they become reachable under
    // the canonical path. tighten_secret_perms remains as belt-and-braces.
    let tmp_path = secret_path.with_extension("tmp");
    let write_ok = write_secret_file_0600(&tmp_path, secret.expose_secret().as_bytes()).await;
    if write_ok {
        if let Err(e) = fs::rename(&tmp_path, &secret_path).await {
            tracing::warn!(error = %e, "failed to rename hmac secret tmp file");
            let _ = fs::remove_file(&tmp_path).await;
        }
    } else {
        let _ = fs::remove_file(&tmp_path).await;
    }
    tighten_secret_perms(&secret_path).await;
    tracing::info!(path = ?secret_path, "generated HMAC secret");

    secret
}

/// H7: open `path` with mode 0o600 (Unix) and write `bytes` + fsync. On
/// Windows the umask isn't relevant; create + write + sync is sufficient
/// because Windows ACLs default to user-restrictive. Returns false on any
/// error (caller logs via warn elsewhere).
#[cfg(unix)]
/// Write a GUI secret file with owner-only permissions on Unix systems.
async fn write_secret_file_0600(path: &std::path::Path, bytes: &[u8]) -> bool {
    use tokio::io::AsyncWriteExt;
    let mut opts = fs::OpenOptions::new();
    opts.create(true).write(true).truncate(true).mode(0o600);
    let mut f = match opts.open(path).await {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(path = ?path, error = %e, "failed to create hmac secret tmp file");
            return false;
        }
    };
    if let Err(e) = f.write_all(bytes).await {
        tracing::warn!(path = ?path, error = %e, "failed to write hmac secret bytes");
        return false;
    }
    if let Err(e) = f.sync_all().await {
        tracing::warn!(path = ?path, error = %e, "failed to fsync hmac secret tmp file");
        return false;
    }
    true
}

#[cfg(not(unix))]
/// Write a GUI secret file on platforms without Unix permission bits.
async fn write_secret_file_0600(path: &std::path::Path, bytes: &[u8]) -> bool {
    use tokio::io::AsyncWriteExt;
    let mut opts = fs::OpenOptions::new();
    opts.create(true).write(true).truncate(true);
    let mut f = match opts.open(path).await {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(path = ?path, error = %e, "failed to create hmac secret tmp file");
            return false;
        }
    };
    if let Err(e) = f.write_all(bytes).await {
        tracing::warn!(path = ?path, error = %e, "failed to write hmac secret bytes");
        return false;
    }
    let _ = f.sync_all().await;
    true
}

#[cfg(unix)]
/// Tighten an existing GUI secret file to owner-only permissions on Unix systems.
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
/// Warn operators that secret-file permission tightening is unavailable.
async fn tighten_secret_perms(path: &std::path::Path) {
    // R8 S-004: on non-unix targets we cannot chmod. Warn once per process
    // so operators notice that the HMAC secret on disk inherits default ACLs.
    // The recommended hardening is an NTFS ACL restricting the file to the
    // service account (icacls / windows-rs SetNamedSecurityInfo).
    use std::sync::atomic::{AtomicBool, Ordering};
    static WARNED: AtomicBool = AtomicBool::new(false);
    if !WARNED.swap(true, Ordering::Relaxed) {
        tracing::warn!(
            path = ?path,
            "tighten_secret_perms is a no-op on this platform; restrict HMAC secret ACL manually (NTFS: icacls <path> /inheritance:r /grant:r SYSTEM:F Administrators:F <service-account>:F)"
        );
    }
}

/// Authenticated GUI session resolved from a signed cookie.
#[derive(Debug, Clone)]
pub struct GuiSession {
    pub user_id: i64,
    pub key_id: i64,
    pub scopes: Vec<Scope>,
}

/// Provides helpers for checking resolved GUI session authorization.
impl GuiSession {
    /// Return whether this session grants the requested scope or admin.
    pub fn has_scope(&self, scope: &Scope) -> bool {
        self.scopes.contains(scope) || self.scopes.contains(&Scope::Admin)
    }
}

/// Encode scopes into the compact cookie payload representation.
fn encode_scopes(scopes: &[Scope]) -> String {
    scopes
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// Decode scopes from the compact cookie payload representation.
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

/// Derive a CSRF token bound to a GUI session cookie:
/// `HMAC(secret, "csrf:" + session_cookie)`. Because the server recomputes and
/// validates it from the session cookie rather than trusting a separately
/// settable cookie, an attacker who can fixate the readable `kleos_csrf` cookie
/// still cannot forge a token that matches the victim's session. The token is
/// not a credential on its own -- it is only meaningful alongside the HttpOnly,
/// HMAC-signed session cookie -- so delivering it to JS is safe and lets the
/// SPA authenticate writes via the cookie instead of a long-lived localStorage
/// bearer key.
fn compute_csrf_token(session_cookie_value: &str, secret: &SecretString) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.expose_secret().as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(b"csrf:");
    mac.update(session_cookie_value.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Verify the `X-CSRF-Token` header against the session cookie for a mutating
/// GUI-cookie request. True only when both are present and the header equals
/// the recomputed token (constant-time).
pub async fn verify_gui_csrf(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(cookie_value) = get_auth_cookie(headers) else {
        return false;
    };
    let Some(header_token) = headers.get(CSRF_HEADER_NAME).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let secret = get_hmac_secret(&state.config.data_dir).await;
    let expected = compute_csrf_token(&cookie_value, &secret);
    expected
        .as_bytes()
        .ct_eq(header_token.as_bytes())
        .unwrap_u8()
        == 1
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

    // Check expiration and future timestamps (reject beyond 60s clock skew).
    // R7-008: a cookie whose ts is in the future would otherwise survive the
    // intended max-age window by an arbitrary offset. HMAC verification alone
    // does not bound ts; we cap drift at 60s to tolerate normal clock skew.
    let now = chrono::Utc::now().timestamp();
    if ts - now > 60 {
        return None;
    }
    if now - ts > GUI_COOKIE_MAX_AGE {
        return None;
    }

    Some(GuiSession {
        user_id,
        key_id,
        scopes,
    })
}

/// Extract the kleos_auth cookie from headers
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
    let active_key = kleos_lib::auth::get_active_key_by_id(&state.db, session.key_id)
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
        return Err(AppError::from(kleos_lib::EngError::Internal(
            "server is in safe mode; writes are temporarily disabled".into(),
        )));
    }
    let session = get_gui_session(state, headers)
        .await
        .ok_or_else(|| AppError::from(kleos_lib::EngError::Auth("GUI auth required".into())))?;
    if !session.has_scope(&scope) {
        return Err(AppError::from(kleos_lib::EngError::Auth(format!(
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
        kleos_lib::kleos_env("SECURE_COOKIES")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    });

    // L-R3-005: prefer SameSite=Strict regardless of ENGRAM_SECURE_COOKIES.
    // The previous default (Lax in non-secure mode) let any cross-site
    // top-level GET (including links to /gui/logout) carry the cookie,
    // which made forced-logout CSRF trivially exploitable.
    if secure {
        "Path=/; HttpOnly; Secure; SameSite=Strict"
    } else {
        "Path=/; HttpOnly; SameSite=Strict"
    }
}

/// Cookie attributes for the readable CSRF token: same SameSite=Strict / Secure
/// policy as the session cookie but WITHOUT HttpOnly, so the SPA can read the
/// token and echo it in the X-CSRF-Token header.
fn cookie_attributes_readable() -> &'static str {
    static SECURE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let secure = *SECURE.get_or_init(|| {
        kleos_lib::kleos_env("SECURE_COOKIES")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    });
    if secure {
        "Path=/; Secure; SameSite=Strict"
    } else {
        "Path=/; SameSite=Strict"
    }
}

/// POST /gui/auth - authenticate with API key
async fn gui_auth(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Response {
    // SECURITY (SEC-MED-7): gui_enabled controls whether the GUI is reachable.
    // It does NOT gate a password prompt -- actual authentication uses the API
    // key submitted in the form. Set KLEOS_GUI_PASSWORD or the legacy
    // ENGRAM_GUI_PASSWORD to any non-empty value to enable the GUI.
    if !state.config.gui_enabled {
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

    // Companion CSRF cookie (readable by JS) bound to the session cookie. The
    // SPA echoes it in X-CSRF-Token on mutating requests so cookie-authenticated
    // writes are CSRF-safe -- removing the need to persist a raw API key in
    // localStorage.
    let csrf_token = compute_csrf_token(&cookie_value, &secret);
    let csrf_cookie = format!(
        "{}={}; Max-Age={}; {}",
        CSRF_COOKIE_NAME,
        csrf_token,
        GUI_COOKIE_MAX_AGE,
        cookie_attributes_readable()
    );

    Response::builder()
        .status(StatusCode::OK)
        .header(header::SET_COOKIE, cookie)
        .header(header::SET_COOKIE, csrf_cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(format!(
            r#"{{"ok":true,"user_id":{}}}"#,
            auth_ctx.user_id
        )))
        .unwrap()
}

/// POST /gui/logout - clear auth cookie. POST-only so a top-level GET
/// navigation from a third-party site cannot force-logout the user
/// (L-R3-005). SameSite=Strict on the cookie is the primary defense; this
/// is belt-and-suspenders.
async fn gui_logout(headers: HeaderMap) -> Response {
    let attrs = cookie_attributes(&headers);
    let cookie = format!("{}=; Max-Age=0; {}", COOKIE_NAME, attrs);
    let csrf_clear = format!(
        "{}=; Max-Age=0; {}",
        CSRF_COOKIE_NAME,
        cookie_attributes_readable()
    );

    Response::builder()
        .status(StatusCode::OK)
        .header(header::SET_COOKIE, cookie)
        .header(header::SET_COOKIE, csrf_clear)
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
    let candidates = [PathBuf::from("gui/build")];

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

// H-013: login CSS and JS are embedded as static string constants and served
// via dedicated routes so they work without a gui/build directory and allow
// 'unsafe-inline' to be removed from the CSP. (gui/build/ is .gitignored
// as a generated artifact, so content is inlined here instead of include_str!.)
const LOGIN_CSS: &str = "body { font-family: system-ui, sans-serif; background: #1a1a2e; color: #eee; display: flex; justify-content: center; align-items: center; min-height: 100vh; margin: 0; }\n\
.login-box { background: #16213e; padding: 2rem; border-radius: 8px; box-shadow: 0 4px 24px rgba(0,0,0,0.3); max-width: 400px; width: 100%; }\n\
h1 { margin: 0 0 1.5rem; font-size: 1.5rem; text-align: center; color: #7f5af0; }\n\
input { width: 100%; padding: 0.75rem; margin-bottom: 1rem; border: 1px solid #2d3a52; border-radius: 4px; background: #0f0f1e; color: #eee; box-sizing: border-box; font-family: monospace; font-size: 0.9rem; }\n\
button { width: 100%; padding: 0.75rem; background: #7f5af0; color: #fff; border: none; border-radius: 4px; cursor: pointer; font-size: 1rem; }\n\
button:hover { background: #6b4ed1; }\n\
.error { color: #ff6b6b; margin-bottom: 1rem; text-align: center; display: none; }\n\
.hint { color: #888; font-size: 0.8rem; text-align: center; margin-top: 1rem; }\n";

const LOGIN_JS: &str = "document.getElementById('login-form').addEventListener('submit', async (e) => {\n\
    e.preventDefault();\n\
    const form = e.target;\n\
    const error = document.getElementById('error');\n\
    const data = new FormData(form);\n\
    error.style.display = 'none';\n\
    error.textContent = 'Invalid API key';\n\
    try {\n\
        const res = await fetch('/gui/auth', { method: 'POST', body: new URLSearchParams(data) });\n\
        if (res.ok) {\n\
            window.location.href = '/';\n\
            return;\n\
        }\n\
        const message = await res.text();\n\
        if (message) {\n\
            error.textContent = message;\n\
        }\n\
        error.style.display = 'block';\n\
    } catch (_error) {\n\
        error.textContent = 'Unable to reach Kleos. Check that the server is running and try again.';\n\
        error.style.display = 'block';\n\
    }\n\
});\n";

/// Serve login page HTML (H-013: no inline style or script -- external assets only)
fn login_html() -> &'static str {
    r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Kleos Login</title>
    <link rel="stylesheet" href="/_app/login.css">
</head>
<body>
    <div class="login-box">
        <h1>Kleos</h1>
        <div class="error" id="error">Invalid API key</div>
        <form id="login-form">
            <input type="password" name="api_key" placeholder="API Key" autofocus required>
            <button type="submit">Login</button>
        </form>
        <p class="hint">Use your API key to authenticate</p>
    </div>
    <script src="/_app/login.js"></script>
</body>
</html>"#
}

/// GET /_app/login.css -- H-013: serve embedded login stylesheet (no build dir needed)
async fn serve_login_css() -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/css; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store, no-cache, must-revalidate")
        .body(Body::from(LOGIN_CSS))
        .unwrap()
}

/// GET /_app/login.js -- H-013: serve embedded login script (no build dir needed)
async fn serve_login_js() -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )
        .header(header::CACHE_CONTROL, "no-store, no-cache, must-revalidate")
        .body(Body::from(LOGIN_JS))
        .unwrap()
}

/// GET /_app/* - serve bundled SPA static assets.
async fn serve_app_assets(State(state): State<AppState>, Path(path): Path<String>) -> Response {
    // SECURITY: when gui_enabled is false, the GUI is disabled entirely.
    if !state.config.gui_enabled {
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

/// Resolve the GUI build directory once per process.
fn get_cached_build_dir(state: &AppState) -> Option<PathBuf> {
    CACHED_BUILD_DIR
        .get_or_init(|| resolve_gui_build_dir(state))
        .clone()
}

/// Return whether a path should always be handled as a static asset request.
fn is_static_asset_path(path: &str) -> bool {
    path.starts_with("/_app/")
        || path
            .rsplit('/')
            .next()
            .map(|segment| segment.contains('.'))
            .unwrap_or(false)
}

/// Return whether a path starts with a named top-level segment.
fn has_top_level_segment(path: &str, segment: &str) -> bool {
    let trimmed = path.trim_start_matches('/');
    trimmed == segment || trimmed.starts_with(&format!("{segment}/"))
}

/// Return whether a path is one of the coordination service API subpaths.
fn is_coordination_api_path(path: &str) -> bool {
    const COORDINATION_SEGMENTS: &[&str] = &["axon", "broca", "chiasm", "loom", "soma", "thymus"];

    COORDINATION_SEGMENTS
        .iter()
        .any(|segment| has_top_level_segment(path, segment) && path != format!("/{segment}"))
}

/// Return whether a memory path targets a numeric memory API route.
fn is_memory_api_path(path: &str) -> bool {
    let mut segments = path.trim_start_matches('/').split('/');
    matches!(segments.next(), Some("memory"))
        && segments
            .next()
            .and_then(|id| id.parse::<i64>().ok())
            .is_some()
}

/// Return whether a path is an API route that the SPA fallback must not swallow.
fn is_api_route_path(path: &str) -> bool {
    if matches!(path, "/gui/auth" | "/gui/logout") || path.starts_with("/gui/memories") {
        return true;
    }
    if is_coordination_api_path(path) || is_memory_api_path(path) {
        return true;
    }

    let first_segment = path
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or_default();

    matches!(
        first_segment,
        "activity"
            | "admin"
            | "agents"
            | "approvals"
            | "artifacts"
            | "audit"
            | "batch"
            | "brain"
            | "claims"
            | "commerce"
            | "communities"
            | "consolidate"
            | "context"
            | "contradictions"
            | "conversations"
            | "decay"
            | "digests"
            | "dispatch"
            | "docs"
            | "entities"
            | "entity-relationships"
            | "episodes"
            | "errors"
            | "export"
            | "facts"
            | "fetch"
            | "feed"
            | "fsrs"
            | "gate"
            | "graph"
            | "grounding"
            | "growth"
            | "handoffs"
            | "health"
            | "identities"
            | "identity-keys"
            | "import"
            | "ingest"
            | "inbox"
            | "intelligence"
            | "jobs"
            | "keys"
            | "list"
            | "live"
            | "mcp"
            | "mcp-tokens"
            | "messages"
            | "metrics"
            | "onboard"
            | "pack"
            | "pending"
            | "personality"
            | "policy"
            | "projects"
            | "prompts"
            | "queue"
            | "quota"
            | "rate-limit"
            | "ready"
            | "reflect"
            | "reflections"
            | "schema"
            | "scratch"
            | "search"
            | "sessions"
            | "skills"
            | "spaces"
            | "stats"
            | "structural"
            | "supervisor"
            | "sync"
            | "tasks"
            | "users"
            | "usage"
            | "verify"
            | "webhooks"
    )
}

/// Return whether an HTML GET path should serve the SPA index.
fn is_spa_fallback_path(path: &str) -> bool {
    path == "/" || (!is_static_asset_path(path) && !is_api_route_path(path))
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

    // Check if this is a non-API HTML route.
    let accepts_html = request
        .headers()
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("text/html"))
        .unwrap_or(false);

    if !accepts_html || !is_spa_fallback_path(path) {
        return next.run(request).await;
    }

    // Check if GUI build is available
    let Some(build_dir) = get_cached_build_dir(&state) else {
        return next.run(request).await;
    };

    // SECURITY: when gui_enabled is false, the GUI is disabled.
    // Pass through to next handler (which will 404) rather than serving an app shell.
    if !state.config.gui_enabled {
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
        return Err(AppError::from(kleos_lib::EngError::InvalidInput(
            "content is required".into(),
        )));
    }

    // M-R3-007: route writes to the caller's shard so /gui/memory/* and
    // /memory/* share the same backing store.
    let db = crate::extractors::resolve_db_for_user(&state, user_id).await?;
    let result = memory::store(
        &db,
        memory::types::StoreRequest {
            content: content.to_string(),
            category: body.category.unwrap_or_else(|| "general".to_string()),
            source: "gui".to_string(),
            importance: body.importance.unwrap_or(5).clamp(1, 10),
            tags: body.tags,
            is_static: body.is_static,
            user_id: Some(user_id),
            ..Default::default()
        },
        None,
        false,
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(json!({ "created": true, "id": result.id })),
    ))
}

/// PATCH /gui/memories/{id} - update a memory through GUI cookie auth.
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
        chunk_embeddings: None,
    };

    let db = crate::extractors::resolve_db_for_user(&state, user_id).await?;
    memory::update(&db, id, req, user_id, false).await?;
    Ok(Json(json!({ "updated": true, "id": id })))
}

/// DELETE /gui/memories/{id} - delete a memory through GUI cookie auth.
async fn gui_delete_memory(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let user_id = require_gui_scope(&state, &headers, Scope::Write).await?;

    let db = crate::extractors::resolve_db_for_user(&state, user_id).await?;
    memory::delete(&db, id, user_id).await?;
    Ok(Json(json!({ "deleted": true, "id": id })))
}

/// POST /gui/memories/bulk-archive - archive a batch of memories through GUI cookie auth.
async fn gui_bulk_archive(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<BulkArchiveBody>,
) -> Result<Json<Value>, AppError> {
    let user_id = require_gui_scope(&state, &headers, Scope::Write).await?;

    let db = crate::extractors::resolve_db_for_user(&state, user_id).await?;
    let mut archived = 0;
    for id in &body.ids {
        if memory::mark_archived(&db, *id, user_id).await.is_ok() {
            archived += 1;
        }
    }

    Ok(Json(json!({ "archived": archived })))
}

/// Turn a track filename ("moonlit-dreams.mp3") into a display name
/// ("Moonlit Dreams"). An optional names.json sidecar (handled in the manifest
/// handler) overrides this for tracks that need exact titles.
fn prettify_track_name(file_name: &str) -> String {
    let stem = file_name.strip_suffix(".mp3").unwrap_or(file_name);
    stem.split(['-', '_'])
        .filter(|s| !s.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// GET /media/music/manifest.json -- list available tracks. Empty array when
/// no music dir is configured or the dir cannot be read, which keeps the GUI
/// player hidden by default.
async fn serve_music_manifest(State(state): State<AppState>) -> Response {
    let Some(dir) = state.config.gui_music_dir.as_ref() else {
        return music_json("[]".to_string());
    };
    // Reject any parent-directory traversal in the operator-configured music
    // dir before touching the filesystem. This `..` containment check is also
    // the sanitizer the CodeQL rust/path-injection query recognizes: the dir is
    // used only on the branch where the check passed, so the env-sourced path no
    // longer taints the reads below.
    if dir.contains("..") {
        return music_json("[]".to_string());
    }
    let root = std::path::Path::new(dir);
    // Optional sidecar of exact titles.
    let overrides: std::collections::HashMap<String, String> =
        match fs::read_to_string(root.join("names.json")).await {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
            Err(_) => std::collections::HashMap::new(),
        };
    let mut entries: Vec<Value> = Vec::new();
    if let Ok(mut rd) = fs::read_dir(root).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.to_ascii_lowercase().ends_with(".mp3") {
                continue;
            }
            let title = overrides
                .get(&name)
                .cloned()
                .unwrap_or_else(|| prettify_track_name(&name));
            entries.push(json!({ "src": name, "name": title }));
        }
    }
    // Stable order so the player track index is deterministic across loads.
    entries.sort_by(|a, b| a["src"].as_str().cmp(&b["src"].as_str()));
    music_json(serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string()))
}

/// Build a no-store JSON response for the music manifest.
fn music_json(body: String) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(body))
        .unwrap()
}

/// GET /media/music/{file} -- serve one mp3 from the configured music dir.
/// Path-traversal-safe: the canonical resolved path must stay within the dir,
/// and only .mp3 files are served.
async fn serve_music_file(State(state): State<AppState>, Path(file): Path<String>) -> Response {
    let Some(dir) = state.config.gui_music_dir.as_ref() else {
        return (StatusCode::NOT_FOUND, "no music").into_response();
    };
    // Accept only a bare .mp3 filename. The strict character allowlist rejects
    // path separators, parent-dir segments, and any other character, so the
    // user-supplied value cannot escape the configured music directory.
    let valid = file.len() <= 255
        && file.to_ascii_lowercase().ends_with(".mp3")
        && !file.contains("..")
        && file
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !valid {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    let root = std::path::Path::new(dir);
    let full = root.join(&file);
    // Defense in depth: the resolved path must still resolve inside the dir.
    let (Ok(canonical), Ok(root_canonical)) = (full.canonicalize(), root.canonicalize()) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    if !canonical.starts_with(&root_canonical) {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    match fs::read(&canonical).await {
        Ok(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "audio/mpeg")
            .header(header::CACHE_CONTROL, "public, max-age=86400")
            .body(Body::from(bytes))
            .unwrap(),
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// Build the GUI router with auth, static assets, and memory mutation routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/gui/auth", post(gui_auth))
        .route("/gui/logout", post(gui_logout))
        // H-013: login assets served as embedded constants; no build dir needed.
        // These specific routes shadow the wildcard below for these paths.
        .route("/_app/login.css", get(serve_login_css))
        .route("/_app/login.js", get(serve_login_js))
        .route("/_app/{*path}", get(serve_app_assets))
        .route("/media/music/manifest.json", get(serve_music_manifest))
        .route("/media/music/{file}", get(serve_music_file))
        // GUI memory CRUD
        .route("/gui/memories", post(gui_create_memory))
        .route(
            "/gui/memories/{id}",
            patch(gui_update_memory).delete(gui_delete_memory),
        )
        .route("/gui/memories/bulk-archive", post(gui_bulk_archive))
}

/// Unit tests for GUI auth helpers and embedded login assets.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prettify_track_name_humanizes_filenames() {
        assert_eq!(prettify_track_name("moonlit-dreams.mp3"), "Moonlit Dreams");
        assert_eq!(prettify_track_name("soft_bee_pulse.mp3"), "Soft Bee Pulse");
    }

    // The login bridge establishes the cookie session via /gui/auth and
    // redirects to the app. It must NOT persist the raw API key in localStorage
    // (the cookie + CSRF token returned by /gui/auth are the credential now).
    #[test]
    fn login_js_uses_cookie_session_not_localstorage() {
        assert!(LOGIN_JS.contains("fetch('/gui/auth'"));
        assert!(LOGIN_JS.contains("window.location.href = '/'"));
        assert!(
            !LOGIN_JS.contains("localStorage.setItem('kleos_api_key'"),
            "the inline login must not persist the raw API key"
        );
    }

    // Verify network failures render a visible login error instead of silently throwing in the page.
    #[test]
    fn login_js_handles_fetch_failures() {
        assert!(LOGIN_JS.contains("try {"));
        assert!(LOGIN_JS.contains("catch"));
        assert!(LOGIN_JS.contains("Unable to reach Kleos"));
    }

    // The CSRF token must be deterministic per session, bound to both the
    // session cookie and the server secret, and full SHA-256 width.
    #[test]
    fn csrf_token_is_session_and_secret_bound() {
        let secret = SecretString::new("unit-test-secret".to_string());
        let session_a = "1:7:1700000000:rw.deadbeef";
        let session_b = "2:9:1700000000:rw.cafebabe";

        let t_a = compute_csrf_token(session_a, &secret);
        assert_eq!(t_a, compute_csrf_token(session_a, &secret), "deterministic");
        assert_ne!(
            t_a,
            compute_csrf_token(session_b, &secret),
            "different session -> different token"
        );
        assert_eq!(t_a.len(), 64, "hex-encoded sha256");

        let other = SecretString::new("a-different-secret".to_string());
        assert_ne!(
            compute_csrf_token(session_a, &other),
            t_a,
            "token is bound to the server secret"
        );
    }
}
