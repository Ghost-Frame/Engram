//! AES-256-GCM encryption for secrets.

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use argon2::{Algorithm, Argon2, Params, Version};
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::{CredError, Result, SecretData};

/// Nonce size for AES-256-GCM (96 bits = 12 bytes).
pub const NONCE_SIZE: usize = 12;

/// Key size for AES-256-GCM (256 bits = 32 bytes).
pub const KEY_SIZE: usize = 32;

/// Argon2id memory parameter in KiB. 64 MiB resists GPU brute force while
/// staying under the smallest container footprint we ship.
const ARGON2_MEMORY_KIB: u32 = 65536;

/// Argon2id iteration count. 3 passes is the OWASP 2023 recommendation for
/// the 64 MiB memory class.
const ARGON2_ITERATIONS: u32 = 3;

/// Argon2id parallelism. Single lane keeps WASM and small-container targets
/// viable; the memory cost already dominates.
const ARGON2_PARALLELISM: u32 = 1;

/// Domain separation string mixed into the deterministic salt. Changing this
/// invalidates every ciphertext in the cred database.
const KDF_DOMAIN: &[u8] = b"engram-cred-kdf-v1";

/// Derive an encryption key from a password and optional YubiKey response.
///
/// Inputs are bound with Argon2id using a deterministic 16-byte salt derived
/// from `user_id` and a fixed domain separation tag. The salt is deterministic
/// because this function must return the same key for the same inputs on every
/// call; per-user isolation comes from `user_id` being mixed into both the salt
/// and the password material.
///
/// Parameters: m = 64 MiB, t = 3, p = 1, output = 32 bytes (OWASP 2023).
pub fn derive_key(user_id: i64, password: &[u8], yubikey_response: Option<&[u8]>) -> [u8; KEY_SIZE] {
    // Deterministic 16-byte salt: SHA-256(domain || user_id) truncated.
    let mut salt_hasher = Sha256::new();
    salt_hasher.update(KDF_DOMAIN);
    salt_hasher.update(user_id.to_le_bytes());
    let salt_digest = salt_hasher.finalize();
    let salt = &salt_digest[..16];

    // Password material: user_id || password || yubikey_response.
    let mut material = Vec::with_capacity(
        8 + password.len() + yubikey_response.map(|r| r.len()).unwrap_or(0),
    );
    material.extend_from_slice(&user_id.to_le_bytes());
    material.extend_from_slice(password);
    if let Some(response) = yubikey_response {
        material.extend_from_slice(response);
    }

    let params = Params::new(
        ARGON2_MEMORY_KIB,
        ARGON2_ITERATIONS,
        ARGON2_PARALLELISM,
        Some(KEY_SIZE),
    )
    .expect("argon2 params within library bounds");
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

    let mut key = [0u8; KEY_SIZE];
    argon2
        .hash_password_into(&material, salt, &mut key)
        .expect("argon2id derivation never fails with validated params");
    key
}

/// Encrypt secret data with AES-256-GCM.
///
/// Returns (encrypted_data, nonce).
pub fn encrypt_secret(key: &[u8; KEY_SIZE], data: &SecretData) -> Result<(Vec<u8>, [u8; NONCE_SIZE])> {
    let plaintext =
        serde_json::to_vec(data).map_err(|e| CredError::Encryption(e.to_string()))?;

    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| CredError::Encryption(format!("invalid key: {}", e)))?;

    // Generate random nonce
    let mut nonce_bytes = [0u8; NONCE_SIZE];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_ref())
        .map_err(|e| CredError::Encryption(format!("encryption failed: {}", e)))?;

    Ok((ciphertext, nonce_bytes))
}

/// Decrypt secret data with AES-256-GCM.
pub fn decrypt_secret(
    key: &[u8; KEY_SIZE],
    encrypted_data: &[u8],
    nonce: &[u8; NONCE_SIZE],
) -> Result<SecretData> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| CredError::Decryption(format!("invalid key: {}", e)))?;

    let nonce = Nonce::from_slice(nonce);

    let plaintext = cipher
        .decrypt(nonce, encrypted_data)
        .map_err(|e| CredError::Decryption(format!("decryption failed: {}", e)))?;

    serde_json::from_slice(&plaintext).map_err(|e| CredError::Decryption(e.to_string()))
}

/// Generate a random 256-bit key.
pub fn generate_random_key() -> [u8; KEY_SIZE] {
    let mut key = [0u8; KEY_SIZE];
    rand::thread_rng().fill_bytes(&mut key);
    key
}

/// Hash a key for storage (used for agent key hashes).
pub fn hash_key(key: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SecretData;

    #[test]
    fn derive_key_deterministic() {
        let key1 = derive_key(1, b"password123", None);
        let key2 = derive_key(1, b"password123", None);
        assert_eq!(key1, key2);
    }

    #[test]
    fn derive_key_varies_with_user() {
        let key1 = derive_key(1, b"password", None);
        let key2 = derive_key(2, b"password", None);
        assert_ne!(key1, key2);
    }

    #[test]
    fn derive_key_varies_with_password() {
        let key1 = derive_key(1, b"password1", None);
        let key2 = derive_key(1, b"password2", None);
        assert_ne!(key1, key2);
    }

    #[test]
    fn derive_key_varies_with_yubikey() {
        let key1 = derive_key(1, b"password", None);
        let key2 = derive_key(1, b"password", Some(b"yubikey-response"));
        assert_ne!(key1, key2);
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key = derive_key(1, b"test-password", None);
        let secret = SecretData::ApiKey {
            key: "super-secret-api-key".into(),
            endpoint: Some("https://api.example.com".into()),
            notes: None,
        };

        let (encrypted, nonce) = encrypt_secret(&key, &secret).unwrap();
        let decrypted = decrypt_secret(&key, &encrypted, &nonce).unwrap();

        match decrypted {
            SecretData::ApiKey {
                key: k,
                endpoint,
                notes,
            } => {
                assert_eq!(k, "super-secret-api-key");
                assert_eq!(endpoint, Some("https://api.example.com".into()));
                assert_eq!(notes, None);
            }
            _ => panic!("wrong type"),
        }
    }

    #[test]
    fn wrong_key_fails_decryption() {
        let key1 = derive_key(1, b"correct-password", None);
        let key2 = derive_key(1, b"wrong-password", None);
        let secret = SecretData::Note {
            content: "secret note".into(),
        };

        let (encrypted, nonce) = encrypt_secret(&key1, &secret).unwrap();
        let result = decrypt_secret(&key2, &encrypted, &nonce);
        assert!(result.is_err());
    }

    #[test]
    fn wrong_nonce_fails_decryption() {
        let key = derive_key(1, b"password", None);
        let secret = SecretData::Note {
            content: "secret".into(),
        };

        let (encrypted, _nonce) = encrypt_secret(&key, &secret).unwrap();
        let wrong_nonce = [0u8; NONCE_SIZE];
        let result = decrypt_secret(&key, &encrypted, &wrong_nonce);
        assert!(result.is_err());
    }

    #[test]
    fn hash_key_consistent() {
        let key = b"test-key-data";
        let hash1 = hash_key(key);
        let hash2 = hash_key(key);
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64); // SHA-256 hex = 64 chars
    }
}
