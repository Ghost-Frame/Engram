use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use p256::ecdsa::signature::Verifier;
use sha2::{Digest, Sha256};

use crate::{EngError, Result};

type HmacSha256 = Hmac<Sha256>;

// ---------------------------------------------------------------------------
// Signature algorithm enum
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureAlgo {
    EcdsaP256,
    Ed25519,
}

impl SignatureAlgo {
    pub fn from_header(s: &str) -> Result<Self> {
        match s {
            "ecdsa-p256" => Ok(Self::EcdsaP256),
            "ed25519" => Ok(Self::Ed25519),
            _ => Err(EngError::InvalidInput(format!(
                "unsupported signature algorithm: {s}"
            ))),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::EcdsaP256 => "ecdsa-p256",
            Self::Ed25519 => "ed25519",
        }
    }
}

// ---------------------------------------------------------------------------
// Auth tier
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthTier {
    Piv,
    Soft,
    Session,
    Bearer,
}

impl AuthTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Piv => "piv",
            Self::Soft => "soft",
            Self::Session => "session",
            Self::Bearer => "bearer",
        }
    }
}

// ---------------------------------------------------------------------------
// Canonical envelope
// ---------------------------------------------------------------------------

pub struct CanonicalEnvelope {
    method: String,
    path: String,
    query: String,
    body_hash: String,
    timestamp_ms: u64,
    nonce: String,
    identity_hash: String,
}

impl CanonicalEnvelope {
    pub fn new(
        method: &str,
        path: &str,
        query: &str,
        body: &[u8],
        timestamp_ms: u64,
        nonce_hex: &str,
        identity_hash_hex: &str,
    ) -> Self {
        let body_hash = hex::encode(Sha256::digest(body));
        let mut sorted_query = query
            .split('&')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        sorted_query.sort();
        Self {
            method: method.to_ascii_uppercase(),
            path: path.to_string(),
            query: sorted_query.join("&"),
            body_hash,
            timestamp_ms,
            nonce: nonce_hex.to_string(),
            identity_hash: identity_hash_hex.to_string(),
        }
    }

    pub fn build(&self) -> Vec<u8> {
        format!(
            "KLEOSv1\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            self.method,
            self.path,
            self.query,
            self.body_hash,
            self.timestamp_ms,
            self.nonce,
            self.identity_hash,
        )
        .into_bytes()
    }

    pub fn timestamp_ms(&self) -> u64 {
        self.timestamp_ms
    }

    pub fn nonce_hex(&self) -> &str {
        &self.nonce
    }

    pub fn identity_hash_hex(&self) -> &str {
        &self.identity_hash
    }
}

// ---------------------------------------------------------------------------
// Signature verification
// ---------------------------------------------------------------------------

pub fn verify_signature(
    algo: SignatureAlgo,
    pubkey_pem: &str,
    envelope_bytes: &[u8],
    sig_hex: &str,
) -> Result<()> {
    let sig_bytes =
        hex::decode(sig_hex).map_err(|e| EngError::InvalidInput(format!("bad sig hex: {e}")))?;

    match algo {
        SignatureAlgo::EcdsaP256 => verify_p256(pubkey_pem, envelope_bytes, &sig_bytes),
        SignatureAlgo::Ed25519 => verify_ed25519(pubkey_pem, envelope_bytes, &sig_bytes),
    }
}

fn verify_p256(pubkey_pem: &str, message: &[u8], sig_bytes: &[u8]) -> Result<()> {
    use p256::ecdsa::{Signature, VerifyingKey};
    use p256::pkcs8::DecodePublicKey;

    let vk = VerifyingKey::from_public_key_pem(pubkey_pem)
        .map_err(|e| EngError::InvalidInput(format!("bad P256 pubkey PEM: {e}")))?;

    let sig = Signature::from_bytes(sig_bytes.into())
        .map_err(|e| EngError::InvalidInput(format!("bad P256 signature: {e}")))?;

    // VerifyingKey::verify hashes the message with SHA-256 internally
    vk.verify(message, &sig)
        .map_err(|_| EngError::Auth("P256 signature verification failed".into()))
}

fn verify_ed25519(pubkey_pem: &str, message: &[u8], sig_bytes: &[u8]) -> Result<()> {
    use ed25519_dalek::{Signature, VerifyingKey};

    let pubkey_der = pem_to_ed25519_pubkey(pubkey_pem)?;

    let vk = VerifyingKey::from_bytes(&pubkey_der)
        .map_err(|e| EngError::InvalidInput(format!("bad Ed25519 pubkey: {e}")))?;

    let sig = Signature::from_bytes(
        sig_bytes
            .try_into()
            .map_err(|_| EngError::InvalidInput("Ed25519 signature must be 64 bytes".into()))?,
    );

    vk.verify_strict(message, &sig)
        .map_err(|_| EngError::Auth("Ed25519 signature verification failed".into()))
}

fn pem_to_ed25519_pubkey(pem: &str) -> Result<[u8; 32]> {
    // Ed25519 SubjectPublicKeyInfo DER has a fixed 12-byte prefix before the
    // 32-byte key. The OID is 1.3.101.112 (id-EdDSA / Ed25519).
    const ED25519_SPKI_PREFIX: [u8; 12] = [
        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
    ];

    let der = decode_pem_der(pem, "PUBLIC KEY")?;
    if der.len() != 44 || !der.starts_with(&ED25519_SPKI_PREFIX) {
        return Err(EngError::InvalidInput(
            "PEM does not contain a valid Ed25519 SubjectPublicKeyInfo".into(),
        ));
    }
    // M8: avoid raw [12..] indexing, which would panic if der.len() ever
    // changed without the length check above. Using checked slicing keeps
    // the function panic-free even if a future ED25519_SPKI_PREFIX edit
    // skips the length update.
    let key_slice = der.get(12..44).ok_or_else(|| {
        EngError::InvalidInput("Ed25519 SPKI bytes shorter than required after prefix".into())
    })?;
    let mut key = [0u8; 32];
    key.copy_from_slice(key_slice);
    Ok(key)
}

