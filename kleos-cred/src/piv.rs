//! YubiKey PIV applet operations for ECDH bootstrap auth.
//!
//! See `~/projects/plans/2026-04-26-ecdh-bootstrap-auth-piv.md` for the
//! full design. This module wraps `ykman` (CLI) for key generation /
//! certificate creation / pubkey export, and Python `yubikit` (subprocess)
//! for the ECDH key agreement and ECDSA signing operations that the
//! `ykman` CLI does not expose.
//!
//! Slot allocation:
//! - 9D KEY_MANAGEMENT: P-256 ECDH key agreement (server-side in credd).
//! - 9A AUTHENTICATION: P-256 ECDSA signing (client identity proof).

use std::path::PathBuf;
use std::process::Command;

use crate::{CredError, Result};

/// PIV slot identifiers we care about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PivSlot {
    /// 9A AUTHENTICATION -- client identity proof (ECDSA sign).
    Authentication,
    /// 9D KEY_MANAGEMENT -- ECDH key agreement (server side).
    KeyManagement,
}

impl PivSlot {
    /// Hex string passed to `ykman piv` commands.
    pub fn as_hex(&self) -> &'static str {
        match self {
            PivSlot::Authentication => "9a",
            PivSlot::KeyManagement => "9d",
        }
    }

    /// Python `yubikit.piv.SLOT.<name>` symbol used by ECDH/sign helpers.
    pub fn yubikit_name(&self) -> &'static str {
        match self {
            PivSlot::Authentication => "AUTHENTICATION",
            PivSlot::KeyManagement => "KEY_MANAGEMENT",
        }
    }
}

/// PIN policy for generated PIV keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinPolicy {
    Never,
    Once,
    Always,
}

impl PinPolicy {
    pub fn as_str(&self) -> &'static str {
        match self {
            PinPolicy::Never => "never",
            PinPolicy::Once => "once",
            PinPolicy::Always => "always",
        }
    }
}

/// Touch policy for generated PIV keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPolicy {
    Never,
    Always,
    Cached,
}

impl TouchPolicy {
    pub fn as_str(&self) -> &'static str {
        match self {
            TouchPolicy::Never => "never",
            TouchPolicy::Always => "always",
            TouchPolicy::Cached => "cached",
        }
    }
}

/// Standard config dir for cred (matches yubikey.rs::config_dir).
fn config_dir() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".config"))
                .unwrap_or_else(|_| PathBuf::from("."))
        });
    base.join("cred")
}

/// Path where the PEM-encoded public key for `slot` is stored after
/// `cred piv setup`. credd reads from these files at startup.
pub fn pubkey_path(slot: PivSlot) -> PathBuf {
    config_dir().join(format!("piv-{}-pubkey.pem", slot.as_hex()))
}

fn ykman_missing(e: std::io::Error) -> CredError {
    CredError::YubiKey(format!(
        "ykman not found on PATH (install yubikey-manager): {}",
        e
    ))
}

/// YubiKey factory-default PIV PIN. Used when `YKMAN_PIN` env is not set.
pub const DEFAULT_PIN: &str = "123456";

/// Returns the user-supplied management key if `YKMAN_MGMT_KEY` is set,
/// otherwise None which signals "use default by piping a blank line to
/// ykman" (the YubiKey-side check accepts the prompt-default sentinel
/// rather than the literal hex bytes that the docs advertise).
fn mgmt_key_override() -> Option<String> {
    std::env::var("YKMAN_MGMT_KEY").ok()
}

fn piv_pin() -> String {
    std::env::var("YKMAN_PIN").unwrap_or_else(|_| DEFAULT_PIN.to_string())
}

/// Run a ykman command. If `YKMAN_MGMT_KEY` is set, pass it via
/// `--management-key`; otherwise pipe a blank line to stdin so ykman
/// uses its factory-default management key. `extra_path` is appended
/// after the management-key args (used for output PEM paths).
fn ykman_with_mgmt(args: &[&str], extra_path: Option<&PathBuf>) -> Result<std::process::Output> {
    use std::io::Write;
    let mut cmd = Command::new("ykman");
    cmd.args(args);
    let mk = mgmt_key_override();
    if let Some(ref k) = mk {
        cmd.args(["--management-key", k]);
    }
    if let Some(p) = extra_path {
        cmd.arg(p);
    }
    if mk.is_none() {
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        let mut child = cmd.spawn().map_err(ykman_missing)?;
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(b"\n");
        }
        child
            .wait_with_output()
            .map_err(|e| CredError::YubiKey(format!("ykman wait failed: {}", e)))
    } else {
        cmd.output().map_err(ykman_missing)
    }
}

