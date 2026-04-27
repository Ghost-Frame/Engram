//! Bootstrap-bearer resolver for kleos-lib clients.
//!
//! Talks to credd's `/bootstrap/kleos-bearer?agent=<slot>` endpoint to fetch
//! the per-agent Kleos bearer at process startup, without requiring any
//! plaintext key on disk.
//!
//! Resolution order:
//!
//! 1. `KLEOS_API_KEY` / `ENGRAM_API_KEY` env vars (test/debug overrides).
//! 2. credd via `CREDD_SOCKET` (Unix domain) or `CREDD_BIND` (TCP, default
//!    `127.0.0.1:4400`). Auth is the value of `CREDD_AGENT_KEY` (a scoped
//!    bootstrap-agent token).
//!
//! Results are cached in process memory keyed by agent slot; the cache
//! honors the `expires_at` field returned by credd so a leaked bearer
//! goes stale on its own TTL.

use std::collections::HashMap;
use std::env;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use thiserror::Error;

/// Errors produced by [`resolve_api_key`].
#[derive(Debug, Error)]
pub enum CredError {
    /// `CREDD_AGENT_KEY` env var is missing; cannot authenticate to credd.
    #[error("CREDD_AGENT_KEY is not set; cannot authenticate to credd")]
    NoAgentKey,

    /// credd is unreachable (socket not found, connection refused, etc.).
    #[error("credd unreachable: {0}")]
    Unreachable(String),

    /// credd returned a response that could not be parsed.
    #[error("bad response from credd: {0}")]
    BadResponse(String),

    /// credd response did not include a `key` field.
    #[error("credd response is missing the 'key' field")]
    MissingKey,
}

/// Cached entry: the resolved bearer plus when it goes stale.
#[derive(Clone)]
struct CacheEntry {
    key: String,
    expires_at: SystemTime,
}

// Process-lifetime cache: slot -> (key, expires_at). A miss or expired hit
// triggers a fresh fetch from credd.
static KEY_CACHE: Mutex<Option<HashMap<String, CacheEntry>>> = Mutex::new(None);

fn cache_get(slot: &str) -> Option<String> {
    let guard = KEY_CACHE.lock().unwrap();
    let entry = guard.as_ref()?.get(slot)?.clone();
    if SystemTime::now() >= entry.expires_at {
        return None;
    }
    Some(entry.key)
}

fn cache_set(slot: String, key: String, expires_at: SystemTime) {
    let mut guard = KEY_CACHE.lock().unwrap();
    guard.get_or_insert_with(HashMap::new).insert(
        slot,
        CacheEntry { key, expires_at },
    );
}

/// Returns the agent slot string to use for this process.
///
/// `KLEOS_AGENT_SLOT` env wins. Falls back to `claude-code-<hostname>`
/// where hostname comes from `/proc/sys/kernel/hostname` or `HOSTNAME`.
pub fn current_agent_slot() -> String {
    if let Ok(slot) = env::var("KLEOS_AGENT_SLOT") {
        if !slot.is_empty() {
            return slot;
        }
    }
    let hostname = read_hostname();
    format!("claude-code-{}", hostname)
}

fn read_hostname() -> String {
    if let Ok(h) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
        let trimmed = h.trim().to_string();
        if !trimmed.is_empty() {
            return trimmed;
        }
    }
    if let Ok(h) = env::var("HOSTNAME") {
        if !h.is_empty() {
            return h;
        }
    }
    "wsl".to_string()
}

