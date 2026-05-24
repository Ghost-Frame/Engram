//! MCP direct-auth token: identity-signed bearer for static-header clients.
//!
//! Token format: `kleos.<base64url(payload_json)>.<base64url(ed25519_sig)>`
//! Payload is canonical JSON (alphabetical keys via BTreeMap).

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::auth::Scope;

/// Current token format version.
const TOKEN_VERSION: u64 = 1;

/// Prefix that identifies an MCP direct-auth token in the Bearer header.
pub const TOKEN_PREFIX: &str = "kleos.";

/// Default TTL: 30 days in seconds.
pub const DEFAULT_TTL_SECS: u64 = 30 * 24 * 3600;

/// Default max TTL: 90 days in seconds.
pub const DEFAULT_MAX_TTL_SECS: u64 = 90 * 24 * 3600;

/// Parsed MCP token payload (deserialized from the base64url segment).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpTokenPayload {
    /// Token format version (always 1 for this spec).
    pub v: u64,
    /// Unique token ID, 32-char hex (128 bits). Revocation key.
    pub jti: String,
    /// Fingerprint of the signing identity key.
    pub kid: String,
    /// User ID of the token owner.
    pub uid: i64,
    /// Tenant ID. Absent in single-user/shared-DB mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tid: Option<i64>,
    /// CSV scope list (strict: only read, write, admin).
    pub scopes: String,
    /// Issued-at timestamp, unix seconds.
    pub iat: u64,
    /// Expiry timestamp, unix seconds.
    pub exp: u64,
}

/// A decoded but not yet verified MCP token (raw segments).
#[derive(Debug)]
pub struct DecodedToken {
    /// The raw payload bytes (base64url-decoded). Used for sig verification.
    pub payload_bytes: Vec<u8>,
    /// The parsed payload claims.
    pub payload: McpTokenPayload,
    /// The raw signature bytes (64 bytes Ed25519).
    pub signature_bytes: Vec<u8>,
}

/// Errors from MCP token operations.
#[derive(Debug, thiserror::Error)]
pub enum McpTokenError {
    #[error("malformed token: {0}")]
    Malformed(String),
    #[error("unsupported token version: {0}")]
    UnsupportedVersion(u64),
    #[error("token expired")]
    Expired,
    #[error("invalid scope: {0}")]
    InvalidScope(String),
    #[error("scope exceeds cap: {0}")]
    ScopeExceedsCap(String),
    #[error("TTL exceeds maximum: requested {requested}s, max {max}s")]
    TtlExceedsMax { requested: u64, max: u64 },
    #[error("invalid signature")]
    InvalidSignature,
    #[error("signing error: {0}")]
    SigningError(String),
    #[error("uid mismatch: token says {token_uid}, authenticated as {auth_uid}")]
    UidMismatch { token_uid: i64, auth_uid: i64 },
}

/// Parse scopes strictly: only accept explicit "read", "write", "admin".
/// Rejects "*" wildcard and unknown tokens. Returns error on invalid input.
pub fn parse_scopes_strict(s: &str) -> Result<Vec<Scope>, McpTokenError> {
    if s.is_empty() {
        return Err(McpTokenError::InvalidScope("empty scope string".into()));
    }
    let mut scopes = Vec::new();
    for part in s.split(',') {
        let trimmed = part.trim();
        match trimmed {
            "read" => scopes.push(Scope::Read),
            "write" => scopes.push(Scope::Write),
            "admin" => scopes.push(Scope::Admin),
            other => {
                return Err(McpTokenError::InvalidScope(format!(
                    "unknown or forbidden scope '{}' (wildcards not allowed)",
                    other
                )));
            }
        }
    }
    Ok(scopes)
}

/// Check that every scope in `requested` exists in `cap`.
pub fn scopes_within_cap(requested: &[Scope], cap: &[Scope]) -> Result<(), McpTokenError> {
    for scope in requested {
        if !cap.contains(scope) && !cap.contains(&Scope::Admin) {
            return Err(McpTokenError::ScopeExceedsCap(format!(
                "scope '{}' not in minting key's scopes",
                scope
            )));
        }
    }
    Ok(())
}