/// Generate a P-256 keypair on-device in `slot` with the given policies.
/// Writes the public key to `out_pem` (PEM-encoded). The private key
/// never leaves the YubiKey.
pub fn generate_p256_key(
    slot: PivSlot,
    pin_policy: PinPolicy,
    touch_policy: TouchPolicy,
    out_pem: &PathBuf,
) -> Result<()> {
    let out = ykman_with_mgmt(
        &[
            "piv",
            "keys",
            "generate",
            "--algorithm",
            "eccp256",
            "--pin-policy",
            pin_policy.as_str(),
            "--touch-policy",
            touch_policy.as_str(),
            slot.as_hex(),
        ],
        Some(out_pem),
    )?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(CredError::YubiKey(format!(
            "ykman piv keys generate {} failed: {}",
            slot.as_hex(),
            stderr.trim()
        )));
    }
    Ok(())
}

/// Generate a self-signed certificate in `slot`. PIV requires a cert in
/// the slot even when we do not use X.509 validation.
pub fn generate_self_signed_cert(slot: PivSlot, subject: &str, pubkey_pem: &PathBuf) -> Result<()> {
    let pin = piv_pin();
    let out = ykman_with_mgmt(
        &[
            "piv",
            "certificates",
            "generate",
            "--subject",
            subject,
            "--pin",
            &pin,
            slot.as_hex(),
        ],
        Some(pubkey_pem),
    )?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(CredError::YubiKey(format!(
            "ykman piv certificates generate {} failed: {}",
            slot.as_hex(),
            stderr.trim()
        )));
    }
    Ok(())
}

/// Read the public key currently stored in `slot` and return its PEM
/// representation. Useful for `cred piv status` and for re-exporting
/// when the cached pubkey file was deleted.
pub fn export_pubkey_pem(slot: PivSlot) -> Result<String> {
    let tmp = std::env::temp_dir().join(format!("cred-piv-{}-export.pem", slot.as_hex()));
    let out = Command::new("ykman")
        .args(["piv", "keys", "export", slot.as_hex()])
        .arg(&tmp)
        .output()
        .map_err(ykman_missing)?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let _ = std::fs::remove_file(&tmp);
        return Err(CredError::YubiKey(format!(
            "ykman piv keys export {} failed: {}",
            slot.as_hex(),
            stderr.trim()
        )));
    }

    let pem = std::fs::read_to_string(&tmp)
        .map_err(|e| CredError::YubiKey(format!("read pubkey tempfile: {}", e)))?;
    let _ = std::fs::remove_file(&tmp);
    Ok(pem)
}

/// Cheap probe of whether `slot` has a key provisioned. Discards stdout.
pub fn slot_has_key(slot: PivSlot) -> bool {
    let tmp = std::env::temp_dir().join(format!("cred-piv-probe-{}.pem", slot.as_hex()));
    let ok = Command::new("ykman")
        .args(["piv", "keys", "export", slot.as_hex()])
        .arg(&tmp)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let _ = std::fs::remove_file(&tmp);
    ok
}

