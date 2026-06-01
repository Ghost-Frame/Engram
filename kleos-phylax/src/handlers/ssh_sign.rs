//! Server-side SSH signing: parse an OpenSSH private key and produce an
//! SSH wire-format signature. The private key never leaves this process.

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
// Must be in scope for `key.try_sign(...)` to resolve; `Signer` is the trait
// that provides the `try_sign` method used below.
use signature::Signer;
use ssh_key::PrivateKey;

use kleos_cred::storage::get_secret;
use kleos_cred::types::SecretData;
use kleos_cred::CredError;
use kleos_credd::auth::Auth;
use kleos_credd::handlers::AppError;

use crate::state::PhylaxState;

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

/// Body for a sign request: hex-encoded data to sign + agent protocol flags.
#[derive(Deserialize)]
pub struct SignRequest {
    /// Hex-encoded data the SSH client wants signed.
    pub data_hex: String,
    /// SSH agent protocol sign flags (0 for ed25519).
    pub flags: u32,
}

/// Response: hex-encoded SSH wire-format signature blob.
#[derive(Serialize)]
pub struct SignResponse {
    /// Hex-encoded signature blob for SSH_AGENT_SIGN_RESPONSE.
    pub signature_hex: String,
}

/// Load the plaintext SSH private key PEM for (user, category, name) from the vault.
async fn load_ssh_pem(
    state: &PhylaxState,
    user_id: i64,
    category: &str,
    name: &str,
) -> Result<String, AppError> {
    let (_row, data) = get_secret(
        &state.inner.db,
        user_id,
        category,
        name,
        state.inner.master_key.as_ref(),
    )
    .await?;
    match data {
        SecretData::SshKey { private_key, .. } => Ok(private_key),
        _ => Err(CredError::PermissionDenied("secret is not an SSH key".into()).into()),
    }
}

/// POST /phylax/ssh/{category}/{name}/sign -- sign `data_hex` with the vault key.
/// Auto-sign path only; the approval gate is added in a later task.
pub async fn sign(
    Auth(auth): Auth,
    State(state): State<PhylaxState>,
    Path((category, name)): Path<(String, String)>,
    Json(body): Json<SignRequest>,
) -> Result<Json<SignResponse>, AppError> {
    if !auth.is_master() && !auth.can_access_category(&category) {
        return Err(CredError::PermissionDenied("category not permitted".into()).into());
    }
    let data = hex::decode(&body.data_hex)
        .map_err(|_| CredError::PermissionDenied("bad data_hex".into()))?;
    let pem = load_ssh_pem(&state, auth.user_id(), &category, &name).await?;
    let sig = sign_with_pem(&pem, &data, body.flags)
        .map_err(|e| CredError::PermissionDenied(format!("sign failed: {e}")))?;
    Ok(Json(SignResponse { signature_hex: hex::encode(sig) }))
}
