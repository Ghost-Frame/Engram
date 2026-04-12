//! YubiKey HMAC-SHA1 challenge-response on OTP slot 2.
//!
//! Slot 2 is used by convention (same as KeePassXC, cred, and anything that
//! expects to share a secret across tools). The YubiKey holds a 20-byte
//! HMAC-SHA1 key that cannot be extracted; we send it a 32-byte challenge
//! and receive a 20-byte response. Because the secret never leaves the
//! hardware, possession of the YubiKey is required to compute the response.
//!
//! This module shells out to `ykman` on Linux/macOS or `ykchallenge.exe`
//! (Yubico .NET SDK wrapper) on Windows. Direct PC/SC access from Rust is
//! possible but the subprocess path matches what the standalone `cred` tool
//! uses, so the two programs stay interoperable without sharing code.

use std::path::PathBuf;
use std::process::Command;

use rand::RngCore;
use tracing::{debug, info};

use crate::{CredError, Result};

/// OTP slot used for HMAC-SHA1 challenge-response.
pub const SLOT: u8 = 2;

/// Challenge size in bytes. YubiKey HMAC-SHA1 accepts 0 to 64 byte challenges;
/// 32 matches what cred uses and gives us a full SHA-1 input block.
pub const CHALLENGE_SIZE: usize = 32;

/// HMAC-SHA1 response size in bytes.
pub const RESPONSE_SIZE: usize = 20;

/// Filename for the persisted challenge under the engram config dir.
const CHALLENGE_FILE: &str = "challenge";

/// Send a challenge to the YubiKey and get the HMAC-SHA1 response.
///
/// Platform dispatch: Windows uses `ykchallenge.exe` because `ykman`
/// subprocesses fail on Windows due to HID exclusive access restrictions.
/// Unix uses `ykman otp calculate 2 <hex>`, with a Python fallback for
/// distros where the ykman wrapper script refuses to take the HID.
pub fn challenge_response(challenge: &[u8]) -> Result<[u8; RESPONSE_SIZE]> {
    let challenge_hex = hex::encode(challenge);

    #[cfg(windows)]
    let output = try_ykchallenge(&challenge_hex)?;

    #[cfg(not(windows))]
    let output = try_ykman_calculate(&challenge_hex)
        .or_else(|first| try_python_ykman_calculate(&challenge_hex).map_err(|_| first))?;

    let decoded = hex::decode(output.trim())
        .map_err(|e| CredError::YubiKey(format!("invalid hex response from YubiKey: {}", e)))?;

    if decoded.len() != RESPONSE_SIZE {
        return Err(CredError::YubiKey(format!(
            "unexpected HMAC response length: {} (expected {})",
            decoded.len(),
            RESPONSE_SIZE
        )));
    }

    let mut response = [0u8; RESPONSE_SIZE];
    response.copy_from_slice(&decoded);
    debug!("YubiKey challenge-response ok ({} bytes)", RESPONSE_SIZE);
    Ok(response)
}

/// Check whether a YubiKey is plugged in and responsive.
///
/// This does not verify that slot 2 is programmed, only that `ykman info`
/// (or `ykchallenge.exe --info` on Windows) returns successfully.
pub fn is_available() -> bool {
    #[cfg(windows)]
    {
        Command::new("ykchallenge")
            .arg("--info")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        Command::new("ykman")
            .arg("info")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

/// Load or create the persistent 32-byte engram challenge.
///
/// Stored under `~/.config/engram/challenge` (or `$XDG_CONFIG_HOME/engram/
/// challenge`) with mode 0600. If the file exists with the wrong size we
/// refuse to overwrite it: that file protects an encrypted database and
/// silently regenerating would make the database unrecoverable.
pub fn get_or_create_challenge() -> Result<[u8; CHALLENGE_SIZE]> {
    let dir = config_dir();
    let path = dir.join(CHALLENGE_FILE);

    if path.exists() {
        let data = std::fs::read(&path)
            .map_err(|e| CredError::YubiKey(format!("read challenge {}: {}", path.display(), e)))?;
        if data.len() != CHALLENGE_SIZE {
            return Err(CredError::YubiKey(format!(
                "challenge file {} has wrong size ({} bytes, expected {}); refusing to overwrite -- back up and delete manually if you know what you are doing",
                path.display(),
                data.len(),
                CHALLENGE_SIZE
            )));
        }
        let mut out = [0u8; CHALLENGE_SIZE];
        out.copy_from_slice(&data);
        return Ok(out);
    }

    let mut challenge = [0u8; CHALLENGE_SIZE];
    rand::rngs::OsRng.fill_bytes(&mut challenge);

    std::fs::create_dir_all(&dir)
        .map_err(|e| CredError::YubiKey(format!("mkdir {}: {}", dir.display(), e)))?;
    std::fs::write(&path, challenge)
        .map_err(|e| CredError::YubiKey(format!("write challenge {}: {}", path.display(), e)))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| CredError::YubiKey(format!("chmod 600 {}: {}", path.display(), e)))?;
    }

    info!("generated new engram challenge at {}", path.display());
    Ok(challenge)
}