/// Build canonical payload JSON bytes from an McpTokenPayload.
/// Uses BTreeMap for alphabetical key order (deterministic signing).
fn canonical_payload_bytes(payload: &McpTokenPayload) -> Vec<u8> {
    let mut map = BTreeMap::new();
    map.insert("exp", serde_json::Value::from(payload.exp));
    map.insert("iat", serde_json::Value::from(payload.iat));
    map.insert("jti", serde_json::Value::from(payload.jti.as_str()));
    map.insert("kid", serde_json::Value::from(payload.kid.as_str()));
    map.insert("scopes", serde_json::Value::from(payload.scopes.as_str()));
    if let Some(tid) = payload.tid {
        map.insert("tid", serde_json::Value::from(tid));
    }
    map.insert("uid", serde_json::Value::from(payload.uid));
    map.insert("v", serde_json::Value::from(payload.v));
    serde_json::to_vec(&map).expect("BTreeMap serialization cannot fail")
}

/// Generate a 128-bit random jti (32 hex chars).
pub fn generate_jti() -> String {
    use rand::Rng;
    let bytes: [u8; 16] = rand::rng().random();
    hex::encode(bytes)
}

/// Get current unix timestamp in seconds.
pub fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_secs()
}

/// Mint a new MCP token: build payload, sign it, return the full token string.
pub fn mint(
    signing_key: &SigningKey,
    kid: &str,
    uid: i64,
    tid: Option<i64>,
    scopes: &str,
    ttl_secs: u64,
    max_ttl_secs: u64,
) -> Result<(String, McpTokenPayload), McpTokenError> {
    // Validate scopes strictly.
    let _ = parse_scopes_strict(scopes)?;

    // Enforce TTL cap.
    if ttl_secs > max_ttl_secs {
        return Err(McpTokenError::TtlExceedsMax {
            requested: ttl_secs,
            max: max_ttl_secs,
        });
    }

    let now = now_unix_secs();
    let payload = McpTokenPayload {
        v: TOKEN_VERSION,
        jti: generate_jti(),
        kid: kid.to_string(),
        uid,
        tid,
        scopes: scopes.to_string(),
        iat: now,
        exp: now + ttl_secs,
    };

    let payload_bytes = canonical_payload_bytes(&payload);
    let signature: Signature = signing_key.sign(&payload_bytes);

    let token = format!(
        "kleos.{}.{}",
        URL_SAFE_NO_PAD.encode(&payload_bytes),
        URL_SAFE_NO_PAD.encode(signature.to_bytes()),
    );

    Ok((token, payload))
}

/// Decode a token string into its raw components without verifying the signature.
/// Returns the raw payload bytes (for sig verification) and parsed claims.
pub fn decode(token: &str) -> Result<DecodedToken, McpTokenError> {
    let stripped = token
        .strip_prefix(TOKEN_PREFIX)
        .ok_or_else(|| McpTokenError::Malformed("missing kleos. prefix".into()))?;

    let parts: Vec<&str> = stripped.splitn(2, '.').collect();
    if parts.len() != 2 {
        return Err(McpTokenError::Malformed(
            "expected exactly 3 dot-separated segments".into(),
        ));
    }

    let payload_bytes = URL_SAFE_NO_PAD
        .decode(parts[0])
        .map_err(|e| McpTokenError::Malformed(format!("payload base64: {}", e)))?;

    let signature_bytes = URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|e| McpTokenError::Malformed(format!("signature base64: {}", e)))?;

    if signature_bytes.len() != 64 {
        return Err(McpTokenError::Malformed(format!(
            "signature must be 64 bytes, got {}",
            signature_bytes.len()
        )));
    }

    let payload: McpTokenPayload = serde_json::from_slice(&payload_bytes)
        .map_err(|e| McpTokenError::Malformed(format!("payload JSON: {}", e)))?;

    if payload.v != TOKEN_VERSION {
        return Err(McpTokenError::UnsupportedVersion(payload.v));
    }

    Ok(DecodedToken {
        payload_bytes,
        payload,
        signature_bytes,
    })
}

