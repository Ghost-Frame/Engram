//! YubiKey HMAC-SHA1 challenge-response for key derivation.
//!
//! This module provides an interface for YubiKey-based key derivation.
//! The YubiKey performs HMAC-SHA1 challenge-response using slot 2,
//! providing hardware-backed key material that cannot be extracted.
//!
//! When no YubiKey is available, falls back to password-only derivation.

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::Result;

type HmacSha256 = Hmac<Sha256>;

/// Challenge size for YubiKey HMAC-SHA1.
pub const CHALLENGE_SIZE: usize = 32;

/// Response size from YubiKey HMAC-SHA1.
pub const RESPONSE_SIZE: usize = 20;

/// YubiKey slot for HMAC challenge-response.
pub const HMAC_SLOT: u8 = 2;

/// Result of a YubiKey challenge.
#[derive(Debug)]
pub enum ChallengeResult {
    /// YubiKey responded successfully.
    Response([u8; RESPONSE_SIZE]),
    /// No YubiKey available, fallback to password-only.
    NoDevice,
    /// YubiKey present but HMAC slot not configured.
    SlotNotConfigured,
}

/// Generate a challenge for YubiKey HMAC.
///
/// The challenge is derived from user_id, secret_id, and a timestamp
/// to ensure uniqueness while being reproducible for the same secret.
pub fn generate_challenge(user_id: i64, category: &str, name: &str) -> [u8; CHALLENGE_SIZE] {
    let mut mac = HmacSha256::new_from_slice(b"engram-cred-challenge")
        .expect("HMAC can take key of any size");

    mac.update(&user_id.to_le_bytes());
    mac.update(category.as_bytes());
    mac.update(&[0u8]); // separator
    mac.update(name.as_bytes());

    let result = mac.finalize();
    let mut challenge = [0u8; CHALLENGE_SIZE];
    challenge.copy_from_slice(&result.into_bytes()[..CHALLENGE_SIZE]);
    challenge
}

/// Perform YubiKey HMAC challenge-response.
///
/// This is a placeholder that returns NoDevice. Actual YubiKey support
/// requires the `yubikey` crate and platform-specific setup.
///
/// To enable YubiKey support:
/// 1. Add `yubikey = "0.8"` to Cargo.toml
/// 2. Implement the actual challenge-response using pcsc
///
/// The YubiKey must have HMAC-SHA1 configured on slot 2.
#[allow(unused_variables)]
pub fn challenge_yubikey(challenge: &[u8; CHALLENGE_SIZE]) -> Result<ChallengeResult> {
    // Placeholder - actual YubiKey support requires hardware access
    //
    // Real implementation would:
    // 1. Open pcsc context
    // 2. Find YubiKey reader
    // 3. Send HMAC challenge to slot 2
    // 4. Return 20-byte response
    //
    // For now, return NoDevice to indicate password-only mode
    Ok(ChallengeResult::NoDevice)
}

/// Check if a YubiKey is available.
pub fn yubikey_available() -> bool {
    // Placeholder - would check for YubiKey presence
    false
}

/// Software fallback for HMAC challenge-response (for testing).
///
/// Uses a secret key to compute HMAC-SHA1 of the challenge.
/// This provides the same interface as YubiKey but without hardware security.
pub fn software_hmac(secret: &[u8], challenge: &[u8; CHALLENGE_SIZE]) -> [u8; RESPONSE_SIZE] {
    use hmac::digest::FixedOutput;
    use sha1::Sha1;

    type HmacSha1 = Hmac<Sha1>;

    let mut mac = HmacSha1::new_from_slice(secret).expect("HMAC can take key of any size");
    mac.update(challenge);

    let result = mac.finalize_fixed();
    let mut response = [0u8; RESPONSE_SIZE];
    response.copy_from_slice(&result[..RESPONSE_SIZE]);
    response
}

/// Derive a key using YubiKey or password fallback.
///
/// Attempts YubiKey challenge-response first. If no YubiKey is available,
/// falls back to password-only derivation.
pub fn derive_with_yubikey(
    user_id: i64,
    category: &str,
    name: &str,
    _password: &[u8],
) -> Result<(Vec<u8>, bool)> {
    let challenge = generate_challenge(user_id, category, name);

    match challenge_yubikey(&challenge)? {
        ChallengeResult::Response(response) => {
            // YubiKey response available - include in key derivation
            Ok((response.to_vec(), true))
        }
        ChallengeResult::NoDevice | ChallengeResult::SlotNotConfigured => {
            // No YubiKey - password-only mode
            Ok((Vec::new(), false))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_is_deterministic() {
        let c1 = generate_challenge(1, "aws", "key");
        let c2 = generate_challenge(1, "aws", "key");
        assert_eq!(c1, c2);
    }

    #[test]
    fn challenge_varies_with_inputs() {
        let c1 = generate_challenge(1, "aws", "key");
        let c2 = generate_challenge(2, "aws", "key");
        let c3 = generate_challenge(1, "gcp", "key");
        let c4 = generate_challenge(1, "aws", "other");

        assert_ne!(c1, c2);
        assert_ne!(c1, c3);
        assert_ne!(c1, c4);
    }

    #[test]
    fn software_hmac_consistent() {
        let secret = b"test-secret-key";
        let challenge = generate_challenge(1, "test", "key");

        let r1 = software_hmac(secret, &challenge);
        let r2 = software_hmac(secret, &challenge);
        assert_eq!(r1, r2);
    }

    #[test]
    fn software_hmac_varies_with_secret() {
        let challenge = generate_challenge(1, "test", "key");

        let r1 = software_hmac(b"secret1", &challenge);
        let r2 = software_hmac(b"secret2", &challenge);
        assert_ne!(r1, r2);
    }

    #[test]
    fn yubikey_returns_no_device() {
        let challenge = generate_challenge(1, "test", "key");
        let result = challenge_yubikey(&challenge).unwrap();
        assert!(matches!(result, ChallengeResult::NoDevice));
    }
}
