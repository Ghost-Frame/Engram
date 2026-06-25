//! Single-use capability tokens for out-of-band approval decisions.
//!
//! When an M3 approval is raised, phylaxd generates a single-use capability
//! token and persists only its SHA-256 hash on the approval row. The raw token
//! is handed once to an external, operator-run notifier over a private
//! transport; whoever presents the matching token back to the `decide-token`
//! endpoint may decide that one approval. Verification is constant-time, and the
//! raw token never appears in logs or API responses.

use rand::rngs::OsRng;
use rand::TryRngCore;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// A freshly generated capability token: the raw value (handed to the notifier
/// exactly once) and its hex SHA-256 hash (the only form persisted).
pub struct CapabilityToken {
    /// Raw 32-byte token, hex-encoded. Sent out of band once, never stored.
    pub raw: String,
    /// Hex SHA-256 of `raw`. Persisted on the approval row.
    pub hash_hex: String,
}

/// Generate a 32-byte random token plus its hex SHA-256 hash.
pub fn generate() -> CapabilityToken {
    let mut bytes = [0u8; 32];
    OsRng
        .try_fill_bytes(&mut bytes)
        .expect("OS CSPRNG must be available");
    let raw = hex::encode(bytes);
    let hash_hex = hash_hex(&raw);
    CapabilityToken { raw, hash_hex }
}

/// Hex SHA-256 of a raw token string.
pub fn hash_hex(raw: &str) -> String {
    let digest = Sha256::digest(raw.as_bytes());
    hex::encode(digest)
}

/// Constant-time compare a presented raw token against a stored hex hash.
pub fn verify(presented_raw: &str, stored_hash_hex: &str) -> bool {
    let presented = hash_hex(presented_raw);
    presented
        .as_bytes()
        .ct_eq(stored_hash_hex.as_bytes())
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_then_verify_roundtrips() {
        let t = generate();
        assert_eq!(t.raw.len(), 64); // 32 bytes hex
        assert!(verify(&t.raw, &t.hash_hex));
    }

    #[test]
    fn wrong_token_fails() {
        let t = generate();
        assert!(!verify("deadbeef", &t.hash_hex));
    }

    #[test]
    fn hash_is_deterministic() {
        assert_eq!(hash_hex("abc"), hash_hex("abc"));
        assert_ne!(hash_hex("abc"), hash_hex("abd"));
    }
}
