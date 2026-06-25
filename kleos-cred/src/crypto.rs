//! AES-256-GCM encryption for secrets.

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use argon2::{Algorithm, Argon2, Params, Version};
use rand::rngs::OsRng;
use rand::TryRngCore;
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

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

/// Environment variable naming a file that holds a persisted random KDF salt
/// (32 hex chars = 16 bytes). When set and the file is valid, password-based
/// key derivation (`derive_key`) uses this salt (KDF v2) instead of the legacy
/// deterministic per-user salt, so two deployments that share a password no
/// longer derive the same master key.
///
/// Backward compatible: when unset or unreadable, derivation falls back to the
/// deterministic salt, so existing vaults keep opening. It MUST be set
/// identically across every cred binary that shares a vault (cred, credd,
/// phylaxd); a mismatch derives a different key and cannot decrypt.
pub const KDF_SALT_FILE_ENV: &str = "CRED_KDF_SALT_FILE";

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
#[deprecated(since = "1.0.0", note = "use derive_key with modern KDF parameters")]
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
/// Inputs are bound with Argon2id. The salt is the persisted random salt named
/// by `CRED_KDF_SALT_FILE` (KDF v2) when one is configured, otherwise a
/// deterministic 16-byte salt derived from `user_id` and a fixed domain
/// separation tag (KDF v1, the backward-compatible default). The v1 salt is
/// deterministic so the same inputs derive the same key on every call; per-user
/// isolation comes from `user_id` being mixed into both the salt and the
/// password material. See `resolve_kdf_salt`.
///
/// Parameters: m = 64 MiB, t = 3, p = 1, output = 32 bytes (OWASP 2023).
///
/// Returns a `Zeroizing` wrapper so the key material is erased from memory
/// when the value is dropped.
pub fn derive_key(
    user_id: i64,
    password: &[u8],
    yubikey_response: Option<&[u8]>,
) -> Zeroizing<[u8; KEY_SIZE]> {
    // KDF salt: a persisted random salt (v2) when CRED_KDF_SALT_FILE names a
    // valid one, otherwise the legacy deterministic salt derived from user_id
    // (v1). See resolve_kdf_salt.
    let salt = resolve_kdf_salt(user_id);
    derive_key_with_salt(user_id, password, yubikey_response, &salt)
}

/// Derive a key from the same material as [`derive_key`] but with an explicit
/// salt, bypassing salt resolution. Pure (no env / file access) so callers and
/// tests can pin the salt directly.
pub fn derive_key_with_salt(
    user_id: i64,
    password: &[u8],
    yubikey_response: Option<&[u8]>,
    salt: &[u8],
) -> Zeroizing<[u8; KEY_SIZE]> {
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

    Zeroizing::new(key)
}

/// The legacy deterministic 16-byte salt for a user: SHA-256(domain || user_id)
/// truncated. Used when no persisted random salt is configured.
fn legacy_deterministic_salt(user_id: i64) -> [u8; SALT_SIZE] {
    let mut hasher = Sha256::new();
    hasher.update(KDF_DOMAIN);
    hasher.update(user_id.to_le_bytes());
    let digest = hasher.finalize();
    let mut salt = [0u8; SALT_SIZE];
    salt.copy_from_slice(&digest[..SALT_SIZE]);
    salt
}

/// Read a persisted KDF salt (32 hex chars = 16 bytes) from `path`. Returns
/// `None` if the file is missing, unreadable, or not exactly 16 bytes of hex.
/// A salt is not secret, so it is stored in plaintext hex.
fn read_salt_file(path: &str) -> Option<[u8; SALT_SIZE]> {
    let content = std::fs::read_to_string(path).ok()?;
    let bytes = hex::decode(content.trim()).ok()?;
    if bytes.len() != SALT_SIZE {
        return None;
    }
    let mut salt = [0u8; SALT_SIZE];
    salt.copy_from_slice(&bytes);
    Some(salt)
}

/// Resolve the KDF salt for `user_id`. Prefers a persisted random salt named by
/// the CRED_KDF_SALT_FILE env var (KDF v2); falls back to the legacy
/// deterministic salt (KDF v1) when that is unset or invalid, so existing
/// vaults continue to derive the same key.
fn resolve_kdf_salt(user_id: i64) -> [u8; SALT_SIZE] {
    if let Ok(path) = std::env::var(KDF_SALT_FILE_ENV) {
        if !path.is_empty() {
            if let Some(salt) = read_salt_file(&path) {
                return salt;
            }
        }
    }
    legacy_deterministic_salt(user_id)
}

