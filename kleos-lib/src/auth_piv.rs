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
    let mut key = [0u8; 32];
    key.copy_from_slice(&der[12..]);
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

pub fn derive_identity_hash(
    pubkey_der: &[u8],
    host: &str,
    agent: &str,
    model: &str,
) -> [u8; 16] {
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

        let drift = if now_ms > ts_ms {
            now_ms - ts_ms
        } else {
            ts_ms - now_ms
        };
        if drift > REPLAY_WINDOW_MS {
            return Err(EngError::Auth(format!(
                "request timestamp outside {REPLAY_WINDOW_MS}ms window (drift={drift}ms)"
            )));
        }

        let key = (identity_hash_hex.to_string(), nonce_hex.to_string());
        let mut nonces = self.nonces.lock().unwrap();

        self.maybe_gc(&mut nonces);

        if nonces.contains_key(&key) {
            return Err(EngError::Auth("duplicate nonce (replay)".into()));
        }
        nonces.insert(key, NonceEntry { inserted: Instant::now() });
        Ok(())
    }

    fn maybe_gc(&self, nonces: &mut HashMap<(String, String), NonceEntry>) {
        let mut last = self.last_gc.lock().unwrap();
        if last.elapsed() < GC_INTERVAL {
            return;
        }
        *last = Instant::now();
        nonces.retain(|_, v| v.inserted.elapsed() < NONCE_TTL);
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

const SESSION_TTL: Duration = Duration::from_secs(900); // 15 minutes

pub struct SessionManager {
    key: [u8; 32],
    sessions: Arc<Mutex<HashMap<String, SessionEntry>>>,
}

struct SessionEntry {
    identity_id: i64,
    expires_at: Instant,
}

impl SessionManager {
    pub fn new(key: [u8; 32]) -> Self {
        Self {
            key,
            sessions: Arc::new(Mutex::new(HashMap::new())),
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
        Ok(Self::new(key))
    }

    pub fn mint(&self, identity_id: i64) -> String {
        let expires_at = Instant::now() + SESSION_TTL;
        let expires_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            + SESSION_TTL.as_millis() as u64;

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

        let mut sessions = self.sessions.lock().unwrap();
        sessions.insert(
            token.clone(),
            SessionEntry {
                identity_id,
                expires_at,
            },
        );

        // GC expired sessions opportunistically
        sessions.retain(|_, v| v.expires_at > Instant::now());

        token
    }

    pub fn verify(&self, token: &str) -> Result<i64> {
        let sessions = self.sessions.lock().unwrap();
        let entry = sessions
            .get(token)
            .ok_or_else(|| EngError::Auth("unknown session token".into()))?;

        if entry.expires_at <= Instant::now() {
            return Err(EngError::Auth("session token expired".into()));
        }

        Ok(entry.identity_id)
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

    let drift = if now_ms > ts_ms {
        now_ms - ts_ms
    } else {
        ts_ms - now_ms
    };
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
        let h1 = derive_identity_hash(pk, "wsl", "claude-code", "opus");
        let h2 = derive_identity_hash(pk, "rocky", "claude-code", "opus");
        let h3 = derive_identity_hash(pk, "wsl", "opencode", "opus");
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
        use p256::ecdsa::{SigningKey, Signature, signature::Signer};
        use p256::elliptic_curve::rand_core::OsRng;
        use p256::pkcs8::{EncodePublicKey, LineEnding};

        let sk = SigningKey::random(&mut OsRng);
        let vk = sk.verifying_key();
        let pubkey_pem = vk.to_public_key_pem(LineEnding::LF).unwrap();

        let envelope = CanonicalEnvelope::new(
            "POST", "/store", "", b"{\"content\":\"test\"}", now_ms(), "aabbccdd", "1122334455",
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

    // -- Ed25519 sign/verify round-trip --

    #[test]
    fn ed25519_sign_verify_roundtrip() {
        use ed25519_dalek::{SigningKey, Signer};

        let mut secret = [0u8; 32];
        rand::Rng::fill(&mut rand::rng(), &mut secret);
        let sk = SigningKey::from_bytes(&secret);
        let vk = sk.verifying_key();

        // Build PEM from the 32-byte pubkey
        let pubkey_pem = ed25519_pubkey_to_pem(vk.as_bytes());

        let envelope = CanonicalEnvelope::new(
            "GET", "/search", "q=test", b"", now_ms(), "aabb", "ccdd",
        );
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
}