fn decode_pem_der(pem: &str, expected_label: &str) -> Result<Vec<u8>> {
    let begin = format!("-----BEGIN {expected_label}-----");
    let end = format!("-----END {expected_label}-----");
    let b64: String = pem
        .lines()
        .skip_while(|l| !l.starts_with(&begin))
        .skip(1)
        .take_while(|l| !l.starts_with(&end))
        .collect::<Vec<_>>()
        .join("");
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(&b64)
        .map_err(|e| EngError::InvalidInput(format!("PEM base64 decode failed: {e}")))
}

// ---------------------------------------------------------------------------
// HKDF identity derivation
// ---------------------------------------------------------------------------

pub fn derive_identity_hash(pubkey_der: &[u8], host: &str, agent: &str, model: &str) -> [u8; 16] {
    use hkdf::Hkdf;
    let hk = Hkdf::<Sha256>::new(Some(b"kleos-identity-v1"), pubkey_der);
    let info = format!("{host}|{agent}|{model}");
    let mut out = [0u8; 16];
    hk.expand(info.as_bytes(), &mut out)
        .expect("16 bytes is a valid HKDF-SHA256 output length");
    out
}

pub fn identity_hash_hex(pubkey_der: &[u8], host: &str, agent: &str, model: &str) -> String {
    hex::encode(derive_identity_hash(pubkey_der, host, agent, model))
}

// ---------------------------------------------------------------------------
// Replay guard
// ---------------------------------------------------------------------------

const REPLAY_WINDOW_MS: u64 = 60_000;
const NONCE_TTL: Duration = Duration::from_secs(90);
const GC_INTERVAL: Duration = Duration::from_secs(30);

struct NonceEntry {
    inserted: Instant,
}

pub struct ReplayGuard {
    nonces: Arc<Mutex<HashMap<(String, String), NonceEntry>>>,
    last_gc: Arc<Mutex<Instant>>,
}

impl ReplayGuard {
    pub fn new() -> Self {
        Self {
            nonces: Arc::new(Mutex::new(HashMap::new())),
            last_gc: Arc::new(Mutex::new(Instant::now())),
        }
    }

    pub fn check(&self, identity_hash_hex: &str, nonce_hex: &str, ts_ms: u64) -> Result<()> {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let drift = now_ms.abs_diff(ts_ms);
        if drift > REPLAY_WINDOW_MS {
            return Err(EngError::Auth(format!(
                "request timestamp outside {REPLAY_WINDOW_MS}ms window (drift={drift}ms)"
            )));
        }

        let key = (identity_hash_hex.to_string(), nonce_hex.to_string());
        let mut nonces = self
            .nonces
            .lock()
            .map_err(|_| EngError::Auth("replay guard mutex poisoned".into()))?;

        self.maybe_gc(&mut nonces)?;

        if nonces.contains_key(&key) {
            return Err(EngError::Auth("duplicate nonce (replay)".into()));
        }
        nonces.insert(
            key,
            NonceEntry {
                inserted: Instant::now(),
            },
        );
        Ok(())
    }

    fn maybe_gc(&self, nonces: &mut HashMap<(String, String), NonceEntry>) -> Result<()> {
        let mut last = self
            .last_gc
            .lock()
            .map_err(|_| EngError::Auth("replay guard mutex poisoned".into()))?;
        if last.elapsed() < GC_INTERVAL {
            return Ok(());
        }
        *last = Instant::now();
        nonces.retain(|_, v| v.inserted.elapsed() < NONCE_TTL);
        Ok(())
    }
}

impl Default for ReplayGuard {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Session tokens
// ---------------------------------------------------------------------------

/// Default sliding-window TTL: each verified session call extends the token
/// expiry by this much. Overridable via KLEOS_SESSION_TTL_SECS.
const DEFAULT_SESSION_TTL_SECS: u64 = 900; // 15 minutes

/// Default absolute lifetime cap from initial mint. Even with continuous
/// activity, a session token must be replaced via fresh PIV signing once it
/// exceeds this age. Overridable via KLEOS_SESSION_MAX_LIFETIME_SECS.
const DEFAULT_SESSION_MAX_LIFETIME_SECS: u64 = 86_400; // 24 hours

pub struct SessionManager {
    key: [u8; 32],
    sessions: Arc<Mutex<HashMap<String, SessionEntry>>>,
    ttl: Duration,
    max_lifetime: Duration,
}

struct SessionEntry {
    identity_id: i64,
    issued_at: Instant,
    expires_at: Instant,
}

impl SessionManager {
    /// Construct a SessionManager with the given HMAC key and default
    /// TTL / max-lifetime. Tests use this directly; production goes through
    /// `from_env_or_generate`.
    pub fn new(key: [u8; 32]) -> Self {
        Self::with_durations(
            key,
            Duration::from_secs(DEFAULT_SESSION_TTL_SECS),
            Duration::from_secs(DEFAULT_SESSION_MAX_LIFETIME_SECS),
        )
    }

    /// Construct a SessionManager with explicit TTL and absolute-lifetime
    /// caps. If `max_lifetime < ttl` the caller's intent is incoherent, so
    /// we floor `max_lifetime` at `ttl` to keep refresh logic monotonic.
    pub fn with_durations(key: [u8; 32], ttl: Duration, max_lifetime: Duration) -> Self {
        let max_lifetime = if max_lifetime < ttl {
            ttl
        } else {
            max_lifetime
        };
        Self {
            key,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            ttl,
            max_lifetime,
        }
    }

