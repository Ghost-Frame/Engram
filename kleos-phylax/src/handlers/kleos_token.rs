//! Mint short-lived, identity-signed Kleos bearers for local agents.
use ed25519_dalek::SigningKey;
use kleos_lib::mcp_token::{self, McpTokenError};

/// Socket-minted tokens are hard-capped at read,write; admin is never issued here.
const MINT_SCOPE_CAP: &str = "read,write";

/// Mint a Kleos mcp_token bearer signed by `signing_key`. `kid` is the minting
/// key fingerprint, `uid` the Kleos user id. Enforces the read,write cap and a
/// fixed TTL (no renewal window beyond ttl). Returns the bearer string only.
pub fn mint_token_with_key(
    signing_key: &SigningKey,
    kid: &str,
    uid: i64,
    ttl_secs: u64,
    scopes: &str,
) -> Result<String, McpTokenError> {
    let requested = mcp_token::parse_scopes_strict(scopes)?;
    let cap = mcp_token::parse_scopes_strict(MINT_SCOPE_CAP)?;
    mcp_token::scopes_within_cap(&requested, &cap)?;
    let (token, _payload) =
        mcp_token::mint(signing_key, kid, uid, None, scopes, ttl_secs, ttl_secs)?;
    Ok(token)
}