/// Perform ECDH key agreement using the YubiKey PIV applet.
/// `peer_pubkey_pem` is the peer's P-256 public key in PEM form.
/// Returns the raw 32-byte shared secret. Only `KeyManagement` (9D)
/// is supported.
///
/// Implemented via Python `yubikit` because `ykman` CLI does not expose
/// `calculate_secret`. Pattern mirrors `try_python_ykman_calculate` in
/// `yubikey.rs`.
pub fn ecdh_agree(slot: PivSlot, peer_pubkey_pem: &str) -> Result<[u8; 32]> {
    if slot != PivSlot::KeyManagement {
        return Err(CredError::InvalidInput(format!(
            "ECDH only supported on KEY_MANAGEMENT (9D), not {}",
            slot.as_hex()
        )));
    }

    let script = format!(
        r#"
import sys, base64
from ykman.device import list_all_devices
from yubikit.piv import PivSession, SLOT
from yubikit.core.smartcard import SmartCardConnection
from cryptography.hazmat.primitives.serialization import load_pem_public_key

peer_pem = sys.stdin.buffer.read()
peer = load_pem_public_key(peer_pem)

devices = list_all_devices()
if not devices:
    print("no yubikey detected", file=sys.stderr); sys.exit(2)

dev, _info = devices[0]
with dev.open_connection(SmartCardConnection) as conn:
    session = PivSession(conn)
    shared = session.calculate_secret(SLOT.{slot}, peer)
    sys.stdout.write(base64.b16encode(shared).decode().lower())
"#,
        slot = slot.yubikit_name(),
    );

    let mut child = Command::new("python3")
        .args(["-c", &script])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| CredError::YubiKey(format!("python3 spawn failed: {}", e)))?;

    {
        use std::io::Write;
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| CredError::YubiKey("failed to open python3 stdin".into()))?;
        stdin
            .write_all(peer_pubkey_pem.as_bytes())
            .map_err(|e| CredError::YubiKey(format!("write peer pubkey: {}", e)))?;
    }

    let out = child
        .wait_with_output()
        .map_err(|e| CredError::YubiKey(format!("python3 wait failed: {}", e)))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(CredError::YubiKey(format!(
            "PIV ECDH (slot {}) failed: {}",
            slot.as_hex(),
            stderr.trim()
        )));
    }

    let hex_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let bytes = hex::decode(&hex_str)
        .map_err(|e| CredError::YubiKey(format!("invalid hex from ECDH subprocess: {}", e)))?;
    if bytes.len() != 32 {
        return Err(CredError::YubiKey(format!(
            "expected 32-byte ECDH shared secret, got {}",
            bytes.len()
        )));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(arr)
}

/// ECDSA-sign `payload` using the private key in `slot`. Returns the raw
/// DER-encoded signature. Implemented via Python `yubikit`. Only
/// `Authentication` (9A) is supported.
pub fn piv_sign(slot: PivSlot, payload: &[u8]) -> Result<Vec<u8>> {
    if slot != PivSlot::Authentication {
        return Err(CredError::InvalidInput(format!(
            "PIV sign only supported on AUTHENTICATION (9A), not {}",
            slot.as_hex()
        )));
    }

    let payload_hex = hex::encode(payload);
    let script = format!(
        r#"
import sys, base64
from ykman.device import list_all_devices
from yubikit.piv import PivSession, SLOT, KEY_TYPE, HASH_ALGORITHM
from yubikit.core.smartcard import SmartCardConnection
from cryptography.hazmat.primitives import hashes

payload = bytes.fromhex("{payload}")
digest = hashes.Hash(hashes.SHA256())
digest.update(payload)
prehashed = digest.finalize()

devices = list_all_devices()
if not devices:
    print("no yubikey detected", file=sys.stderr); sys.exit(2)

dev, _info = devices[0]
with dev.open_connection(SmartCardConnection) as conn:
    session = PivSession(conn)
    sig = session.sign(SLOT.{slot}, KEY_TYPE.ECCP256, prehashed, hash_algorithm=HASH_ALGORITHM.SHA256)
    sys.stdout.write(base64.b16encode(sig).decode().lower())
"#,
        payload = payload_hex,
        slot = slot.yubikit_name(),
    );

    let out = Command::new("python3")
        .args(["-c", &script])
        .output()
        .map_err(|e| CredError::YubiKey(format!("python3 sign spawn failed: {}", e)))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(CredError::YubiKey(format!(
            "PIV sign (slot {}) failed: {}",
            slot.as_hex(),
            stderr.trim()
        )));
    }

    let hex_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
    hex::decode(&hex_str)
        .map_err(|e| CredError::YubiKey(format!("invalid hex from sign subprocess: {}", e)))
}

/// SHA-256 fingerprint of a PEM public key, formatted as colon-separated
/// uppercase hex (e.g. `AB:CD:EF:...`). Used by `cred piv status`.
pub fn pubkey_fingerprint(pem: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(pem.as_bytes());
    let digest = hasher.finalize();
    digest
        .iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join(":")
}
