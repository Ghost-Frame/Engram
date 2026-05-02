//! AES-256-GCM encryption for secrets.

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use argon2::{Algorithm, Argon2, Params, Version};
use rand::Rng;
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::{CredError, Result, SecretData};

/// Nonce size for AES-256-GCM (96 bits = 12 bytes).
pub const NONCE_SIZE: usize = 12;

/// Key size for AES-256-GCM (256 bits = 32 bytes).
pub const KEY_SIZE: usize = 32;

/// Salt size for Argon2id key derivation (16 bytes).
pub const SALT_SIZE: usize = 16;

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

/// Legacy salt for private cred compatibility (single-user mode).
const LEGACY_SALT: &[u8] = b"cred-yubikey-v1\0";

/// Legacy argon2 params matching private cred: m=19MiB, t=2, p=1.
const LEGACY_ARGON2_MEMORY_KIB: u32 = 19 * 1024;
const LEGACY_ARGON2_ITERATIONS: u32 = 2;

/// Derive an encryption key compatible with private cred (single-user YubiKey-only mode).
///
/// This uses the exact same KDF parameters as private cred for backwards compatibility:
/// - Salt: fixed "cred-yubikey-v1\0"
/// - Input: just the YubiKey HMAC response
/// - Params: m=19MiB, t=2, p=1, output=32 bytes
///
/// SECURITY (SEC-LOW-7): this function is ONLY for YubiKey-based single-user
/// mode (private cred migration path). It MUST NOT be used for new password-
/// based key derivation -- use [`derive_key`] instead, which mixes in a
/// per-user deterministic salt and uses stronger params.
pub fn derive_key_legacy(yubikey_response: &[u8]) -> [u8; KEY_SIZE] {
    let params = Params::new(
        LEGACY_ARGON2_MEMORY_KIB,
        LEGACY_ARGON2_ITERATIONS,
        ARGON2_PARALLELISM,
        Some(KEY_SIZE),
    )
    .expect("argon2 params within library bounds");
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

    let mut key = [0u8; KEY_SIZE];
    argon2
        .hash_password_into(yubikey_response, LEGACY_SALT, &mut key)
        .expect("argon2id derivation never fails with validated params");
    key
}

/// Derive an encryption key from a password and optional YubiKey response.
///
/// Inputs are bound with Argon2id using a deterministic 16-byte salt derived
/// from `user_id` and a fixed domain separation tag. The salt is deterministic
/// because this function must return the same key for the same inputs on every
/// call; per-user isolation comes from `user_id` being mixed into both the salt
/// and the password material.
///
/// Parameters: m = 64 MiB, t = 3, p = 1, output = 32 bytes (OWASP 2023).
pub fn derive_key(
    user_id: i64,
    password: &[u8],
    yubikey_response: Option<&[u8]>,
) -> [u8; KEY_SIZE] {
    // Deterministic 16-byte salt: SHA-256(domain || user_id) truncated.
    let mut salt_hasher = Sha256::new();
    salt_hasher.update(KDF_DOMAIN);
    salt_hasher.update(user_id.to_le_bytes());
    let salt_digest = salt_hasher.finalize();
    let salt = &salt_digest[..16];

    // Password material: user_id || password || yubikey_response.
    let mut material =
        Vec::with_capacity(8 + password.len() + yubikey_response.map(|r| r.len()).unwrap_or(0));
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

    // SECURITY (SEC-H5): zeroize password material from heap memory to prevent
    // recovery via core dump, /proc/self/mem, or swap.
    material.zeroize();

    key
}

/// Encrypt secret data with AES-256-GCM.
///
/// Returns (encrypted_data, nonce).
pub fn encrypt_secret(
    key: &[u8; KEY_SIZE],
    data: &SecretData,
) -> Result<(Vec<u8>, [u8; NONCE_SIZE])> {
    let plaintext = serde_json::to_vec(data).map_err(|e| CredError::Encryption(e.to_string()))?;

    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| CredError::Encryption(format!("invalid key: {}", e)))?;

    // Generate random nonce
    let mut nonce_bytes = [0u8; NONCE_SIZE];
    rand::rng().fill(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_ref())
        .map_err(|e| CredError::Encryption(format!("encryption failed: {}", e)))?;

    Ok((ciphertext, nonce_bytes))
}

/// Encrypt raw bytes with AES-256-GCM.
///
/// Returns: nonce (12 bytes) || ciphertext+tag.
pub fn encrypt(key: &[u8; KEY_SIZE], plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| CredError::Encryption(format!("invalid key: {}", e)))?;

    let mut nonce_bytes = [0u8; NONCE_SIZE];
    rand::rng().fill(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| CredError::Encryption(format!("encryption failed: {}", e)))?;

    let mut output = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&ciphertext);
    Ok(output)
}