/// Create a random KDF salt file at `path` (mode 0600 on Unix) if it does not
/// already exist, returning the salt as hex. If the file already exists it is
/// left untouched and its current value returned -- overwriting would orphan
/// every secret already derived with the existing salt.
pub fn init_kdf_salt_file(path: &std::path::Path) -> Result<String> {
    if let Some(existing) = path.to_str().and_then(read_salt_file) {
        return Ok(hex::encode(existing));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CredError::Encryption(format!("salt dir: {e}")))?;
    }
    let mut salt = [0u8; SALT_SIZE];
    OsRng
        .try_fill_bytes(&mut salt)
        .expect("OS CSPRNG must be available");
    let hex_salt = hex::encode(salt);

    // Write atomically with owner-only perms (the salt is not secret, but
    // tight perms avoid a foot-gun where another user swaps it).
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, format!("{hex_salt}\n"))
        .map_err(|e| CredError::Encryption(format!("write salt: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| CredError::Encryption(format!("chmod salt: {e}")))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| CredError::Encryption(format!("rename salt: {e}")))?;
    Ok(hex_salt)
}

/// Encrypt secret data with AES-256-GCM.
///
/// Returns (encrypted_data, nonce).
pub fn encrypt_secret(
    key: &[u8; KEY_SIZE],
    data: &SecretData,
) -> Result<(Vec<u8>, [u8; NONCE_SIZE])> {
    // Serialized secret plaintext; wrap in Zeroizing so it is scrubbed from
    // the heap on drop rather than left recoverable via core dump or swap.
    let plaintext =
        Zeroizing::new(serde_json::to_vec(data).map_err(|e| CredError::Encryption(e.to_string()))?);

    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| CredError::Encryption(format!("invalid key: {}", e)))?;

    // Generate random nonce
    let mut nonce_bytes = [0u8; NONCE_SIZE];
    OsRng
        .try_fill_bytes(&mut nonce_bytes)
        .expect("OS CSPRNG must be available");
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_slice())
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
    OsRng
        .try_fill_bytes(&mut nonce_bytes)
        .expect("OS CSPRNG must be available");
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

    // Decrypted secret plaintext; wrap in Zeroizing so it is scrubbed from the
    // heap on drop once parsed into the returned SecretData.
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(nonce, encrypted_data)
            .map_err(|e| CredError::Decryption(format!("decryption failed: {}", e)))?,
    );

    serde_json::from_slice(&plaintext[..]).map_err(|e| CredError::Decryption(e.to_string()))
}

