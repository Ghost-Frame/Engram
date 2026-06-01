//! Server-side SSH signing: parse an OpenSSH/PEM private key and produce an
//! SSH wire-format signature. The private key never leaves this process.

use signature::Signer;
use ssh_key::PrivateKey;

/// Error type for the pure signer.
#[derive(Debug, thiserror::Error)]
pub enum SshSignError {
    /// The stored bytes did not parse as an OpenSSH/PEM private key.
    #[error("parse private key: {0}")]
    Parse(String),
    /// The signing operation itself failed.
    #[error("sign: {0}")]
    Sign(String),
    /// Encoding the signature to wire format failed.
    #[error("encode: {0}")]
    Encode(String),
}

/// Parse an OpenSSH/PEM private key and produce an SSH wire-format signature
/// over `data`. `_flags` is reserved (rsa-sha2 selection); ed25519 ignores it.
pub fn sign_with_pem(pem: &str, data: &[u8], _flags: u32) -> Result<Vec<u8>, SshSignError> {
    let key = PrivateKey::from_openssh(pem.as_bytes())
        .or_else(|_| pem.parse::<PrivateKey>())
        .map_err(|e| SshSignError::Parse(e.to_string()))?;
    let sig: ssh_key::Signature =
        key.try_sign(data).map_err(|e| SshSignError::Sign(e.to_string()))?;
    // TryFrom<Signature> for Vec<u8> encodes to SSH wire format (algorithm-prefixed).
    let blob = Vec::<u8>::try_from(sig).map_err(|e| SshSignError::Encode(e.to_string()))?;
    Ok(blob)
}
