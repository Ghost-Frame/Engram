//! At-rest database encryption key resolution.
//!
//! Shared by the `cred` CLI and the credd/phylaxd daemon so both derive a
//! byte-identical SQLCipher key for the same host configuration. The per-secret
//! `master_key` (which encrypts individual `encrypted_data` values) is a
//! separate key; this module only resolves the whole-file SQLCipher key.

use kleos_lib::config::{Config, EncryptionMode};

use crate::crypto::derive_key;

/// Resolve the optional 32-byte at-rest (SQLCipher) key for the configured
/// `EncryptionMode`.
///
/// Returns `Ok(None)` when encryption is disabled, which the caller passes
/// straight to `Database::connect_encrypted` for a plaintext open (backward
/// compatible with hosts that have not opted in).
///
/// `precomputed_response` lets a yubikey-auth caller reuse the slot-2
/// challenge-response it already obtained for the per-secret master key, so a
/// single `cred` invocation performs at most one YubiKey HMAC and does not add
/// device contention.
pub fn resolve_at_rest_key(
    config: &Config,
    precomputed_response: Option<&[u8]>,
) -> anyhow::Result<Option<[u8; 32]>> {
    match config.encryption.mode {
        // No at-rest encryption: open the database in plaintext.
        EncryptionMode::None => Ok(None),
        // YubiKey: derive the SQLCipher key from the slot-2 HMAC response,
        // reusing a precomputed response when the caller already has one.
        EncryptionMode::Yubikey => {
            tracing::info!("at-rest encryption mode: yubikey");
            // The response either comes from the caller (single shared HMAC) or
            // we perform our own challenge-response here.
            let derived = match precomputed_response {
                Some(response) => derive_key(0, b"", Some(response)),
                None => {
                    let challenge = crate::yubikey::get_or_create_challenge()
                        .map_err(|e| anyhow::anyhow!("YubiKey challenge: {e}"))?;
                    let response = crate::yubikey::challenge_response(&challenge)
                        .map_err(|e| anyhow::anyhow!("YubiKey response: {e}"))?;
                    derive_key(0, b"", Some(&response))
                }
            };
            // Copy out of the Zeroizing wrapper into the fixed array the DB layer expects.
            let mut key = [0u8; 32];
            key.copy_from_slice(&derived[..]);
            Ok(Some(key))
        }
        // Keyfile / Env: defer to the shared kleos_lib resolver (no YubiKey).
        _ => kleos_lib::encryption::resolve_key(config)
            .map_err(|e| anyhow::anyhow!("encryption key: {e}")),
    }
}