    pub fn from_env_or_generate() -> Result<Self> {
        let key = if let Ok(hex_str) = std::env::var("KLEOS_SESSION_KEY") {
            let bytes = hex::decode(hex_str.trim())
                .map_err(|e| EngError::Internal(format!("KLEOS_SESSION_KEY bad hex: {e}")))?;
            if bytes.len() != 32 {
                return Err(EngError::Internal(format!(
                    "KLEOS_SESSION_KEY must be 32 bytes, got {}",
                    bytes.len()
                )));
            }
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            arr
        } else {
            let mut key = [0u8; 32];
            use rand::Rng;
            rand::rng().fill(&mut key);
            tracing::warn!("KLEOS_SESSION_KEY not set, generated ephemeral key (sessions will not survive restart)");
            key
        };

        let ttl = parse_positive_secs_env("KLEOS_SESSION_TTL_SECS", DEFAULT_SESSION_TTL_SECS);
        let max_lifetime = parse_positive_secs_env(
            "KLEOS_SESSION_MAX_LIFETIME_SECS",
            DEFAULT_SESSION_MAX_LIFETIME_SECS,
        );
        if max_lifetime < ttl {
            tracing::warn!(
                ttl_secs = ttl.as_secs(),
                max_lifetime_secs = max_lifetime.as_secs(),
                "KLEOS_SESSION_MAX_LIFETIME_SECS is below KLEOS_SESSION_TTL_SECS; clamping max to ttl",
            );
        }

        Ok(Self::with_durations(key, ttl, max_lifetime))
    }

    /// Generate a fresh session token bound to `identity_id` with a new
    /// sliding window. Records `issued_at` for the absolute-lifetime cap
    /// enforced by `refresh`.
    pub fn mint(&self, identity_id: i64) -> String {
        let now = Instant::now();
        self.mint_with_issue(identity_id, now, now + self.ttl)
    }

    /// Internal helper: build and store a SessionEntry. Used by `mint` (which
    /// stamps a new `issued_at`) and `refresh` (which inherits the original
    /// `issued_at` so the hard cap is anchored to the first mint).
    fn mint_with_issue(&self, identity_id: i64, issued_at: Instant, expires_at: Instant) -> String {
        let expires_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            + (expires_at.saturating_duration_since(Instant::now())).as_millis() as u64;

        let mut random_8 = [0u8; 8];
        use rand::Rng;
        rand::rng().fill(&mut random_8);

        let mut mac = HmacSha256::new_from_slice(&self.key).unwrap();
        mac.update(&identity_id.to_le_bytes());
        mac.update(&expires_ms.to_le_bytes());
        mac.update(&random_8);
        let tag = mac.finalize().into_bytes();

        use base64::Engine;
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(tag);

        // Recover from poison rather than cascading a panic to all future callers.
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        sessions.insert(
            token.clone(),
            SessionEntry {
                identity_id,
                issued_at,
                expires_at,
            },
        );

        // GC expired sessions opportunistically
        sessions.retain(|_, v| v.expires_at > Instant::now());

        token
    }

    /// Verify a session token and return the identity ID if the token is
    /// known and not past its current `expires_at`. Does not extend the
    /// session; callers wanting sliding-window behavior must also call
    /// `refresh`.
    pub fn verify(&self, token: &str) -> Result<i64> {
        let sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = sessions
            .get(token)
            .ok_or_else(|| EngError::Auth("unknown session token".into()))?;

        if entry.expires_at <= Instant::now() {
            return Err(EngError::Auth("session token expired".into()));
        }

        Ok(entry.identity_id)
    }

    /// Roll an active session forward by minting a replacement token that
    /// inherits the original `issued_at` (anchoring the hard cap) and gets
    /// a fresh `expires_at = now + ttl`.
    ///
    /// The OLD token is intentionally left in the map until its own
    /// `expires_at` fires. This is required for concurrency safety: two
    /// near-simultaneous requests carrying the same cached session token
    /// must both verify successfully, even though only the first triggers
    /// a refresh. Pre-emptively removing the old token would race the
    /// second request into a spurious 401.
    ///
    /// The client picks up the new token from `x-kleos-session-issued`
    /// and uses it on subsequent calls. The old token simply ages out.
    ///
    /// Returns:
    /// - `Ok(new_token)` on successful refresh.
    /// - `Err` if the token is unknown, already expired, or has crossed
    ///   `max_lifetime` since its original mint (forcing fresh PIV re-auth).
    pub fn refresh(&self, token: &str) -> Result<String> {
        let now = Instant::now();
        let (identity_id, issued_at) = {
            let sessions = self
                .sessions
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());

            let entry = sessions
                .get(token)
                .ok_or_else(|| EngError::Auth("unknown session token".into()))?;

            if entry.expires_at <= now {
                return Err(EngError::Auth("session token expired".into()));
            }

            if now.saturating_duration_since(entry.issued_at) >= self.max_lifetime {
                return Err(EngError::Auth(
                    "session token exceeded absolute lifetime cap".into(),
                ));
            }

            (entry.identity_id, entry.issued_at)
        };

        let expires_at = now + self.ttl;
        Ok(self.mint_with_issue(identity_id, issued_at, expires_at))
    }
}

/// Read an env var as positive u64 seconds. Falls back to `default_secs` if
/// the var is missing, malformed, zero, or negative.
fn parse_positive_secs_env(var: &str, default_secs: u64) -> Duration {
    match std::env::var(var) {
        Ok(raw) => match raw.trim().parse::<u64>() {
            Ok(v) if v > 0 => Duration::from_secs(v),
            Ok(_) => {
                tracing::warn!(var, "value must be > 0; using default");
                Duration::from_secs(default_secs)
            }
            Err(e) => {
                tracing::warn!(var, error = %e, "could not parse as u64; using default");
                Duration::from_secs(default_secs)
            }
        },
        Err(_) => Duration::from_secs(default_secs),
    }
}

// ---------------------------------------------------------------------------
// Timestamp check (shared between replay guard and standalone use)
// ---------------------------------------------------------------------------

