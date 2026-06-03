//! SSH certificate authority handlers.

use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use kleos_cred::CredError;
use kleos_credd::handlers::AppError;

use crate::state::PhylaxState;

/// Request body for signing caller-provided SSH public key material.
#[derive(Deserialize)]
pub struct SignRequest {
    /// SSH certificate key identity.
    pub identity: String,
    /// SSH authorized principal list.
    pub principal: String,
    /// OpenSSH validity interval such as `+5m` or `+2h`.
    pub ttl: String,
    /// OpenSSH public key text to sign.
    pub public_key: Option<String>,
}

/// Request body for minting an agent keypair and SSH certificate.
#[derive(Deserialize)]
pub struct MintRequest {
    /// Agent name used as key identity and output filename.
    pub agent: String,
    /// SSH authorized principal list.
    pub principal: String,
    /// OpenSSH validity interval such as `+5m` or `+2h`.
    pub ttl: String,
}

/// Sign caller-provided SSH public key material through the Phylax SSH CA.
pub async fn sign(
    State(state): State<PhylaxState>,
    Json(body): Json<SignRequest>,
) -> Result<Json<Value>, AppError> {
    validate_common(&body.identity, &body.principal, &body.ttl)?;
    let public_key = body.public_key.as_deref().ok_or_else(|| {
        CredError::InvalidInput("public_key is required for SSH CA signing".into())
    })?;
    validate_public_key(public_key)?;

    let signed =
        state
            .ssh_ca_signer
            .sign(&body.identity, &body.principal, &body.ttl, public_key)?;

    Ok(Json(json!({
        "identity": body.identity,
        "principal": body.principal,
        "ttl": body.ttl,
        "cert_public_key": signed.cert_public_key,
    })))
}

/// Mint an agent keypair and SSH certificate through the Phylax SSH CA.
pub async fn mint(
    State(state): State<PhylaxState>,
    Json(body): Json<MintRequest>,
) -> Result<Json<Value>, AppError> {
    validate_common(&body.agent, &body.principal, &body.ttl)?;

    let minted = state
        .ssh_ca_signer
        .mint(&body.agent, &body.principal, &body.ttl)?;

    Ok(Json(json!({
        "agent": body.agent,
        "principal": body.principal,
        "ttl": body.ttl,
        "key_path": minted.key_path,
        "cert_path": minted.cert_path,
        "cert_public_key": minted.cert_public_key,
    })))
}

/// Validate common SSH CA request fields.
fn validate_common(identity: &str, principal: &str, ttl: &str) -> Result<(), AppError> {
    validate_token("identity_or_agent", identity)?;
    validate_token("principal", principal)?;
    validate_token("ttl", ttl)?;
    if !ttl.starts_with('+') {
        return Err(CredError::InvalidInput("ttl must start with '+'".into()).into());
    }
    Ok(())
}

/// Validate a non-empty SSH CA token field.
fn validate_token(name: &str, value: &str) -> Result<(), AppError> {
    if value.trim().is_empty() {
        return Err(CredError::InvalidInput(format!("{} is required", name)).into());
    }
    Ok(())
}

/// Validate OpenSSH public key text before invoking the signer.
fn validate_public_key(public_key: &str) -> Result<(), AppError> {
    if !public_key.starts_with("ssh-") && !public_key.starts_with("ecdsa-") {
        return Err(
            CredError::InvalidInput("public_key must be an OpenSSH public key".into()).into(),
        );
    }
    Ok(())
}
