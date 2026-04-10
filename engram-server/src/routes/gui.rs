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
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;
use std::path::PathBuf;
use std::sync::OnceLock;
use tokio::fs;

use crate::error::AppError;
use crate::state::AppState;
use engram_lib::auth;
use engram_lib::memory;

type HmacSha256 = Hmac<Sha256>;

const GUI_COOKIE_MAX_AGE: i64 = 7 * 24 * 60 * 60; // 7 days
const COOKIE_NAME: &str = "engram_auth";

// SPA routes that serve index.html
const SPA_ROUTES: &[&str] = &[
    "/", "/gui", "/graph", "/search", "/inbox", "/timeline", "/entities", "/projects",
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

/// Get or create the HMAC secret for cookie signing
async fn get_hmac_secret(data_dir: &str) -> String {
    if let Ok(secret) = std::env::var("ENGRAM_HMAC_SECRET") {
        return secret;
    }

    let secret_path = PathBuf::from(data_dir).join(".hmac_secret");

    // Try to read existing secret
    if let Ok(secret) = fs::read_to_string(&secret_path).await {
        return secret;
    }

    // Generate new secret
    let secret = format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4());

    // Ensure data dir exists
    let _ = fs::create_dir_all(data_dir).await;

    // Write secret (ignore errors, we'll use the generated one anyway)
    let _ = fs::write(&secret_path, &secret).await;
    tracing::info!(path = ?secret_path, "generated HMAC secret");

    secret
}

/// Sign user_id and timestamp to create a cookie value
/// Format: {user_id}:{timestamp}.{hmac}
fn sign_cookie(user_id: i64, ts: i64, secret: &str) -> String {
    let payload = format!("{}:{}", user_id, ts);
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(payload.as_bytes());
    let result = mac.finalize();
    let hex = hex::encode(result.into_bytes());
    format!("{}.{}", payload, hex)
}

/// Verify a cookie value and check expiration
/// Returns the user_id if valid, None otherwise
/// Cookie format: {user_id}:{timestamp}.{hmac}
fn verify_cookie(cookie: &str, secret: &str) -> Option<i64> {
    let dot_idx = cookie.find('.')?;
    let payload = &cookie[..dot_idx];
    let sig = &cookie[dot_idx + 1..];

    // Parse payload: user_id:timestamp
    let colon_idx = payload.find(':')?;
    let user_id: i64 = payload[..colon_idx].parse().ok()?;
    let ts: i64 = payload[colon_idx + 1..].parse().ok()?;

    // Check expiration
    let now = chrono::Utc::now().timestamp();
    if now - ts > GUI_COOKIE_MAX_AGE {
        return None;
    }

    // Verify HMAC
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(payload.as_bytes());
    let expected = hex::encode(mac.finalize().into_bytes());

    // Constant-time comparison
    if expected.len() != sig.len() {
        return None;
    }
    if !expected.as_bytes().iter().zip(sig.as_bytes()).all(|(a, b)| a == b) {
        return None;
    }

    Some(user_id)
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

/// Check if a request is authenticated via GUI cookie
/// Returns the user_id if authenticated, None otherwise
pub async fn get_gui_user_id(state: &AppState, headers: &HeaderMap) -> Option<i64> {
    let cookie = get_auth_cookie(headers)?;
    let secret = get_hmac_secret(&state.config.data_dir).await;
    verify_cookie(&cookie, &secret)
}

/// Check if a request is authenticated via GUI cookie (bool convenience wrapper)
pub async fn is_gui_authenticated(state: &AppState, headers: &HeaderMap) -> bool {
    get_gui_user_id(state, headers).await.is_some()
}

/// Determine cookie attributes based on request protocol
fn cookie_attributes(headers: &HeaderMap) -> &'static str {
    let is_https = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(|v| v == "https")
        .unwrap_or(false);

    if is_https {
        "Path=/; HttpOnly; Secure; SameSite=Strict"
    } else {
        "Path=/; HttpOnly; SameSite=Lax"
    }
}

#[derive(serde::Deserialize)]
pub struct LoginForm {
    api_key: String,
}