pub fn check_timestamp(ts_ms: u64) -> Result<()> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    let drift = now_ms.abs_diff(ts_ms);
    if drift > REPLAY_WINDOW_MS {
        return Err(EngError::Auth(format!(
            "request timestamp outside {REPLAY_WINDOW_MS}ms window (drift={drift}ms)"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Nonce generation
// ---------------------------------------------------------------------------

pub fn generate_nonce() -> String {
    let mut buf = [0u8; 12];
    use rand::Rng;
    rand::rng().fill(&mut buf);
    hex::encode(buf)
}

// ---------------------------------------------------------------------------
// Client-side request signing
// ---------------------------------------------------------------------------

enum SigningBackend {
    Ed25519(ed25519_dalek::SigningKey),
    #[cfg(feature = "piv")]
    Piv(Mutex<yubikey::YubiKey>),
}

/// Read the PIV PIN required for runtime signing. Refuses to fall back to
/// the YubiKey factory-default PIN ("123456"); a missing or factory-default
/// PIV_PIN is treated as a hard configuration error so callers never burn
/// PIN retries against a hardened YubiKey. Always available, irrespective
/// of the `piv` cargo feature, because callers include Python-subprocess
/// signing paths that do not link the `yubikey` Rust crate.
pub fn runtime_piv_pin() -> std::result::Result<String, &'static str> {
    match std::env::var("PIV_PIN") {
        Ok(p) if p.is_empty() => Err("PIV_PIN is set but empty"),
        Ok(p) if p == "123456" => {
            Err("PIV_PIN equals the YubiKey factory-default; refusing to use it")
        }
        Ok(p) => Ok(p),
        Err(_) => Err("PIV_PIN environment variable is not set"),
    }
}

/// Verify PIV PIN then sign a SHA-256 digest with slot 9A. Nano 5.7.4+
/// firmware requires PIN before signing. If `PIV_PIN` is unset or equal to
/// the factory default this returns `AuthenticationError` BEFORE touching
/// the YubiKey, so no PIN retries are consumed against a misconfigured env.
#[cfg(feature = "piv")]
fn piv_verify_and_sign(
    yk: &mut yubikey::YubiKey,
    digest: &[u8],
) -> std::result::Result<Vec<u8>, yubikey::Error> {
    let pin = runtime_piv_pin().map_err(|_| yubikey::Error::AuthenticationError)?;
    yk.verify_pin(pin.as_bytes())
        .map_err(|_| yubikey::Error::AuthenticationError)?;
    yubikey::piv::sign_data(
        yk,
        digest,
        yubikey::piv::AlgorithmId::EccP256,
        yubikey::piv::SlotId::Authentication,
    )
    .map(|buf| buf.to_vec())
}

pub struct RequestSigner {
    backend: SigningBackend,
    algo: SignatureAlgo,
    pubkey_pem: String,
    pubkey_der: Vec<u8>,
    fingerprint: String,
    host_label: String,
    agent_label: String,
    model_label: String,
    identity_hash: String,
    session_token: Mutex<Option<String>>,
}

const ED25519_SPKI_PREFIX_CONST: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

impl RequestSigner {
    pub fn from_key_bytes(secret: [u8; 32], host: &str, agent: &str, model: &str) -> Self {
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&secret);
        let vk = signing_key.verifying_key();

        let mut der = Vec::with_capacity(44);
        der.extend_from_slice(&ED25519_SPKI_PREFIX_CONST);
        der.extend_from_slice(vk.as_bytes());

        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&der);
        let pubkey_pem = format!("-----BEGIN PUBLIC KEY-----\n{b64}\n-----END PUBLIC KEY-----");

        let fingerprint = hex::encode(Sha256::digest(&der));
        let identity_hash = identity_hash_hex(&der, host, agent, model);

        Self {
            backend: SigningBackend::Ed25519(signing_key),
            algo: SignatureAlgo::Ed25519,
            pubkey_pem,
            pubkey_der: der,
            fingerprint,
            host_label: host.to_string(),
            agent_label: agent.to_string(),
            model_label: model.to_string(),
            identity_hash,
            session_token: Mutex::new(None),
        }
    }

    #[cfg(feature = "piv")]
    pub fn from_yubikey(host: &str, agent: &str, model: &str) -> Result<Self> {
        use yubikey::piv::SlotId;

        let mut yk = yubikey::YubiKey::open()
            .map_err(|e| EngError::Internal(format!("cannot open YubiKey: {e}")))?;

        let cert = yubikey::certificate::Certificate::read(&mut yk, SlotId::Authentication)
            .map_err(|e| EngError::Internal(format!("cannot read PIV slot 9a certificate: {e}")))?;

        // Extract SPKI DER bytes from the certificate
        let spki = cert.subject_pki();
        let pubkey_der = {
            use p256::pkcs8::der::Encode;
            spki.to_der()
                .map_err(|e| EngError::Internal(format!("cannot encode SPKI to DER: {e}")))?
        };

        let pubkey_pem = {
            use base64::Engine;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&pubkey_der);
            let wrapped: Vec<&str> = b64
                .as_bytes()
                .chunks(64)
                .map(|c| std::str::from_utf8(c).unwrap())
                .collect();
            format!(
                "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----",
                wrapped.join("\n")
            )
        };

        let fingerprint = hex::encode(Sha256::digest(&pubkey_der));
        let identity_hash = identity_hash_hex(&pubkey_der, host, agent, model);

        let serial = yk.serial().to_string();
        tracing::info!(
            serial = %serial,
            fingerprint = %fingerprint,
            "PIV signer initialized from YubiKey"
        );

        Ok(Self {
            backend: SigningBackend::Piv(Mutex::new(yk)),
            algo: SignatureAlgo::EcdsaP256,
            pubkey_pem,
            pubkey_der,
            fingerprint,
            host_label: host.to_string(),
            agent_label: agent.to_string(),
            model_label: model.to_string(),
            identity_hash,
            session_token: Mutex::new(None),
        })
    }

    pub fn from_file(path: &std::path::Path, host: &str, agent: &str, model: &str) -> Result<Self> {
        // The file holds raw private-key material. Every heap buffer that
        // touches the secret (file bytes, decoded DER, hex-decoded bytes) is
        // wrapped in Zeroizing so it is scrubbed when this function returns.
        let raw = zeroize::Zeroizing::new(std::fs::read(path).map_err(|e| {
            EngError::Internal(format!("cannot read identity key {}: {e}", path.display()))
        })?);

        if raw.len() == 32 {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&raw);
            return Ok(Self::from_key_bytes(arr, host, agent, model));
        }

        let text = std::str::from_utf8(&raw).map_err(|_| {
            EngError::InvalidInput("identity key file is not valid UTF-8 or 32-byte raw".into())
        })?;

        if text.contains("PRIVATE KEY") {
            let der = zeroize::Zeroizing::new(decode_pem_der(text, "PRIVATE KEY")?);
            // PKCS8 Ed25519 private key: 16-byte prefix + 34-byte wrapped key
            // The 34 bytes are: 04 20 <32 bytes of private key>
            if der.len() == 48 && der[14] == 0x04 && der[15] == 0x20 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&der[16..48]);
                return Ok(Self::from_key_bytes(arr, host, agent, model));
            }
            return Err(EngError::InvalidInput(
                "unsupported PEM private key format (expected Ed25519 PKCS8)".into(),
            ));
        }

        let decoded = zeroize::Zeroizing::new(hex::decode(text.trim()).map_err(|_| {
            EngError::InvalidInput("identity key file is not 32-byte raw, PEM, or hex".into())
        })?);
        if decoded.len() != 32 {
            return Err(EngError::InvalidInput(format!(
                "hex-encoded key must be 32 bytes, got {}",
                decoded.len()
            )));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&decoded);
        Ok(Self::from_key_bytes(arr, host, agent, model))
    }

    pub fn from_env_or_file(host: &str, agent: &str, model: &str) -> Result<Option<Self>> {
        // T1: Try PIV YubiKey first (highest auth tier)
        #[cfg(feature = "piv")]
        {
            match Self::from_yubikey(host, agent, model) {
                Ok(signer) => return Ok(Some(signer)),
                Err(e) => {
                    tracing::debug!("PIV YubiKey not available, falling back to software key: {e}");
                }
            }
        }

        // T2: Software Ed25519 key from env var or file
        if let Ok(hex_key) = std::env::var("KLEOS_IDENTITY_KEY") {
            let bytes = hex::decode(hex_key.trim())
                .map_err(|e| EngError::InvalidInput(format!("KLEOS_IDENTITY_KEY bad hex: {e}")))?;
            if bytes.len() != 32 {
                return Err(EngError::InvalidInput(format!(
                    "KLEOS_IDENTITY_KEY must be 32 bytes, got {}",
                    bytes.len()
                )));
            }
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            return Ok(Some(Self::from_key_bytes(arr, host, agent, model)));
        }

        let key_path = if let Ok(p) = std::env::var("KLEOS_IDENTITY_KEY_FILE") {
            std::path::PathBuf::from(p)
        } else if let Some(home) = dirs_for_key_path() {
            home.join(".kleos").join("identity.key")
        } else {
            return Ok(None);
        };

        if key_path.exists() {
            Ok(Some(Self::from_file(&key_path, host, agent, model)?))
        } else {
            Ok(None)
        }
    }

    pub fn generate_software_key(
        host: &str,
        agent: &str,
        model: &str,
    ) -> Result<(Self, std::path::PathBuf)> {
        let home = dirs_for_key_path()
            .ok_or_else(|| EngError::Internal("cannot determine home directory".into()))?;
        let kleos_dir = home.join(".kleos");
        std::fs::create_dir_all(&kleos_dir)
            .map_err(|e| EngError::Internal(format!("cannot create ~/.kleos: {e}")))?;
        let key_path = kleos_dir.join("identity.key");
        if key_path.exists() {
            return Err(EngError::InvalidInput(format!(
                "software key already exists at {}; remove it first to regenerate",
                key_path.display()
            )));
        }

        let mut secret = [0u8; 32];
        use rand::Rng;
        rand::rng().fill(&mut secret);

        std::fs::write(&key_path, hex::encode(secret))
            .map_err(|e| EngError::Internal(format!("cannot write key file: {e}")))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| EngError::Internal(format!("cannot chmod key file: {e}")))?;
        }

        let signer = Self::from_key_bytes(secret, host, agent, model);
        Ok((signer, key_path))
    }

    pub fn sign_enrollment_proof(&self) -> Result<String> {
        let proof_msg = format!(
            "KLEOS-ENROLL:{}:{}:{}:{}",
            self.algo.as_str(),
            self.tier(),
            self.host_label,
            self.pubkey_pem,
        );
        match &self.backend {
            SigningBackend::Ed25519(sk) => {
                use ed25519_dalek::Signer;
                Ok(hex::encode(sk.sign(proof_msg.as_bytes()).to_bytes()))
            }
            #[cfg(feature = "piv")]
            SigningBackend::Piv(yk_mutex) => {
                let digest = Sha256::digest(proof_msg.as_bytes());
                let mut yk = yk_mutex.lock().unwrap();
                let result = piv_verify_and_sign(&mut yk, &digest);
                let sig_der = match result {
                    Ok(d) => d,
                    Err(_) => {
                        drop(yk);
                        let mut fresh = yubikey::YubiKey::open().map_err(|e| {
                            EngError::Internal(format!("YubiKey reconnect failed: {e}"))
                        })?;
                        let d = piv_verify_and_sign(&mut fresh, &digest).map_err(|e| {
                            EngError::Internal(format!(
                                "YubiKey PIV signing failed after reconnect: {e}"
                            ))
                        })?;
                        *yk_mutex.lock().unwrap() = fresh;
                        d
                    }
                };
                let sig = p256::ecdsa::Signature::from_der(&sig_der).map_err(|e| {
                    EngError::Internal(format!("invalid ECDSA DER from YubiKey: {e}"))
                })?;
                Ok(hex::encode(sig.to_bytes()))
            }
        }
    }

    #[cfg(feature = "piv")]
    pub fn yubikey_serial(&self) -> Option<String> {
        match &self.backend {
            SigningBackend::Piv(yk_mutex) => {
                let yk = yk_mutex.lock().unwrap();
                Some(yk.serial().to_string())
            }
            _ => None,
        }
    }

    pub fn algo(&self) -> SignatureAlgo {
        self.algo
    }

    pub fn tier(&self) -> &'static str {
        match &self.backend {
            SigningBackend::Ed25519(_) => "soft",
            #[cfg(feature = "piv")]
            SigningBackend::Piv(_) => "piv",
        }
    }

    pub fn pubkey_pem(&self) -> &str {
        &self.pubkey_pem
    }

    pub fn pubkey_der(&self) -> &[u8] {
        &self.pubkey_der
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn identity_hash(&self) -> &str {
        &self.identity_hash
    }

    pub fn host_label(&self) -> &str {
        &self.host_label
    }

    pub fn agent_label(&self) -> &str {
        &self.agent_label
    }

    pub fn model_label(&self) -> &str {
        &self.model_label
    }

    pub fn cached_session(&self) -> Option<String> {
        self.session_token.lock().unwrap().clone()
    }

    pub fn set_session(&self, token: String) {
        *self.session_token.lock().unwrap() = Some(token);
    }

    pub fn clear_session(&self) {
        *self.session_token.lock().unwrap() = None;
    }

    pub fn sign_request(
        &self,
        method: &str,
        path: &str,
        query: &str,
        body: &[u8],
    ) -> Result<SignedRequest> {
        let ts_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let nonce = generate_nonce();

        let envelope = CanonicalEnvelope::new(
            method,
            path,
            query,
            body,
            ts_ms,
            &nonce,
            &self.identity_hash,
        );
        let msg = envelope.build();

        let sig_hex = match &self.backend {
            SigningBackend::Ed25519(sk) => {
                use ed25519_dalek::Signer;
                hex::encode(sk.sign(&msg).to_bytes())
            }
            #[cfg(feature = "piv")]
            SigningBackend::Piv(yk_mutex) => {
                let digest = Sha256::digest(&msg);
                let mut yk = yk_mutex.lock().unwrap();
                let result = piv_verify_and_sign(&mut yk, &digest);
                // Drop stale handle and open fresh on any PCSC error
                let sig_der = match result {
                    Ok(d) => d,
                    Err(_) => {
                        drop(yk);
                        let mut fresh = yubikey::YubiKey::open().map_err(|e| {
                            EngError::Internal(format!("YubiKey reconnect failed: {e}"))
                        })?;
                        let d = piv_verify_and_sign(&mut fresh, &digest).map_err(|e| {
                            EngError::Internal(format!(
                                "YubiKey PIV signing failed after reconnect: {e}"
                            ))
                        })?;
                        *yk_mutex.lock().unwrap() = fresh;
                        d
                    }
                };
                let sig = p256::ecdsa::Signature::from_der(&sig_der).map_err(|e| {
                    EngError::Internal(format!("invalid ECDSA DER from YubiKey: {e}"))
                })?;
                hex::encode(sig.to_bytes())
            }
        };

        Ok(SignedRequest {
            sig_hex,
            algo: self.algo,
            identity_hash: self.identity_hash.clone(),
            ts_ms,
            nonce,
            key_fp: self.fingerprint.clone(),
            host_label: self.host_label.clone(),
            agent_label: self.agent_label.clone(),
            model_label: self.model_label.clone(),
        })
    }

    pub fn generate_keypair() -> ([u8; 32], String) {
        let mut secret = [0u8; 32];
        use rand::Rng;
        rand::rng().fill(&mut secret);
        let sk = ed25519_dalek::SigningKey::from_bytes(&secret);
        let vk = sk.verifying_key();

        let mut der = Vec::with_capacity(44);
        der.extend_from_slice(&ED25519_SPKI_PREFIX_CONST);
        der.extend_from_slice(vk.as_bytes());
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&der);
        let pubkey_pem = format!("-----BEGIN PUBLIC KEY-----\n{b64}\n-----END PUBLIC KEY-----");

        (secret, pubkey_pem)
    }
}