/// Generate a random 256-bit key.
pub fn generate_random_key() -> [u8; KEY_SIZE] {
    let mut key = [0u8; KEY_SIZE];
    OsRng
        .try_fill_bytes(&mut key)
        .expect("OS CSPRNG must be available");
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
    OsRng
        .try_fill_bytes(&mut salt)
        .expect("OS CSPRNG must be available");

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
    OsRng
        .try_fill_bytes(&mut secret)
        .expect("OS CSPRNG must be available");
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

    // --- KDF v2 (persisted random salt) --- These tests stay pure: they never
    // set CRED_KDF_SALT_FILE, so the default-path tests above remain stable.

    #[test]
    fn legacy_salt_is_deterministic_and_user_scoped() {
        assert_eq!(legacy_deterministic_salt(1), legacy_deterministic_salt(1));
        assert_ne!(legacy_deterministic_salt(1), legacy_deterministic_salt(2));
    }

    #[test]
    fn derive_key_default_matches_legacy_salt() {
        // With no salt file configured, derive_key must equal an explicit
        // derivation using the legacy deterministic salt (backward compat).
        let salt = legacy_deterministic_salt(1);
        let via_default = derive_key(1, b"pw", None);
        let via_explicit = derive_key_with_salt(1, b"pw", None, &salt);
        assert_eq!(via_default, via_explicit);
    }

    #[test]
    fn different_salts_diverge_same_password() {
        // The core of the fix: two deployments with the same password but
        // different (random) salts derive different keys.
        let key_a = derive_key_with_salt(1, b"shared-pw", None, &[0xAA; SALT_SIZE]);
        let key_b = derive_key_with_salt(1, b"shared-pw", None, &[0xBB; SALT_SIZE]);
        assert_ne!(key_a, key_b);
        // Same salt -> same key.
        let key_a2 = derive_key_with_salt(1, b"shared-pw", None, &[0xAA; SALT_SIZE]);
        assert_eq!(key_a, key_a2);
    }

    #[test]
    fn init_kdf_salt_file_creates_idempotent_readable_salt() {
        let path = std::env::temp_dir().join(format!("cred-kdf-test-{}.salt", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let hex1 = init_kdf_salt_file(&path).expect("create salt");
        assert_eq!(hex1.len(), SALT_SIZE * 2); // 16 bytes -> 32 hex chars

        // Idempotent: a second call returns the same salt, never overwrites.
        let hex2 = init_kdf_salt_file(&path).expect("reuse salt");
        assert_eq!(hex1, hex2);

        // read_salt_file round-trips what was written.
        let read = read_salt_file(path.to_str().unwrap()).expect("read salt");
        assert_eq!(hex::encode(read), hex1);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_salt_file_rejects_bad_input() {
        let path = std::env::temp_dir().join(format!("cred-kdf-bad-{}.salt", std::process::id()));
        std::fs::write(&path, "not-hex").unwrap();
        assert!(read_salt_file(path.to_str().unwrap()).is_none());
        std::fs::write(&path, "aabb").unwrap(); // valid hex but wrong length
        assert!(read_salt_file(path.to_str().unwrap()).is_none());
        let _ = std::fs::remove_file(&path);
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

    #[allow(deprecated)]
    #[test]
    fn derive_key_legacy_deterministic() {
        let response = b"yubikey-hmac-response-20-bytes!";
        let key1 = derive_key_legacy(response);
        let key2 = derive_key_legacy(response);
        assert_eq!(key1, key2);
    }

    #[allow(deprecated)]
    #[test]
    fn derive_key_legacy_differs_from_new() {
        // Legacy and new KDF must produce different keys (different params/salt)
        let response = b"yubikey-hmac-response";
        let legacy = derive_key_legacy(response);
        let new = derive_key(0, b"", Some(response));
        assert_ne!(legacy, *new);
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

    #[test]
    fn tamper_detection_ciphertext_byte_flip() {
        let key = generate_random_key();
        let plaintext = b"tamper detection test";

        let mut ciphertext = encrypt(&key, plaintext).unwrap();
        // Flip a byte in the ciphertext portion (after the nonce)
        ciphertext[NONCE_SIZE] ^= 0xff;

        let result = decrypt(&key, &ciphertext);
        assert!(
            result.is_err(),
            "decryption must fail when ciphertext is tampered"
        );
    }

    #[test]
    fn tamper_detection_nonce_byte_flip() {
        let key = generate_random_key();
        let plaintext = b"nonce tamper test";

        let mut ciphertext = encrypt(&key, plaintext).unwrap();
        // Flip a byte in the nonce portion
        ciphertext[0] ^= 0xff;

        let result = decrypt(&key, &ciphertext);
        assert!(
            result.is_err(),
            "decryption must fail when nonce is tampered"
        );
    }

    #[test]
    fn zero_length_plaintext_roundtrip() {
        let key = generate_random_key();
        let plaintext: &[u8] = b"";

        let ciphertext = encrypt(&key, plaintext).unwrap();
        let decrypted = decrypt(&key, &ciphertext).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn derive_key_from_passphrase_deterministic() {
        let salt = [0x42u8; SALT_SIZE];
        let key1 = derive_key_from_passphrase("my-passphrase", &salt).unwrap();
        let key2 = derive_key_from_passphrase("my-passphrase", &salt).unwrap();
        assert_eq!(key1, key2);
    }

    #[test]
    fn derive_key_from_passphrase_differs_with_different_salt() {
        let salt1 = [0x11u8; SALT_SIZE];
        let salt2 = [0x22u8; SALT_SIZE];
        let key1 = derive_key_from_passphrase("my-passphrase", &salt1).unwrap();
        let key2 = derive_key_from_passphrase("my-passphrase", &salt2).unwrap();
        assert_ne!(key1, key2);
    }
}