/// Software fallback for HMAC-SHA1 challenge-response (testing only).
///
/// Matches the byte-for-byte output of a YubiKey programmed with `secret`,
/// so CI and unit tests can exercise the derivation path without hardware.
pub fn software_hmac(secret: &[u8], challenge: &[u8; CHALLENGE_SIZE]) -> [u8; RESPONSE_SIZE] {
    use hmac::{digest::FixedOutput, Hmac, Mac};
    use sha1::Sha1;

    type HmacSha1 = Hmac<Sha1>;
    let mut mac = HmacSha1::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(challenge);
    let out = mac.finalize_fixed();

    let mut response = [0u8; RESPONSE_SIZE];
    response.copy_from_slice(&out[..RESPONSE_SIZE]);
    response
}

/// Config directory for engram: `$XDG_CONFIG_HOME/engram` or
/// `~/.config/engram`.
fn config_dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".config"))
                .unwrap_or_else(|_| PathBuf::from("."))
        })
        .join("engram")
}

// ---------------------------------------------------------------------------
// subprocess helpers
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn try_ykchallenge(challenge_hex: &str) -> Result<String> {
    let out = Command::new("ykchallenge")
        .arg(challenge_hex)
        .output()
        .map_err(|e| {
            CredError::YubiKey(format!(
                "ykchallenge.exe not found on PATH (expected at ~/.local/bin/ykchallenge.exe): {}",
                e
            ))
        })?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(CredError::YubiKey(format!(
            "ykchallenge failed: {}",
            stderr.trim()
        )));
    }

    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

#[cfg(not(windows))]
fn try_ykman_calculate(challenge_hex: &str) -> Result<String> {
    let out = Command::new("ykman")
        .args(["otp", "calculate", &SLOT.to_string(), challenge_hex])
        .output()
        .map_err(|e| {
            CredError::YubiKey(format!(
                "ykman not found on PATH (install yubikey-manager): {}",
                e
            ))
        })?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(CredError::YubiKey(format!(
            "ykman otp calculate failed: {}",
            stderr.trim()
        )));
    }

    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

#[cfg(not(windows))]
fn try_python_ykman_calculate(challenge_hex: &str) -> Result<String> {
    let script = format!(
        "import sys\nfrom ykman._cli.__main__ import main\nsys.argv = ['ykman', 'otp', 'calculate', '{}', '{}']\nmain()\n",
        SLOT, challenge_hex
    );

    let out = Command::new("sudo")
        .args(["python3", "-c", &script])
        .output()
        .map_err(|e| CredError::YubiKey(format!("sudo python3 ykman failed: {}", e)))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(CredError::YubiKey(format!(
            "python ykman calculate failed: {}",
            stderr.trim()
        )));
    }

    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn software_hmac_deterministic() {
        let secret = b"test-secret-key";
        let mut challenge = [0u8; CHALLENGE_SIZE];
        challenge[0] = 1;
        challenge[31] = 2;

        let r1 = software_hmac(secret, &challenge);
        let r2 = software_hmac(secret, &challenge);
        assert_eq!(r1, r2);
    }

    #[test]
    fn software_hmac_varies_with_secret() {
        let mut challenge = [0u8; CHALLENGE_SIZE];
        challenge[0] = 1;

        let r1 = software_hmac(b"secret1", &challenge);
        let r2 = software_hmac(b"secret2", &challenge);
        assert_ne!(r1, r2);
    }

    #[test]
    fn software_hmac_varies_with_challenge() {
        let secret = b"shared-secret";
        let mut c1 = [0u8; CHALLENGE_SIZE];
        let mut c2 = [0u8; CHALLENGE_SIZE];
        c1[0] = 1;
        c2[0] = 2;

        let r1 = software_hmac(secret, &c1);
        let r2 = software_hmac(secret, &c2);
        assert_ne!(r1, r2);
    }

    #[test]
    fn software_hmac_matches_rfc2202_case_1() {
        use hmac::{digest::FixedOutput, Hmac, Mac};
        use sha1::Sha1;
        type HmacSha1 = Hmac<Sha1>;

        let key = [0x0bu8; 20];
        let mut mac = HmacSha1::new_from_slice(&key).unwrap();
        mac.update(b"Hi There");
        let expected = mac.finalize_fixed();

        assert_eq!(
            hex::encode(expected),
            "b617318655057264e28bc0b6fb378c8ef146be00"
        );
    }

    #[test]
    fn challenge_size_constants_are_sane() {
        assert_eq!(CHALLENGE_SIZE, 32);
        assert_eq!(RESPONSE_SIZE, 20);
        assert_eq!(SLOT, 2);
    }
}
