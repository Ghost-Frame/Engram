//! Server-side SSH signing: parse an OpenSSH private key and produce an
//! SSH wire-format signature. The private key never leaves this process.

// Must be in scope for `key.try_sign(...)` to resolve; `Signer` is the trait
// that provides the `try_sign` method used below.
use signature::Signer;
use ssh_key::PrivateKey;

/// Error type for the pure signer.
#[derive(Debug, thiserror::Error)]
pub enum SshSignError {
    /// The stored bytes did not parse as an OpenSSH private key.
    #[error("parse private key: {0}")]
    Parse(String),
    /// The signing operation itself failed.
    #[error("sign: {0}")]
    Sign(String),
    /// Encoding the signature to wire format failed.
    #[error("encode: {0}")]
    Encode(String),
}

/// Parse an OpenSSH-format PEM private key and produce an SSH wire-format
/// signature over `data`.
///
/// Only OpenSSH-format PEM private keys (the `-----BEGIN OPENSSH PRIVATE
/// KEY-----` envelope produced by `ssh-keygen`) are accepted. PKCS#8 /
/// SEC1 PEM private keys are NOT supported.
///
/// `_flags` carries SSH agent protocol sign-request flags (bit 2 =
/// SSH_AGENT_RSA_SHA2_256, bit 4 = SSH_AGENT_RSA_SHA2_512 for RSA keys).
/// The current implementation ignores them because ed25519 has exactly one
/// signature algorithm and needs no flag-based dispatch.
pub fn sign_with_pem(pem: &str, data: &[u8], _flags: u32) -> Result<Vec<u8>, SshSignError> {
    let key = PrivateKey::from_openssh(pem.as_bytes())
        .map_err(|e| SshSignError::Parse(e.to_string()))?;
    let sig: ssh_key::Signature =
        key.try_sign(data).map_err(|e| SshSignError::Sign(e.to_string()))?;
    // TryFrom<Signature> for Vec<u8> encodes to SSH wire format (algorithm-prefixed).
    let blob = Vec::<u8>::try_from(sig).map_err(|e| SshSignError::Encode(e.to_string()))?;
    Ok(blob)
}