/// Resolve the Kleos API key for `agent_slot`. See module docs for order.
pub async fn resolve_api_key(agent_slot: &str) -> Result<String, CredError> {
    // Env overrides (test/debug).
    if let Ok(k) = env::var("KLEOS_API_KEY") {
        if !k.is_empty() {
            return Ok(k);
        }
    }
    if let Ok(k) = env::var("ENGRAM_API_KEY") {
        if !k.is_empty() {
            return Ok(k);
        }
    }

    if let Some(cached) = cache_get(agent_slot) {
        return Ok(cached);
    }

    // Prefer ECDH if PIV is set up on this host (server 9D pubkey is on
    // disk, client 9A signing works). Falls back silently to the legacy
    // token path if PIV is not configured.
    if piv_pubkey_path().exists() {
        match ecdh::resolve_via_ecdh(agent_slot).await {
            Ok((key, expires_at)) => {
                cache_set(agent_slot.to_string(), key.clone(), expires_at);
                return Ok(key);
            }
            Err(ecdh::EcdhClientError::NotConfigured) => {
                // Pubkey path does not actually exist or unparseable; fall
                // through to token path.
            }
            Err(e) => {
                // PIV is configured but bootstrap failed (sig, ECDH,
                // decrypt, etc). Surface the error rather than silently
                // falling back: a failure here is meaningful.
                return Err(CredError::BadResponse(format!("ECDH bootstrap failed: {}", e)));
            }
        }
    }

    let token = env::var("CREDD_AGENT_KEY").map_err(|_| CredError::NoAgentKey)?;
    if token.is_empty() {
        return Err(CredError::NoAgentKey);
    }

    let path = format!("/bootstrap/kleos-bearer?agent={}", agent_slot);

    let body: serde_json::Value = if let Ok(sock) = env::var("CREDD_SOCKET") {
        unix_get_json(&sock, &path, &token).await?
    } else {
        let bind = env::var("CREDD_BIND").unwrap_or_else(|_| "127.0.0.1:4400".into());
        tcp_get_json(&bind, &path, &token).await?
    };

    let key = body["key"]
        .as_str()
        .ok_or(CredError::MissingKey)?
        .to_string();

    let expires_at = parse_expires_at(&body).unwrap_or_else(|| {
        // No TTL hint -> default 1h from now.
        SystemTime::now() + Duration::from_secs(3600)
    });

    cache_set(agent_slot.to_string(), key.clone(), expires_at);
    Ok(key)
}

/// Path to the cached server PIV slot 9D public key.
/// Mirrors kleos_cred::piv::pubkey_path(KeyManagement) without the dep.
fn piv_pubkey_path() -> std::path::PathBuf {
    let base = env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            env::var("HOME")
                .map(|h| std::path::PathBuf::from(h).join(".config"))
                .unwrap_or_else(|_| std::path::PathBuf::from("."))
        });
    base.join("cred").join("piv-9d-pubkey.pem")
}

/// Parse `expires_at` (RFC 3339) or fall back to `ttl_secs`.
fn parse_expires_at(body: &serde_json::Value) -> Option<SystemTime> {
    if let Some(s) = body.get("expires_at").and_then(|v| v.as_str()) {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
            return Some(SystemTime::UNIX_EPOCH + Duration::from_secs(dt.timestamp() as u64));
        }
    }
    if let Some(secs) = body.get("ttl_secs").and_then(|v| v.as_u64()) {
        return Some(SystemTime::now() + Duration::from_secs(secs));
    }
    None
}

/// Raw HTTP/1.1 GET over a Unix socket.
#[cfg(unix)]
async fn unix_get_json(
    sock_path: &str,
    path: &str,
    token: &str,
) -> Result<serde_json::Value, CredError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixStream;

    let mut stream = UnixStream::connect(sock_path)
        .await
        .map_err(|e| CredError::Unreachable(format!("unix socket {}: {}", sock_path, e)))?;

    let request = format!(
        "GET {} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
        path, token
    );

    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| CredError::Unreachable(format!("write: {}", e)))?;

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .map_err(|e| CredError::Unreachable(format!("read: {}", e)))?;

    parse_http_response_body(&response)
}

#[cfg(not(unix))]
async fn unix_get_json(
    sock_path: &str,
    _path: &str,
    _token: &str,
) -> Result<serde_json::Value, CredError> {
    Err(CredError::Unreachable(format!(
        "Unix sockets not supported on this platform ({})",
        sock_path
    )))
}

/// Raw HTTP/1.1 GET over a TCP stream.
async fn tcp_get_json(bind: &str, path: &str, token: &str) -> Result<serde_json::Value, CredError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    let mut stream = TcpStream::connect(bind)
        .await
        .map_err(|e| CredError::Unreachable(format!("tcp {}: {}", bind, e)))?;

    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
        path, bind, token
    );

    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| CredError::Unreachable(format!("write: {}", e)))?;

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .map_err(|e| CredError::Unreachable(format!("read: {}", e)))?;

    parse_http_response_body(&response)
}

