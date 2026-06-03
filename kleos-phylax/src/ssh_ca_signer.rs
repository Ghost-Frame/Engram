//! SSH certificate authority signer abstraction for Phylax.
//!
//! The production implementation shells through the existing `cred ssh-ca`
//! command path, while tests inject a fake signer so they never touch PKCS#11
//! or the physical YubiKey.

use std::path::PathBuf;
use std::process::Command;

use uuid::Uuid;

use kleos_cred::CredError;

/// Result returned after signing caller-provided SSH public key material.
#[derive(Clone, Debug)]
pub struct SignedSshCertificate {
    /// OpenSSH public certificate text.
    pub cert_public_key: String,
}

/// Result returned after minting an agent keypair and SSH certificate.
#[derive(Clone, Debug)]
pub struct MintedSshCertificate {
    /// Path to the generated private key on the Phylax host.
    pub key_path: PathBuf,
    /// Path to the generated SSH public certificate on the Phylax host.
    pub cert_path: PathBuf,
    /// OpenSSH public certificate text.
    pub cert_public_key: String,
}

/// Interface implemented by SSH CA signing backends.
pub trait SshCaSigner: Send + Sync {
    /// Sign caller-provided SSH public key material.
    fn sign(
        &self,
        identity: &str,
        principal: &str,
        ttl: &str,
        public_key: &str,
    ) -> Result<SignedSshCertificate, CredError>;

    /// Mint an agent keypair and sign it into an SSH certificate.
    fn mint(
        &self,
        agent: &str,
        principal: &str,
        ttl: &str,
    ) -> Result<MintedSshCertificate, CredError>;
}

/// Production SSH CA signer that delegates to the existing `cred ssh-ca` CLI.
#[derive(Clone, Debug)]
pub struct CommandSshCaSigner;

/// Provides helper methods for the command-backed SSH CA signer.
impl CommandSshCaSigner {
    /// Create a unique temporary public key path for a sign request.
    fn temp_pubkey_path(identity: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "phylax-ssh-ca-{}-{}.pub",
            sanitize_filename(identity),
            Uuid::new_v4()
        ))
    }
}

/// Delegate SSH CA operations to the local `cred ssh-ca` command.
impl SshCaSigner for CommandSshCaSigner {
    /// Sign a supplied public key by invoking the local `cred ssh-ca sign` command.
    fn sign(
        &self,
        identity: &str,
        principal: &str,
        ttl: &str,
        public_key: &str,
    ) -> Result<SignedSshCertificate, CredError> {
        let pubkey_path = Self::temp_pubkey_path(identity);
        std::fs::write(&pubkey_path, public_key).map_err(|e| {
            CredError::InvalidInput(format!("write temporary public key failed: {}", e))
        })?;

        let output = Command::new("cred")
            .args(["ssh-ca", "sign", "-I", identity, "-n", principal, "-V", ttl])
            .arg(&pubkey_path)
            .output()
            .map_err(|e| CredError::YubiKey(format!("ssh-ca signer unavailable: {}", e)))?;

        if !output.status.success() {
            let _ = std::fs::remove_file(&pubkey_path);
            return Err(CredError::YubiKey("ssh-ca signing failed".into()));
        }

        let cert_path = PathBuf::from(format!(
            "{}-cert.pub",
            pubkey_path.to_string_lossy().trim_end_matches(".pub")
        ));
        let cert_public_key = std::fs::read_to_string(&cert_path)
            .map_err(|e| CredError::YubiKey(format!("read signed certificate failed: {}", e)))?;

        let _ = std::fs::remove_file(&pubkey_path);
        let _ = std::fs::remove_file(&cert_path);

        Ok(SignedSshCertificate { cert_public_key })
    }

    /// Mint an agent keypair and certificate by invoking `cred ssh-ca mint`.
    fn mint(
        &self,
        agent: &str,
        principal: &str,
        ttl: &str,
    ) -> Result<MintedSshCertificate, CredError> {
        let output = Command::new("cred")
            .args([
                "ssh-ca", "mint", "--agent", agent, "-n", principal, "--ttl", ttl,
            ])
            .output()
            .map_err(|e| CredError::YubiKey(format!("ssh-ca mint unavailable: {}", e)))?;

        if !output.status.success() {
            return Err(CredError::YubiKey("ssh-ca mint failed".into()));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let key_path = parse_labeled_path(&stdout, "key:")
            .ok_or_else(|| CredError::YubiKey("ssh-ca mint did not return key path".into()))?;
        let cert_path = parse_labeled_path(&stdout, "cert:")
            .ok_or_else(|| CredError::YubiKey("ssh-ca mint did not return cert path".into()))?;
        let cert_public_key = std::fs::read_to_string(&cert_path)
            .map_err(|e| CredError::YubiKey(format!("read minted certificate failed: {}", e)))?;

        Ok(MintedSshCertificate {
            key_path,
            cert_path,
            cert_public_key,
        })
    }
}

/// Replace path-hostile filename characters with underscores.
fn sanitize_filename(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Parse a labeled path from `cred ssh-ca mint` output.
fn parse_labeled_path(output: &str, label: &str) -> Option<PathBuf> {
    output.lines().find_map(|line| {
        line.strip_prefix(label)
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
    })
}
