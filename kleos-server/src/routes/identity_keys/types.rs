//! Request types for the /identity-keys endpoints.

use serde::Deserialize;

/// Body for POST /identity-keys/enroll -- registers a new signing key
/// after verifying a proof-of-possession signature.
#[derive(Deserialize)]
pub struct EnrollBody {
    pub tier: String,
    pub algo: String,
    pub pubkey_pem: String,
    pub host_label: String,
    pub label: Option<String>,
    pub serial: Option<String>,
    pub sig_hex: String,
    /// Server-issued single-use challenge nonce. Required for every
    /// enrollment after the first (bootstrap) key; obtained from
    /// POST /identity-keys/enroll/challenge and bound to the caller.
    pub nonce: Option<String>,
}

/// Body for POST /identity-keys/{id}/revoke.
#[derive(Deserialize)]
pub struct RevokeBody {
    pub reason: Option<String>,
}

/// Query parameters for GET /identity-keys.
#[derive(Deserialize)]
pub struct ListParams {
    pub active_only: Option<bool>,
}