pub struct SignedRequest {
    pub sig_hex: String,
    pub algo: SignatureAlgo,
    pub identity_hash: String,
    pub ts_ms: u64,
    pub nonce: String,
    pub key_fp: String,
    pub host_label: String,
    pub agent_label: String,
    pub model_label: String,
}

impl SignedRequest {
    pub fn apply_headers(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.header("X-Kleos-Sig", &self.sig_hex)
            .header("X-Kleos-Algo", self.algo.as_str())
            .header("X-Kleos-Identity", &self.identity_hash)
            .header("X-Kleos-Ts", self.ts_ms.to_string())
            .header("X-Kleos-Nonce", &self.nonce)
            .header("X-Kleos-Key-Fp", &self.key_fp)
            .header("X-Kleos-Host", &self.host_label)
            .header("X-Kleos-Agent", &self.agent_label)
            .header("X-Kleos-Model", &self.model_label)
    }
}

fn dirs_for_key_path() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    // -- Envelope tests --

    #[test]
    fn envelope_deterministic() {
        let e1 = CanonicalEnvelope::new("POST", "/store", "", b"hello", 1000, "aabb", "ccdd");
        let e2 = CanonicalEnvelope::new("POST", "/store", "", b"hello", 1000, "aabb", "ccdd");
        assert_eq!(e1.build(), e2.build());
    }

    #[test]
    fn envelope_empty_body() {
        let e = CanonicalEnvelope::new("GET", "/search", "q=test", b"", 1000, "aa", "bb");
        let built = e.build();
        let s = String::from_utf8(built).unwrap();
        assert!(s.contains(&hex::encode(Sha256::digest(b""))));
    }

    #[test]
    fn envelope_sorts_query_params() {
        let e1 = CanonicalEnvelope::new("GET", "/s", "z=1&a=2", b"", 1000, "aa", "bb");
        let e2 = CanonicalEnvelope::new("GET", "/s", "a=2&z=1", b"", 1000, "aa", "bb");
        assert_eq!(e1.build(), e2.build());
    }

    #[test]
    fn envelope_method_uppercased() {
        let e1 = CanonicalEnvelope::new("post", "/x", "", b"", 1, "a", "b");
        let e2 = CanonicalEnvelope::new("POST", "/x", "", b"", 1, "a", "b");
        assert_eq!(e1.build(), e2.build());
    }

    // -- HKDF tests --

    #[test]
    fn hkdf_deterministic() {
        let pk = b"fake-pubkey-der-bytes";
        let h1 = derive_identity_hash(pk, "wsl", "claude-code", "opus");
        let h2 = derive_identity_hash(pk, "wsl", "claude-code", "opus");
        assert_eq!(h1, h2);
    }

    #[test]
    fn hkdf_different_labels_different_hash() {
        let pk = b"same-key";
        let h1 = derive_identity_hash(pk, "host-a", "claude-code", "opus");
        let h2 = derive_identity_hash(pk, "host-b", "claude-code", "opus");
        let h3 = derive_identity_hash(pk, "host-a", "opencode", "opus");
        assert_ne!(h1, h2);
        assert_ne!(h1, h3);
        assert_ne!(h2, h3);
    }

    #[test]
    fn hkdf_empty_labels_valid() {
        let pk = b"key";
        let h = derive_identity_hash(pk, "", "", "");
        assert_eq!(h.len(), 16);
        let h2 = derive_identity_hash(pk, "a", "", "");
        assert_ne!(h, h2);
    }

    // -- P256 sign/verify round-trip --

    #[test]
    fn p256_sign_verify_roundtrip() {
        use p256::ecdsa::{signature::Signer, Signature, SigningKey};
        use p256::elliptic_curve::rand_core::OsRng;
        use p256::pkcs8::{EncodePublicKey, LineEnding};

        let sk = SigningKey::random(&mut OsRng);
        let vk = sk.verifying_key();
        let pubkey_pem = vk.to_public_key_pem(LineEnding::LF).unwrap();

        let envelope = CanonicalEnvelope::new(
            "POST",
            "/store",
            "",
            b"{\"content\":\"test\"}",
            now_ms(),
            "aabbccdd",
            "1122334455",
        );
        let msg = envelope.build();
        let sig: Signature = sk.sign(&msg);
        let sig_hex = hex::encode(sig.to_bytes());

        verify_signature(SignatureAlgo::EcdsaP256, &pubkey_pem, &msg, &sig_hex).unwrap();
    }

    #[test]
    fn p256_bad_sig_rejected() {
        use p256::ecdsa::SigningKey;
        use p256::elliptic_curve::rand_core::OsRng;
        use p256::pkcs8::{EncodePublicKey, LineEnding};

        let sk = SigningKey::random(&mut OsRng);
        let vk = sk.verifying_key();
        let pubkey_pem = vk.to_public_key_pem(LineEnding::LF).unwrap();

        let msg = b"KLEOSv1\ntest";
        let bad_sig = "00".repeat(64);

        let result = verify_signature(SignatureAlgo::EcdsaP256, &pubkey_pem, msg, &bad_sig);
        assert!(result.is_err());
    }

    #[test]
    fn p256_manual_pem_wrapping_roundtrip() {
        use p256::ecdsa::{signature::Signer, Signature, SigningKey};
        use p256::elliptic_curve::rand_core::OsRng;
        use p256::pkcs8::EncodePublicKey;

        let sk = SigningKey::random(&mut OsRng);
        let vk = sk.verifying_key();

        let pubkey_der = vk.to_public_key_der().unwrap();
        let b64 = {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.encode(pubkey_der.as_ref())
        };
        assert!(
            b64.len() > 64,
            "P-256 SPKI base64 must exceed 64 chars to test wrapping"
        );

        let wrapped: Vec<&str> = b64
            .as_bytes()
            .chunks(64)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect();
        let pem = format!(
            "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----",
            wrapped.join("\n")
        );

        let msg = b"KLEOS-ENROLL:ecdsa-p256:piv:testhost:fakepem";
        let sig: Signature = sk.sign(msg.as_ref());
        let sig_hex = hex::encode(sig.to_bytes());

        verify_signature(SignatureAlgo::EcdsaP256, &pem, msg, &sig_hex).unwrap();
    }

    // -- Ed25519 sign/verify round-trip --

    #[test]
    fn ed25519_sign_verify_roundtrip() {
        use ed25519_dalek::{Signer, SigningKey};

        let mut secret = [0u8; 32];
        rand::Rng::fill(&mut rand::rng(), &mut secret);
        let sk = SigningKey::from_bytes(&secret);
        let vk = sk.verifying_key();

        // Build PEM from the 32-byte pubkey
        let pubkey_pem = ed25519_pubkey_to_pem(vk.as_bytes());

        let envelope =
            CanonicalEnvelope::new("GET", "/search", "q=test", b"", now_ms(), "aabb", "ccdd");
        let msg = envelope.build();
        let sig = sk.sign(&msg);
        let sig_hex = hex::encode(sig.to_bytes());

        verify_signature(SignatureAlgo::Ed25519, &pubkey_pem, &msg, &sig_hex).unwrap();
    }

    #[test]
    fn ed25519_bad_sig_rejected() {
        use ed25519_dalek::SigningKey;

        let mut secret = [0u8; 32];
        rand::Rng::fill(&mut rand::rng(), &mut secret);
        let sk = SigningKey::from_bytes(&secret);
        let vk = sk.verifying_key();
        let pubkey_pem = ed25519_pubkey_to_pem(vk.as_bytes());

        let msg = b"KLEOSv1\ntest";
        let bad_sig = "00".repeat(64);

        let result = verify_signature(SignatureAlgo::Ed25519, &pubkey_pem, msg, &bad_sig);
        assert!(result.is_err());
    }

    fn ed25519_pubkey_to_pem(raw: &[u8; 32]) -> String {
        // Build SubjectPublicKeyInfo DER
        let mut der = Vec::with_capacity(44);
        der.extend_from_slice(&[
            0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
        ]);
        der.extend_from_slice(raw);
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&der);
        format!("-----BEGIN PUBLIC KEY-----\n{b64}\n-----END PUBLIC KEY-----")
    }

    // -- Replay guard tests --

    #[test]
    fn replay_rejects_duplicate_nonce() {
        let guard = ReplayGuard::new();
        let ts = now_ms();
        guard.check("id1", "nonce1", ts).unwrap();
        let result = guard.check("id1", "nonce1", ts);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("replay"));
    }

    #[test]
    fn replay_allows_different_nonces() {
        let guard = ReplayGuard::new();
        let ts = now_ms();
        guard.check("id1", "nonce1", ts).unwrap();
        guard.check("id1", "nonce2", ts).unwrap();
    }

    #[test]
    fn replay_rejects_stale_timestamp() {
        let guard = ReplayGuard::new();
        let old = now_ms() - 120_000;
        let result = guard.check("id1", "nonce1", old);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("window"));
    }

    // -- Session token tests --

    #[test]
    fn session_mint_verify_roundtrip() {
        let mgr = SessionManager::new([42u8; 32]);
        let token = mgr.mint(7);
        let id = mgr.verify(&token).unwrap();
        assert_eq!(id, 7);
    }

    #[test]
    fn session_unknown_token_rejected() {
        let mgr = SessionManager::new([42u8; 32]);
        let result = mgr.verify("bogus-token");
        assert!(result.is_err());
    }

    #[test]
    fn session_different_key_cannot_verify() {
        let mgr1 = SessionManager::new([1u8; 32]);
        let mgr2 = SessionManager::new([2u8; 32]);
        let token = mgr1.mint(5);
        let result = mgr2.verify(&token);
        assert!(result.is_err());
    }

    // -- Sliding-window refresh tests --
    //
    // These use short Durations + thread::sleep to exercise the timing
    // logic. Bounds are generous enough (40ms+ between checks) to remain
    // reliable on slow CI runners.

    #[test]
    fn session_refresh_returns_new_token_and_keeps_old_valid_until_expiry() {
        // Concurrency safety: two near-simultaneous requests carrying the
        // same cached token must both verify, so refresh leaves the old
        // entry in the map until its own expires_at fires.
        let mgr = SessionManager::with_durations(
            [42u8; 32],
            Duration::from_secs(60),
            Duration::from_secs(3600),
        );
        let old = mgr.mint(11);
        let new = mgr.refresh(&old).expect("refresh succeeds");
        assert_ne!(old, new, "refresh must mint a distinct token");
        assert_eq!(mgr.verify(&new).expect("new token valid"), 11);
        assert_eq!(
            mgr.verify(&old)
                .expect("old token still verifies until natural expiry"),
            11,
        );
    }

    #[test]
    fn session_refresh_is_concurrency_safe() {
        // N threads racing to refresh the same token: every thread should
        // either get Ok(new_token) or, if max_lifetime is hit, Err -- but
        // never panic, never produce a verify-fail for the original token
        // while still within its TTL, and the verify of every returned
        // token should succeed.
        let mgr = Arc::new(SessionManager::with_durations(
            [7u8; 32],
            Duration::from_secs(60),
            Duration::from_secs(3600),
        ));
        let original = mgr.mint(99);

        let threads = 16;
        let barrier = Arc::new(std::sync::Barrier::new(threads));
        let mut handles = Vec::with_capacity(threads);
        for _ in 0..threads {
            let mgr = mgr.clone();
            let token = original.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                let new = mgr.refresh(&token).expect("concurrent refresh");
                let id = mgr.verify(&new).expect("refreshed token verifies");
                assert_eq!(id, 99);
            }));
        }
        for h in handles {
            h.join().expect("thread joined");
        }

        // The original is still valid after the storm.
        assert_eq!(mgr.verify(&original).expect("original still valid"), 99);
    }

    #[test]
    fn session_refresh_unknown_token_rejected() {
        let mgr = SessionManager::new([42u8; 32]);
        let err = mgr.refresh("bogus").unwrap_err().to_string();
        assert!(err.contains("unknown"), "unexpected err: {err}");
    }

    #[test]
    fn session_refresh_expired_token_rejected() {
        let mgr = SessionManager::with_durations(
            [42u8; 32],
            Duration::from_millis(40),
            Duration::from_secs(60),
        );
        let token = mgr.mint(3);
        std::thread::sleep(Duration::from_millis(80));
        let err = mgr.refresh(&token).unwrap_err().to_string();
        assert!(err.contains("expired"), "unexpected err: {err}");
    }

    #[test]
    fn session_sliding_window_carries_past_original_ttl() {
        // ttl=60ms, cap=10s. Original token would die at t=60ms; refresh
        // before then and the new token outlives the original window.
        let mgr = SessionManager::with_durations(
            [42u8; 32],
            Duration::from_millis(60),
            Duration::from_secs(10),
        );
        let t0 = mgr.mint(9);
        std::thread::sleep(Duration::from_millis(30));
        let t1 = mgr.refresh(&t0).expect("refresh while still alive");
        std::thread::sleep(Duration::from_millis(50));
        // t=80ms now -- t0 would have expired by 60ms, but t1 (issued at
        // t=30, expires at t=90) is still alive.
        assert_eq!(
            mgr.verify(&t1).expect("rolled-forward token still valid"),
            9
        );
        assert!(
            mgr.verify(&t0).is_err(),
            "original token must not be reusable after refresh"
        );
    }

    #[test]
    fn session_hard_cap_blocks_refresh_even_when_current_token_valid() {
        // ttl=80ms, cap=100ms. After a refresh at t=40ms the current token
        // is valid until t=120ms, but at t=110ms the absolute lifetime
        // (t > 100ms since original issue) blocks further refresh -- the
        // caller must re-PIV-sign to get a fresh session.
        let mgr = SessionManager::with_durations(
            [42u8; 32],
            Duration::from_millis(80),
            Duration::from_millis(100),
        );
        let t0 = mgr.mint(4);
        std::thread::sleep(Duration::from_millis(40));
        let t1 = mgr.refresh(&t0).expect("first refresh inside cap");
        std::thread::sleep(Duration::from_millis(70));
        // t=110ms: t1 still verifies (issued at 40, expires at 120)
        assert_eq!(mgr.verify(&t1).expect("current token still alive"), 4);
        // but refresh declines because issued_at=0 and now > 100ms
        let err = mgr.refresh(&t1).unwrap_err().to_string();
        assert!(
            err.contains("absolute lifetime") || err.contains("lifetime cap"),
            "unexpected err: {err}"
        );
    }

    #[test]
    fn session_with_durations_floors_max_lifetime_to_ttl() {
        // An incoherent config (max < ttl) is clamped, not rejected, so a
        // misconfigured server never has refresh logic running backwards.
        let mgr = SessionManager::with_durations(
            [0u8; 32],
            Duration::from_secs(600),
            Duration::from_secs(60),
        );
        assert_eq!(mgr.ttl, Duration::from_secs(600));
        assert_eq!(mgr.max_lifetime, Duration::from_secs(600));
    }
}
