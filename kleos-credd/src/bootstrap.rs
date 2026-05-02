//! YubiKey-encrypted bootstrap blob loader.
//!
//! `bootstrap.enc` lives at `$XDG_CONFIG_HOME/cred/bootstrap.enc` (or the
//! path in `$CREDD_BOOTSTRAP_BLOB`). It is an AES-256-GCM blob encrypted
//! with credd's YubiKey-derived master key. The plaintext is a small JSON
//! header followed by an ASCII record-separator byte (0x1E) and then the
//! bare per-host Kleos bearer that credd uses when it needs to talk to
//! Kleos on its own behalf (e.g. to fetch per-agent bearers for the
//! `/bootstrap/kleos-bearer` endpoint).
//!
//! The blob is created via `cred bootstrap wrap engram-rust credd-<host>`
//! once per host and only ever decrypted in credd's process memory. Nothing
//! plaintext lands on disk.
//!
//! On-disk format:
//!
//! ```text
//! +--------+-------------------------------------------------+
//! | "CBv1" |  AES-256-GCM(master_key, plaintext)             |
//! |  4B    |  layout: nonce(12B) || ciphertext+tag           |
//! +--------+-------------------------------------------------+
//! plaintext = <JSON header bytes> || 0x1E || <bare bearer bytes>
//! ```

use std::path::PathBuf;

use anyhow::{Context, Result};
use kleos_cred::crypto::{decrypt, KEY_SIZE};
use tracing::warn;
use zeroize::Zeroizing;

/// Magic bytes prepended to a bootstrap blob. Wrong magic = refuse to start.
pub const BOOTSTRAP_MAGIC: &[u8; 4] = b"CBv1";

/// ASCII record separator that splits the JSON header from the bare bearer.
pub const HEADER_KEY_SEPARATOR: u8 = 0x1E;

/// Resolve the path to `bootstrap.enc`. `CREDD_BOOTSTRAP_BLOB` env wins;
/// otherwise `$XDG_CONFIG_HOME/cred/bootstrap.enc` (XDG-resolved).
pub fn blob_path() -> PathBuf {
    if let Ok(p) = std::env::var("CREDD_BOOTSTRAP_BLOB") {
        return PathBuf::from(p);
    }

    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".config"))
                .unwrap_or_else(|_| PathBuf::from("."))
        });
    base.join("cred").join("bootstrap.enc")
}

/// Load and decrypt `bootstrap.enc` using the already-derived `master_key`.
///
/// Returns:
///   * `Ok(Some(bearer))` if the blob exists and decrypts cleanly.
///   * `Ok(None)` if no blob is present (non-fatal: credd serves everything
///     except `/bootstrap/kleos-bearer`, which will return 404).
///   * `Err(_)` if a blob exists but is malformed or decrypts wrong; credd
///     refuses to start because the operator's intent is unclear.
pub async fn load_bootstrap_blob(master_key: &[u8; KEY_SIZE]) -> Result<Option<Zeroizing<String>>> {
    let path = blob_path();

    if !path.exists() {
        warn!(
            "no bootstrap.enc at {} -- /bootstrap/kleos-bearer will return 404",
            path.display()
        );
        return Ok(None);
    }

    let data = tokio::fs::read(&path)
        .await
        .with_context(|| format!("failed to read {}", path.display()))?;

    if data.len() < BOOTSTRAP_MAGIC.len() || &data[..BOOTSTRAP_MAGIC.len()] != BOOTSTRAP_MAGIC {
        anyhow::bail!(
            "bootstrap.enc has wrong magic (got {:?}); refusing to start",
            &data[..BOOTSTRAP_MAGIC.len().min(data.len())]
        );
    }

    let plaintext = decrypt(master_key, &data[BOOTSTRAP_MAGIC.len()..])
        .context("bootstrap.enc decryption failed (wrong YubiKey or corrupted blob)")?;

    let sep_pos = plaintext
        .iter()
        .position(|&b| b == HEADER_KEY_SEPARATOR)
        .ok_or_else(|| anyhow::anyhow!("bootstrap.enc payload missing 0x1E separator"))?;

    let key_bytes = &plaintext[sep_pos + 1..];
    let key_str = std::str::from_utf8(key_bytes)
        .context("bootstrap.enc bearer bytes are not valid UTF-8")?
        .to_string();

    Ok(Some(Zeroizing::new(key_str)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kleos_cred::crypto::{derive_key_legacy, encrypt};
    use tokio::sync::Mutex;

    // Tests mutate process-global CREDD_BOOTSTRAP_BLOB; serialize them so
    // parallel runners don't observe each other's env state.
    // Uses tokio::sync::Mutex so the guard is Send -- no await_holding_lock lint.
    static ENV_GUARD: Mutex<()> = Mutex::const_new(());

    fn test_key() -> [u8; KEY_SIZE] {
        // Deterministic test key: legacy KDF over a fixed pseudo-YubiKey response.
        derive_key_legacy(b"01234567890123456789")
    }

    #[tokio::test]
    async fn missing_blob_returns_none() {
        let _g = ENV_GUARD.lock().await;
        // Use a path guaranteed not to exist on any test runner.
        std::env::set_var(
            "CREDD_BOOTSTRAP_BLOB",
            "/dev/null/nonexistent-credd-blob/bootstrap.enc",
        );
        let key = test_key();
        let result = load_bootstrap_blob(&key).await.expect("must not error");
        std::env::remove_var("CREDD_BOOTSTRAP_BLOB");
        assert!(result.is_none(), "missing blob must produce Ok(None)");
    }

    #[tokio::test]
    async fn wrong_magic_errors() {
        let _g = ENV_GUARD.lock().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bootstrap.enc");
        std::fs::write(&path, b"BAD1somegarbagedata").unwrap();

        std::env::set_var("CREDD_BOOTSTRAP_BLOB", &path);
        let key = test_key();
        let result = load_bootstrap_blob(&key).await;
        std::env::remove_var("CREDD_BOOTSTRAP_BLOB");

        assert!(result.is_err(), "wrong magic must error");
        assert!(
            result.unwrap_err().to_string().contains("wrong magic"),
            "error must mention wrong magic"
        );
    }

    #[tokio::test]
    async fn good_blob_roundtrip() {
        let _g = ENV_GUARD.lock().await;
        let key = test_key();

        // Build a CBv1 blob: header || 0x1E || "kl_test_bearer_abc123"
        let header = br#"{"v":1,"slot":"engram-rust/credd-test","host":"test"}"#;
        let bare = b"kl_test_bearer_abc123";
        let mut payload = Vec::with_capacity(header.len() + 1 + bare.len());
        payload.extend_from_slice(header);
        payload.push(HEADER_KEY_SEPARATOR);
        payload.extend_from_slice(bare);

        let ciphertext = encrypt(&key, &payload).expect("encrypt");
        let mut blob = Vec::with_capacity(BOOTSTRAP_MAGIC.len() + ciphertext.len());
        blob.extend_from_slice(BOOTSTRAP_MAGIC);
        blob.extend_from_slice(&ciphertext);

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bootstrap.enc");
        std::fs::write(&path, &blob).unwrap();

        std::env::set_var("CREDD_BOOTSTRAP_BLOB", &path);
        let result = load_bootstrap_blob(&key).await;
        std::env::remove_var("CREDD_BOOTSTRAP_BLOB");

        let bearer = result.expect("ok").expect("some");
        assert_eq!(bearer.as_str(), "kl_test_bearer_abc123");
    }
}
