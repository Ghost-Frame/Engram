//! Artifact encryption primitives: key parsing, per-tenant derivation, and
//! AES-256-GCM encrypt/decrypt.
//!
//! Wire format: `[12-byte nonce][ciphertext][16-byte tag]`
//! (AES-256-GCM standard output from the `aes_gcm` crate).
//!
//! Key hierarchy:
//!   master_key (32 bytes, from env/credd)
//!     -> HKDF-SHA256(salt="kleos-artifact-v1", info=tenant_id)
//!       -> per-tenant AES-256-GCM key (32 bytes)

use aes_gcm::aead::{Aead, AeadCore, OsRng};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;

use crate::{EngError, Result};

/// HKDF salt for artifact key derivation (versioned for future rotation).
const HKDF_SALT: &[u8] = b"kleos-artifact-v1";

/// AES-256-GCM nonce length in bytes.
const NONCE_LEN: usize = 12;

/// Parse a master encryption key from hex (64 chars) or base64 (44 chars).
///
/// Returns `Err` if the input is neither valid hex nor valid base64, or if
/// the decoded bytes are not exactly 32 bytes.
pub fn parse_encryption_key(input: &str) -> Result<[u8; 32]> {
    let input = input.trim();
    if input.is_empty() {
        return Err(EngError::InvalidInput(
            "encryption key must not be empty".into(),
        ));
    }

    // Try hex first (64 hex chars = 32 bytes).
    if input.len() == 64 && input.chars().all(|c| c.is_ascii_hexdigit()) {
        let bytes = hex::decode(input)
            .map_err(|e| EngError::InvalidInput(format!("invalid hex key: {e}")))?;
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        return Ok(key);
    }

    // Try base64 (44 chars with padding = 32 bytes).
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(input)
        .map_err(|e| EngError::InvalidInput(format!("invalid base64 key: {e}")))?;
    if bytes.len() != 32 {
        return Err(EngError::InvalidInput(format!(
            "encryption key must be 32 bytes, got {}",
            bytes.len()
        )));
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    Ok(key)
}

/// Derive a per-tenant 32-byte AES-256 key via HKDF-SHA256.
///
/// Uses a versioned salt (`kleos-artifact-v1`) and the tenant ID as info,
/// following the same pattern as `auth_piv::derive_identity_hash`.
pub fn derive_tenant_key(master: &[u8; 32], tenant_id: &str) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(Some(HKDF_SALT), master);
    let mut derived = [0u8; 32];
    hk.expand(tenant_id.as_bytes(), &mut derived)
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    derived
}

/// AES-256-GCM encrypt. Output: `[12-byte nonce][ciphertext][16-byte tag]`.
///
/// The nonce is generated from the OS CSPRNG via `OsRng`.
pub fn encrypt_artifact(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new(key.into());
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|e| EngError::Internal(format!("AES-256-GCM encrypt failed: {e}")))?;

    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// AES-256-GCM decrypt. Input: `[12-byte nonce][ciphertext][16-byte tag]`.
pub fn decrypt_artifact(key: &[u8; 32], ciphertext: &[u8]) -> Result<Vec<u8>> {
    if ciphertext.len() < NONCE_LEN + 16 {
        return Err(EngError::InvalidInput(
            "ciphertext too short for AES-256-GCM (need at least nonce + tag)".into(),
        ));
    }
    let (nonce_bytes, ct) = ciphertext.split_at(NONCE_LEN);
    let nonce = Nonce::from_slice(nonce_bytes);
    let cipher = Aes256Gcm::new(key.into());
    cipher
        .decrypt(nonce, ct)
        .map_err(|_| EngError::Internal("AES-256-GCM decrypt failed (wrong key or corrupt)".into()))
}

/// Encryption state holder. `None` master key = encryption disabled.
pub struct ArtifactEncryption {
    master_key: Option<[u8; 32]>,
}

impl ArtifactEncryption {
    /// Create from a key source string. Empty string disables encryption.
    /// Valid hex (64 chars) or base64 (44 chars) enables it.
    pub fn new(key_source: &str) -> Result<Self> {
        let trimmed = key_source.trim();
        if trimmed.is_empty() {
            return Ok(Self { master_key: None });
        }
        let key = parse_encryption_key(trimmed)?;
        Ok(Self {
            master_key: Some(key),
        })
    }

    /// Whether artifact encryption is active.
    pub fn is_enabled(&self) -> bool {
        self.master_key.is_some()
    }

    /// Encrypt data for a specific tenant. Returns plaintext unchanged if
    /// encryption is disabled.
    pub fn encrypt_for_tenant(&self, tenant_id: &str, data: &[u8]) -> Result<Vec<u8>> {
        let master = match &self.master_key {
            Some(k) => k,
            None => return Ok(data.to_vec()),
        };
        let tenant_key = derive_tenant_key(master, tenant_id);
        encrypt_artifact(&tenant_key, data)
    }