/// POST /gui/auth - authenticate with API key
async fn gui_auth(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Response {
    // GUI auth requires gui_password to be set (as a feature flag)
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

    // Generate signed cookie with user_id
    let ts = chrono::Utc::now().timestamp();
    let secret = get_hmac_secret(&state.config.data_dir).await;
    let cookie_value = sign_cookie(auth_ctx.user_id, ts, &secret);

    let attrs = cookie_attributes(&headers);
    let cookie = format!(
        "{}={}; Max-Age={}; {}",
        COOKIE_NAME, cookie_value, GUI_COOKIE_MAX_AGE, attrs
    );

    Response::builder()
        .status(StatusCode::OK)
        .header(header::SET_COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(format!(r#"{{"ok":true,"user_id":{}}}"#, auth_ctx.user_id)))
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

    for path in candidates {
        if path.join("index.html").exists() {
            return Some(path);
        }
    }

    None
}

/// Serve static file from GUI build
async fn serve_static_file(build_dir: &PathBuf, file_path: &str) -> Option<Response> {
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
    let ext = canonical
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let mime = mime_for_extension(ext);

    // Cache headers: immutable for _app/immutable/, no-cache for everything else
    let cache = if file_path.contains("/immutable/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };

    Some(
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime)
            .header(header::CACHE_CONTROL, cache)
            .body(Body::from(content))
            .unwrap()
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
async fn serve_app_assets(
    State(state): State<AppState>,
    Path(path): Path<String>,
) -> Response {
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
    CACHED_BUILD_DIR.get_or_init(|| resolve_gui_build_dir(state)).clone()
}

/// Middleware that intercepts SPA routes when Accept: text/html is present.
/// Must be applied BEFORE the main router so it can intercept before API handlers.
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

    // Check GUI auth if password is configured
    let gui_auth_required = state.config.gui_password.is_some();
    if gui_auth_required {
        let headers = request.headers();
        if !is_gui_authenticated(&state, headers).await {
            return Html(login_html()).into_response();
        }
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

#[derive(Deserialize)]
struct CreateMemoryBody {
    content: String,
    category: Option<String>,
    importance: Option<i32>,
    tags: Option<Vec<String>>,
    is_static: Option<bool>,
}

async fn gui_create_memory(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateMemoryBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    // Get authenticated user_id from GUI session
    let user_id = get_gui_user_id(&state, &headers).await
        .ok_or_else(|| AppError::from(engram_lib::EngError::Auth("GUI auth required".into())))?;

    let content = body.content.trim();
    if content.is_empty() {
        return Err(AppError::from(engram_lib::EngError::InvalidInput("content is required".into())));
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
    ).await?;

    Ok((StatusCode::CREATED, Json(json!({ "created": true, "id": result.id }))))
}

#[derive(Deserialize)]
struct UpdateMemoryBody {
    content: Option<String>,
    category: Option<String>,
    importance: Option<i32>,
    is_static: Option<bool>,
}

async fn gui_update_memory(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(body): Json<UpdateMemoryBody>,
) -> Result<Json<Value>, AppError> {
    let user_id = get_gui_user_id(&state, &headers).await
        .ok_or_else(|| AppError::from(engram_lib::EngError::Auth("GUI auth required".into())))?;

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
    let user_id = get_gui_user_id(&state, &headers).await
        .ok_or_else(|| AppError::from(engram_lib::EngError::Auth("GUI auth required".into())))?;

    memory::delete(&state.db, id, user_id).await?;
    Ok(Json(json!({ "deleted": true, "id": id })))
}

#[derive(Deserialize)]
struct BulkArchiveBody {
    ids: Vec<i64>,
}

async fn gui_bulk_archive(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<BulkArchiveBody>,
) -> Result<Json<Value>, AppError> {
    let user_id = get_gui_user_id(&state, &headers).await
        .ok_or_else(|| AppError::from(engram_lib::EngError::Auth("GUI auth required".into())))?;

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
        .route("/gui/memories/{id}", patch(gui_update_memory).delete(gui_delete_memory))
        .route("/gui/memories/bulk-archive", post(gui_bulk_archive))
}