/// Split raw HTTP/1.1 response bytes, parse body as JSON.
fn parse_http_response_body(response: &[u8]) -> Result<serde_json::Value, CredError> {
    let sep = b"\r\n\r\n";
    let body_start = response
        .windows(sep.len())
        .position(|w| w == sep)
        .map(|p| p + sep.len())
        .ok_or_else(|| CredError::BadResponse("no header/body separator".into()))?;

    let body = &response[body_start..];

    if let Some(status_line) = response
        .split(|&b| b == b'\n')
        .next()
        .and_then(|l| std::str::from_utf8(l).ok())
    {
        let code: Option<u16> = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok());
        if let Some(code) = code {
            if code != 200 {
                let body_str = std::str::from_utf8(body).unwrap_or("(non-utf8 body)");
                return Err(CredError::BadResponse(format!(
                    "HTTP {}: {}",
                    code,
                    body_str.trim()
                )));
            }
        }
    }

    serde_json::from_slice(body).map_err(|e| CredError::BadResponse(format!("JSON parse: {}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_GUARD: Mutex<()> = Mutex::new(());

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn env_override_kleos_api_key() {
        let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
        env::remove_var("ENGRAM_API_KEY");
        env::set_var("KLEOS_API_KEY", "test-key-12345");
        let result = resolve_api_key("test-slot-env-1").await;
        env::remove_var("KLEOS_API_KEY");
        assert_eq!(result.unwrap(), "test-key-12345");
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn no_env_no_credd_returns_error() {
        let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
        env::remove_var("KLEOS_API_KEY");
        env::remove_var("ENGRAM_API_KEY");
        env::remove_var("CREDD_AGENT_KEY");
        env::remove_var("CREDD_SOCKET");
        env::remove_var("CREDD_BIND");
        let result = resolve_api_key("no-credd-slot-unique-xyz").await;
        assert!(matches!(result, Err(CredError::NoAgentKey)));
    }

    #[test]
    fn current_agent_slot_uses_env() {
        let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
        env::set_var("KLEOS_AGENT_SLOT", "my-custom-slot");
        let slot = current_agent_slot();
        env::remove_var("KLEOS_AGENT_SLOT");
        assert_eq!(slot, "my-custom-slot");
    }

    #[test]
    fn parse_expires_at_rfc3339() {
        let body = serde_json::json!({"expires_at": "2030-01-01T00:00:00Z"});
        let t = parse_expires_at(&body).expect("should parse");
        let now = SystemTime::now();
        assert!(t > now, "year 2030 should be in the future");
    }

    #[test]
    fn parse_expires_at_ttl_fallback() {
        let body = serde_json::json!({"ttl_secs": 60});
        let t = parse_expires_at(&body).expect("should parse");
        let in_30s = SystemTime::now() + Duration::from_secs(30);
        let in_2m = SystemTime::now() + Duration::from_secs(120);
        assert!(t > in_30s && t < in_2m, "ttl 60s puts expiry inside 30s..2m");
    }
}

// ---------------------------------------------------------------------------
// ECDH client (Stage 3 of ECDH PIV port)
// ---------------------------------------------------------------------------

mod ecdh {
    use std::env;
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::time::{Duration, SystemTime};

    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Key, Nonce};
    use hkdf::Hkdf;
    use p256::ecdh::EphemeralSecret;
    use p256::elliptic_curve::rand_core::OsRng;
    use p256::pkcs8::{DecodePublicKey, EncodePublicKey};
    use p256::PublicKey;
    use sha2::Sha256;
    use thiserror::Error;

    use super::{parse_expires_at, piv_pubkey_path};

    const ECDH_PROTOCOL: &str = "ecdh-v1";
    const ECDH_HKDF_SALT: &[u8] = b"credd-ecdh-v1";

    #[derive(Debug, Error)]
    pub enum EcdhClientError {
        #[error("ECDH not configured (server pubkey absent or unparseable)")]
        NotConfigured,
        #[error("PIV signing failed: {0}")]
        Sign(String),
        #[error("credd unreachable: {0}")]
        Unreachable(String),
        #[error("bad response: {0}")]
        BadResponse(String),
        #[error("decrypt failed: {0}")]
        Decrypt(String),
    }

    /// Run the ECDH bootstrap flow against credd. Returns the decrypted
    /// per-agent bearer plus its expires_at hint.
    pub async fn resolve_via_ecdh(
        agent_slot: &str,
    ) -> Result<(String, SystemTime), EcdhClientError> {
        // Load server's 9D public key.
        let pem = std::fs::read_to_string(piv_pubkey_path())
            .map_err(|_| EcdhClientError::NotConfigured)?;
        let server_9d =
            PublicKey::from_public_key_pem(&pem).map_err(|_| EcdhClientError::NotConfigured)?;

        // Generate ephemeral keypair, compute the shared secret in software.
        let eph = EphemeralSecret::random(&mut OsRng);
        let eph_pub = eph.public_key();
        let eph_pub_der = eph_pub
            .to_public_key_der()
            .map_err(|e| EcdhClientError::Sign(format!("encode eph pubkey: {}", e)))?;
        let eph_pub_hex = hex::encode(eph_pub_der.as_bytes());
        let shared = eph.diffie_hellman(&server_9d);
        let shared_bytes = shared.raw_secret_bytes();

        // Sign agent || ephemeral_pubkey_hex with PIV slot 9A.
        let signed_payload = format!("{}|{}", agent_slot, eph_pub_hex);
        let sig_der = piv_sign_9a(signed_payload.as_bytes())?;

        // The credd handler expects a raw r||s signature (Signature::from_slice
        // for P-256). Convert from DER if the YubiKey returned DER.
        let sig_raw = der_to_raw_p256_sig(&sig_der)?;

        // POST the request to credd.
        let body = serde_json::json!({
            "agent": agent_slot,
            "ephemeral_pubkey": eph_pub_hex,
            "signature": hex::encode(&sig_raw),
            "protocol": ECDH_PROTOCOL,
        })
        .to_string();

        let response = if let Ok(sock) = env::var("CREDD_SOCKET") {
            unix_post(&sock, "/bootstrap/kleos-bearer", &body).await?
        } else {
            let bind = env::var("CREDD_BIND").unwrap_or_else(|_| "127.0.0.1:4400".into());
            tcp_post(&bind, "/bootstrap/kleos-bearer", &body).await?
        };

        // Decrypt with the same HKDF / AES-GCM derivation as credd used.
        let encrypted_hex = response["encrypted_bearer"]
            .as_str()
            .ok_or_else(|| EcdhClientError::BadResponse("missing encrypted_bearer".into()))?;
        let nonce_hex = response["nonce"]
            .as_str()
            .ok_or_else(|| EcdhClientError::BadResponse("missing nonce".into()))?;
        let ciphertext = hex::decode(encrypted_hex)
            .map_err(|e| EcdhClientError::BadResponse(format!("ciphertext hex: {}", e)))?;
        let nonce_bytes = hex::decode(nonce_hex)
            .map_err(|e| EcdhClientError::BadResponse(format!("nonce hex: {}", e)))?;
        if nonce_bytes.len() != 12 {
            return Err(EcdhClientError::BadResponse(format!(
                "nonce wrong length: {}",
                nonce_bytes.len()
            )));
        }

        let hk = Hkdf::<Sha256>::new(Some(ECDH_HKDF_SALT), shared_bytes.as_slice());
        let mut bearer_key = [0u8; 32];
        hk.expand(agent_slot.as_bytes(), &mut bearer_key)
            .map_err(|e| EcdhClientError::Decrypt(format!("hkdf expand: {}", e)))?;

        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&bearer_key));
        let plaintext = cipher
            .decrypt(Nonce::from_slice(&nonce_bytes), ciphertext.as_ref())
            .map_err(|e| EcdhClientError::Decrypt(format!("aes-gcm: {}", e)))?;
        let bearer = String::from_utf8(plaintext)
            .map_err(|e| EcdhClientError::Decrypt(format!("utf8: {}", e)))?;

        let expires_at = parse_expires_at(&response).unwrap_or_else(|| {
            SystemTime::now() + Duration::from_secs(3600)
        });

        Ok((bearer, expires_at))
    }

    /// Convert a DER-encoded P-256 ECDSA signature to raw r||s (64 bytes).
    /// The YubiKey returns DER; the server's p256::ecdsa::Signature::from_slice
    /// expects raw bytes.
    fn der_to_raw_p256_sig(der: &[u8]) -> Result<Vec<u8>, EcdhClientError> {
        use p256::ecdsa::Signature;
        let sig = Signature::from_der(der)
            .map_err(|e| EcdhClientError::Sign(format!("decode DER sig: {}", e)))?;
        Ok(sig.to_bytes().to_vec())
    }

    /// PIV slot 9A ECDSA-SHA256 sign, via Python yubikit subprocess.
    /// Same pattern as kleos_cred::piv::piv_sign but local to avoid a
    /// dependency cycle (kleos-cred already depends on kleos-lib).
    fn piv_sign_9a(payload: &[u8]) -> Result<Vec<u8>, EcdhClientError> {
        let payload_hex = hex::encode(payload);
        // NOTE: yubikit's PivSession.sign(message, hash_algorithm=SHA256())
        // hashes the message INTERNALLY when hash_algorithm is set. Pre-hashing
        // and passing the digest causes a double-hash and verification failure
        // on the server. Pass the raw payload bytes.
        let script = format!(
            r#"
import sys, base64
from ykman.device import list_all_devices
from yubikit.piv import PivSession, SLOT, KEY_TYPE
from yubikit.core.smartcard import SmartCardConnection
from cryptography.hazmat.primitives import hashes

payload = bytes.fromhex("{payload}")

devices = list_all_devices()
if not devices:
    print("no yubikey detected", file=sys.stderr); sys.exit(2)

dev, _info = devices[0]
with dev.open_connection(SmartCardConnection) as conn:
    session = PivSession(conn)
    sig = session.sign(SLOT.AUTHENTICATION, KEY_TYPE.ECCP256, payload, hash_algorithm=hashes.SHA256())
    sys.stdout.write(base64.b16encode(sig).decode().lower())
"#,
            payload = payload_hex,
        );

        let out = Command::new("python3")
            .args(["-c", &script])
            .output()
            .map_err(|e| EcdhClientError::Sign(format!("python3 spawn: {}", e)))?;

        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(EcdhClientError::Sign(format!(
                "PIV 9A sign: {}",
                stderr.trim()
            )));
        }

        let hex_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
        hex::decode(&hex_str).map_err(|e| EcdhClientError::Sign(format!("sig hex: {}", e)))
    }

    /// HTTP/1.1 POST over Unix socket. Returns parsed JSON response body.
    #[cfg(unix)]
    async fn unix_post(
        sock_path: &str,
        path: &str,
        body: &str,
    ) -> Result<serde_json::Value, EcdhClientError> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::UnixStream;

        let mut stream = UnixStream::connect(sock_path)
            .await
            .map_err(|e| EcdhClientError::Unreachable(format!("unix {}: {}", sock_path, e)))?;

        let request = format!(
            "POST {} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            path,
            body.len(),
            body
        );

        stream
            .write_all(request.as_bytes())
            .await
            .map_err(|e| EcdhClientError::Unreachable(format!("write: {}", e)))?;

        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .await
            .map_err(|e| EcdhClientError::Unreachable(format!("read: {}", e)))?;

        parse_post_body(&response)
    }

    #[cfg(not(unix))]
    async fn unix_post(
        sock_path: &str,
        _path: &str,
        _body: &str,
    ) -> Result<serde_json::Value, EcdhClientError> {
        Err(EcdhClientError::Unreachable(format!(
            "Unix sockets not supported on this platform ({})",
            sock_path
        )))
    }

    async fn tcp_post(
        bind: &str,
        path: &str,
        body: &str,
    ) -> Result<serde_json::Value, EcdhClientError> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpStream;

        let mut stream = TcpStream::connect(bind)
            .await
            .map_err(|e| EcdhClientError::Unreachable(format!("tcp {}: {}", bind, e)))?;

        let request = format!(
            "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            path,
            bind,
            body.len(),
            body
        );

        stream
            .write_all(request.as_bytes())
            .await
            .map_err(|e| EcdhClientError::Unreachable(format!("write: {}", e)))?;

        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .await
            .map_err(|e| EcdhClientError::Unreachable(format!("read: {}", e)))?;

        parse_post_body(&response)
    }

    fn parse_post_body(response: &[u8]) -> Result<serde_json::Value, EcdhClientError> {
        let sep = b"\r\n\r\n";
        let body_start = response
            .windows(sep.len())
            .position(|w| w == sep)
            .map(|p| p + sep.len())
            .ok_or_else(|| EcdhClientError::BadResponse("no header/body separator".into()))?;
        let body = &response[body_start..];

        if let Some(status_line) = response
            .split(|&b| b == b'\n')
            .next()
            .and_then(|l| std::str::from_utf8(l).ok())
        {
            if let Some(code) = status_line
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse::<u16>().ok())
            {
                if code != 200 {
                    let body_str = std::str::from_utf8(body).unwrap_or("(non-utf8 body)");
                    return Err(EcdhClientError::BadResponse(format!(
                        "HTTP {}: {}",
                        code,
                        body_str.trim()
                    )));
                }
            }
        }

        serde_json::from_slice(body)
            .map_err(|e| EcdhClientError::BadResponse(format!("JSON parse: {}", e)))
    }
}