    /// Decrypt data for a specific tenant. Returns ciphertext unchanged if
    /// encryption is disabled.
    pub fn decrypt_for_tenant(&self, tenant_id: &str, data: &[u8]) -> Result<Vec<u8>> {
        let master = match &self.master_key {
            Some(k) => k,
            None => return Ok(data.to_vec()),
        };
        let tenant_key = derive_tenant_key(master, tenant_id);
        decrypt_artifact(&tenant_key, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip: encrypt then decrypt must recover the original plaintext.
    #[test]
    fn roundtrip_encrypt_decrypt() {
        let key = [0x42u8; 32];
        let plaintext = b"the artifact content to protect";
        let encrypted = encrypt_artifact(&key, plaintext).expect("encrypt");
        assert_ne!(&encrypted[NONCE_LEN..], plaintext);
        let decrypted = decrypt_artifact(&key, &encrypted).expect("decrypt");
        assert_eq!(decrypted, plaintext);
    }

    /// Decrypting with the wrong key must fail.
    #[test]
    fn wrong_key_fails() {
        let key_a = [0x01u8; 32];
        let key_b = [0x02u8; 32];
        let encrypted = encrypt_artifact(&key_a, b"secret data").expect("encrypt");
        let result = decrypt_artifact(&key_b, &encrypted);
        assert!(result.is_err());
    }

    /// Truncated ciphertext must fail.
    #[test]
    fn truncated_ciphertext_fails() {
        let key = [0xAAu8; 32];
        let encrypted = encrypt_artifact(&key, b"test data").expect("encrypt");
        let truncated = &encrypted[..NONCE_LEN + 5];
        let result = decrypt_artifact(&key, truncated);
        assert!(result.is_err());
    }

    /// Parse a 64-character hex string as a 32-byte key.
    #[test]
    fn parse_key_hex() {
        let hex_key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let key = parse_encryption_key(hex_key).expect("parse hex key");
        assert_eq!(key[0], 0x01);
        assert_eq!(key[15], 0xef);
    }

    /// Parse a 44-character base64 string as a 32-byte key.
    #[test]
    fn parse_key_base64() {
        use base64::Engine;
        let raw = [0x55u8; 32];
        let b64 = base64::engine::general_purpose::STANDARD.encode(raw);
        let key = parse_encryption_key(&b64).expect("parse base64 key");
        assert_eq!(key, raw);
    }

    /// Garbage input must be rejected.
    #[test]
    fn parse_key_invalid_rejects() {
        assert!(parse_encryption_key("not-a-valid-key").is_err());
        assert!(parse_encryption_key("zz").is_err());
    }

    /// Two different tenant IDs must produce different derived keys.
    #[test]
    fn per_tenant_derivation_differs() {
        let master = [0xBBu8; 32];
        let k1 = derive_tenant_key(&master, "tenant-alpha");
        let k2 = derive_tenant_key(&master, "tenant-beta");
        assert_ne!(k1, k2);
    }

    /// Empty key source string disables encryption.
    #[test]
    fn empty_key_source_disables() {
        let enc = ArtifactEncryption::new("").expect("empty key");
        assert!(!enc.is_enabled());
    }

    /// ArtifactEncryption round-trip through tenant-scoped methods.
    #[test]
    fn tenant_scoped_roundtrip() {
        let hex_key = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let enc = ArtifactEncryption::new(hex_key).expect("init");
        assert!(enc.is_enabled());

        let plaintext = b"tenant-specific secret";
        let ct = enc.encrypt_for_tenant("t1", plaintext).expect("encrypt");
        let pt = enc.decrypt_for_tenant("t1", &ct).expect("decrypt");
        assert_eq!(pt, plaintext);
    }

    /// Encrypting with one tenant and decrypting with another must fail.
    #[test]
    fn cross_tenant_decrypt_fails() {
        let hex_key = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let enc = ArtifactEncryption::new(hex_key).expect("init");

        let ct = enc
            .encrypt_for_tenant("tenant-1", b"data")
            .expect("encrypt");
        let result = enc.decrypt_for_tenant("tenant-2", &ct);
        assert!(result.is_err());
    }

    /// When encryption is disabled, encrypt/decrypt pass data through unchanged.
    #[test]
    fn disabled_passthrough() {
        let enc = ArtifactEncryption::new("").expect("disabled");
        let data = b"cleartext data";
        let encrypted = enc.encrypt_for_tenant("t1", data).expect("encrypt");
        assert_eq!(encrypted, data);
        let decrypted = enc.decrypt_for_tenant("t1", data).expect("decrypt");
        assert_eq!(decrypted, data);
    }
}