/// Decrypt AES-256-GCM ciphertext.
///
/// Input format: nonce (12 bytes) || ciphertext+tag.
pub fn decrypt(key: &[u8; KEY_SIZE], data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < NONCE_SIZE + 16 {
        return Err(CredError::Decryption("ciphertext too short".into()));
    }

    let cipher = Aes256Gcm::new_from_slice(&key[..])
        .map_err(|e| CredError::Decryption(format!("invalid key: {}", e)))?;

    let nonce = Nonce::from_slice(&data[..NONCE_SIZE]);
    let ciphertext = &data[NONCE_SIZE..];

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| CredError::Decryption(format!("decryption failed: {}", e)))
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
    rand::rng().fill(&mut key);
    key
}

/// Hash a key for storage (used for agent key hashes).
pub fn hash_key(key: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key);
    hex::encode(hasher.finalize())
}

/// Derive a 256-bit AES key from a passphrase and random salt.
///
/// Uses the modern Argon2id parameters (64 MiB, 3 iterations).
/// The salt must be stored alongside the ciphertext.
pub fn derive_key_from_passphrase(passphrase: &str, salt: &[u8]) -> Result<[u8; KEY_SIZE]> {
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
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| CredError::Encryption(format!("passphrase key derivation failed: {}", e)))?;

    Ok(key)
}

/// Encrypt the HMAC secret for the recovery file.
///
/// Format: salt (16 bytes) || nonce (12 bytes) || ciphertext+tag.
pub fn encrypt_recovery(passphrase: &str, hmac_secret: &[u8]) -> Result<Vec<u8>> {
    let mut salt = [0u8; SALT_SIZE];
    rand::rng().fill(&mut salt);

    let key = derive_key_from_passphrase(passphrase, &salt)?;
    let encrypted = encrypt(&key, hmac_secret)?;

    let mut output = Vec::with_capacity(SALT_SIZE + encrypted.len());
    output.extend_from_slice(&salt);
    output.extend_from_slice(&encrypted);
    Ok(output)
}

/// Decrypt the HMAC secret from a recovery file.
///
/// Input format: salt (16 bytes) || nonce (12 bytes) || ciphertext+tag.
pub fn decrypt_recovery(passphrase: &str, data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < SALT_SIZE + NONCE_SIZE + 16 {
        return Err(CredError::Decryption(
            "recovery file too short or corrupted".into(),
        ));
    }

    let salt = &data[..SALT_SIZE];
    let encrypted = &data[SALT_SIZE..];

    let key = derive_key_from_passphrase(passphrase, salt)?;
    decrypt(&key, encrypted)
}

/// Generate a random 20-byte HMAC-SHA1 secret for YubiKey programming.
pub fn generate_hmac_secret() -> [u8; 20] {
    let mut secret = [0u8; 20];
    rand::rng().fill(&mut secret);
    secret
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

    #[test]
    fn derive_key_legacy_deterministic() {
        let response = b"yubikey-hmac-response-20-bytes!";
        let key1 = derive_key_legacy(response);
        let key2 = derive_key_legacy(response);
        assert_eq!(key1, key2);
    }

    #[test]
    fn derive_key_legacy_differs_from_new() {
        // Legacy and new KDF must produce different keys (different params/salt)
        let response = b"yubikey-hmac-response";
        let legacy = derive_key_legacy(response);
        let new = derive_key(0, b"", Some(response));
        assert_ne!(legacy, new);
    }

    #[test]
    fn encrypt_decrypt_raw_roundtrip() {
        let key = generate_random_key();
        let plaintext = b"raw plaintext data for testing";

        let ciphertext = encrypt(&key, plaintext).unwrap();
        let decrypted = decrypt(&key, &ciphertext).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn encrypt_decrypt_raw_wrong_key_fails() {
        let key1 = generate_random_key();
        let key2 = generate_random_key();
        let plaintext = b"some secret data";

        let ciphertext = encrypt(&key1, plaintext).unwrap();
        let result = decrypt(&key2, &ciphertext);
        assert!(result.is_err());
    }

    #[test]
    fn decrypt_too_short_fails() {
        let key = generate_random_key();
        let short_data = [0u8; 10];
        let result = decrypt(&key, &short_data);
        assert!(result.is_err());
    }

    #[test]
    fn encrypt_recovery_decrypt_recovery_roundtrip() {
        let passphrase = "correct-horse-battery-staple";
        let hmac_secret = b"20-byte-hmac-secret!";

        let encrypted = encrypt_recovery(passphrase, hmac_secret).unwrap();
        let decrypted = decrypt_recovery(passphrase, &encrypted).unwrap();
        assert_eq!(decrypted, hmac_secret);
    }

    #[test]
    fn decrypt_recovery_wrong_passphrase_fails() {
        let hmac_secret = b"20-byte-hmac-secret!";

        let encrypted = encrypt_recovery("correct-passphrase", hmac_secret).unwrap();
        let result = decrypt_recovery("wrong-passphrase", &encrypted);
        assert!(result.is_err());
    }

    #[test]
    fn generate_hmac_secret_length() {
        let secret = generate_hmac_secret();
        assert_eq!(secret.len(), 20);
    }

    #[test]
    fn generate_hmac_secret_random() {
        let s1 = generate_hmac_secret();
        let s2 = generate_hmac_secret();
        // Astronomically unlikely to be equal
        assert_ne!(s1, s2);
    }
}