/// Verify the Ed25519 signature over the raw payload bytes.
/// Uses the decoded payload bytes directly (no re-serialization).
pub fn verify_signature(
    verifying_key: &VerifyingKey,
    decoded: &DecodedToken,
) -> Result<(), McpTokenError> {
    let sig_array: [u8; 64] = decoded
        .signature_bytes
        .as_slice()
        .try_into()
        .map_err(|_| McpTokenError::InvalidSignature)?;

    let signature = Signature::from_bytes(&sig_array);

    verifying_key
        .verify(&decoded.payload_bytes, &signature)
        .map_err(|_| McpTokenError::InvalidSignature)
}

/// Check if a token's exp claim is still valid.
pub fn check_expiry(payload: &McpTokenPayload) -> Result<(), McpTokenError> {
    if payload.exp <= now_unix_secs() {
        return Err(McpTokenError::Expired);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixed test keypair for deterministic tests.
    fn test_keypair() -> (SigningKey, VerifyingKey) {
        let sk = SigningKey::from_bytes(&[42u8; 32]);
        let vk = sk.verifying_key();
        (sk, vk)
    }

    /// Fingerprint for the test keypair (first 16 bytes of pubkey, hex-encoded).
    fn test_kid() -> String {
        let (_, vk) = test_keypair();
        hex::encode(&vk.to_bytes()[..16])
    }

    #[test]
    fn mint_and_verify_round_trip() {
        let (sk, vk) = test_keypair();
        let kid = test_kid();

        let (token, payload) = mint(&sk, &kid, 1, None, "read,write", 3600, 86400).unwrap();
        assert!(token.starts_with("kleos."));
        assert_eq!(payload.v, 1);
        assert_eq!(payload.uid, 1);
        assert_eq!(payload.scopes, "read,write");
        assert_eq!(payload.jti.len(), 32);

        let decoded = decode(&token).unwrap();
        assert_eq!(decoded.payload, payload);
        verify_signature(&vk, &decoded).unwrap();
    }

    #[test]
    fn expired_token_rejected() {
        let (sk, _) = test_keypair();
        let kid = test_kid();

        let (token, _) = mint(&sk, &kid, 1, None, "read", 0, 86400).unwrap();
        let decoded = decode(&token).unwrap();
        assert!(check_expiry(&decoded.payload).is_err());
    }

    #[test]
    fn invalid_signature_rejected() {
        let (sk, _) = test_keypair();
        let kid = test_kid();

        let (token, _) = mint(&sk, &kid, 1, None, "read", 3600, 86400).unwrap();
        let mut decoded = decode(&token).unwrap();
        // Corrupt the signature.
        decoded.signature_bytes[0] ^= 0xFF;

        let (_, vk) = test_keypair();
        assert!(verify_signature(&vk, &decoded).is_err());
    }

    #[test]
    fn wrong_key_rejected() {
        let (sk, _) = test_keypair();
        let kid = test_kid();

        let (token, _) = mint(&sk, &kid, 1, None, "read", 3600, 86400).unwrap();
        let decoded = decode(&token).unwrap();

        // Verify with a different key.
        let wrong_sk = SigningKey::from_bytes(&[99u8; 32]);
        let wrong_vk = wrong_sk.verifying_key();
        assert!(verify_signature(&wrong_vk, &decoded).is_err());
    }

    #[test]
    fn malformed_token_rejected() {
        assert!(decode("not-a-token").is_err());
        assert!(decode("kleos.onepart").is_err());
        assert!(decode("kleos.!!!.!!!").is_err());
        assert!(decode("").is_err());
    }

    #[test]
    fn unknown_version_rejected() {
        let (sk, _) = test_keypair();
        let kid = test_kid();

        let (token, _) = mint(&sk, &kid, 1, None, "read", 3600, 86400).unwrap();
        let decoded = decode(&token).unwrap();
        let mut payload = decoded.payload.clone();
        payload.v = 99;
        let payload_bytes = serde_json::to_vec(&payload).unwrap();
        let fake_token = format!(
            "kleos.{}.{}",
            URL_SAFE_NO_PAD.encode(&payload_bytes),
            URL_SAFE_NO_PAD.encode(&decoded.signature_bytes),
        );
        let err = decode(&fake_token).unwrap_err();
        assert!(matches!(err, McpTokenError::UnsupportedVersion(99)));
    }

    #[test]
    fn strict_scope_parser_rejects_wildcard() {
        assert!(parse_scopes_strict("*").is_err());
        assert!(parse_scopes_strict("read,*").is_err());
        assert!(parse_scopes_strict("").is_err());
        assert!(parse_scopes_strict("unknown").is_err());
    }

    #[test]
    fn strict_scope_parser_accepts_valid() {
        let scopes = parse_scopes_strict("read").unwrap();
        assert_eq!(scopes, vec![Scope::Read]);

        let scopes = parse_scopes_strict("read,write").unwrap();
        assert_eq!(scopes, vec![Scope::Read, Scope::Write]);

        let scopes = parse_scopes_strict("read,write,admin").unwrap();
        assert_eq!(scopes, vec![Scope::Read, Scope::Write, Scope::Admin]);
    }

    #[test]
    fn scope_cap_enforcement() {
        let cap = vec![Scope::Read, Scope::Write];
        assert!(scopes_within_cap(&[Scope::Read], &cap).is_ok());
        assert!(scopes_within_cap(&[Scope::Read, Scope::Write], &cap).is_ok());
        assert!(scopes_within_cap(&[Scope::Admin], &cap).is_err());
    }

    #[test]
    fn ttl_max_enforcement() {
        let (sk, _) = test_keypair();
        let kid = test_kid();
        let err = mint(&sk, &kid, 1, None, "read", 999999, 86400).unwrap_err();
        assert!(matches!(err, McpTokenError::TtlExceedsMax { .. }));
    }

    #[test]
    fn canonical_json_determinism() {
        let (_, _) = test_keypair();
        let kid = test_kid();

        let payload = McpTokenPayload {
            v: 1,
            jti: "a".repeat(32),
            kid: kid.clone(),
            uid: 1,
            tid: Some(3),
            scopes: "read,write".into(),
            iat: 1000,
            exp: 2000,
        };
        let bytes1 = canonical_payload_bytes(&payload);
        let bytes2 = canonical_payload_bytes(&payload);
        assert_eq!(bytes1, bytes2, "canonical JSON must be deterministic");

        // Verify round-trip: decode(encode(bytes)) == bytes.
        let encoded = URL_SAFE_NO_PAD.encode(&bytes1);
        let decoded = URL_SAFE_NO_PAD.decode(&encoded).unwrap();
        assert_eq!(bytes1, decoded);
    }

    #[test]
    fn tid_absent_when_none() {
        let payload = McpTokenPayload {
            v: 1,
            jti: "a".repeat(32),
            kid: "test".into(),
            uid: 1,
            tid: None,
            scopes: "read".into(),
            iat: 1000,
            exp: 2000,
        };
        let bytes = canonical_payload_bytes(&payload);
        let json_str = String::from_utf8(bytes).unwrap();
        assert!(
            !json_str.contains("tid"),
            "tid should be absent from JSON when None, got: {}",
            json_str
        );
    }

    #[test]
    fn jti_is_128_bits() {
        let jti = generate_jti();
        assert_eq!(jti.len(), 32, "jti must be 32 hex chars (128 bits)");
        assert!(
            jti.chars().all(|c| c.is_ascii_hexdigit()),
            "jti must be hex"
        );
    }
}
