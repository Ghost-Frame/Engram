//! Extended audit actions for Phylax operations.
//!
//! Writes directly to the existing cred_audit table with Phylax-specific
//! action strings. Session correlation IDs link related operations.

use kleos_cred::CredError;
use kleos_lib::db::Database;
use rusqlite::params;

/// Log a Phylax-specific audit event to cred_audit.
///
/// Uses the same table as credd's existing audit system but with
/// Phylax-specific action strings. The correlation_id field is stored
/// in the access_tier column for linkage.
#[allow(clippy::too_many_arguments)]
pub async fn log_phylax_audit(
    db: &Database,
    user_id: i64,
    agent_name: Option<&str>,
    operator_id: Option<&str>,
    source_ip: Option<&str>,
    policy_id: Option<i64>,
    session_id: Option<&str>,
    action: &str,
    category: &str,
    secret_name: &str,
    success: bool,
    correlation_id: Option<&str>,
) -> kleos_cred::Result<i64> {
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let action_owned = action.to_string();
    let agent_owned = agent_name.map(|s| s.to_string());
    let operator_id_owned = operator_id.map(|s| s.to_string());
    let source_ip_owned = source_ip.map(|s| s.to_string());
    let session_id_owned = session_id.map(|s| s.to_string());
    let cat = category.to_string();
    let sec = secret_name.to_string();
    let corr = correlation_id.map(|s| s.to_string());

    db.write(move |conn| {
        conn.execute(
            "INSERT INTO cred_audit
             (user_id, agent_name, operator_id, source_ip, policy_id, session_id,
              action, category, secret_name, access_tier, success, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                user_id,
                agent_owned,
                operator_id_owned,
                source_ip_owned,
                policy_id,
                session_id_owned,
                action_owned,
                cat,
                sec,
                corr,
                success as i32,
                now
            ],
        )?;
        Ok(conn.last_insert_rowid())
    })
    .await
    .map_err(|e| CredError::Database(e.to_string()))
}

/// Well-known Phylax audit action strings.
pub mod actions {
    /// An agent requested approval for a policy-gated secret.
    pub const APPROVAL_REQUESTED: &str = "approval_requested";
    /// Operator approved an agent's access request.
    pub const APPROVAL_GRANTED: &str = "approval_granted";
    /// Operator denied an agent's access request.
    pub const APPROVAL_DENIED: &str = "approval_denied";
    /// An approval request expired without a decision.
    pub const APPROVAL_EXPIRED: &str = "approval_expired";
    /// A single-use lease was minted from an approval.
    pub const LEASE_MINTED: &str = "lease_minted";
    /// A lease was successfully redeemed for a secret.
    pub const LEASE_REDEEMED: &str = "lease_redeemed";
    /// A replay attempt on an already-used lease was rejected.
    pub const LEASE_REPLAY: &str = "lease_replay_rejected";
    /// An ECDH challenge was issued to an agent.
    pub const ECDH_CHALLENGE: &str = "ecdh_challenge_issued";
    /// ECDH authentication succeeded.
    pub const ECDH_SUCCESS: &str = "ecdh_auth_success";
    /// ECDH authentication failed.
    pub const ECDH_FAILED: &str = "ecdh_auth_failed";
    /// A PIV 9A public key was enrolled.
    pub const PIV_ENROLLED: &str = "piv_enrolled";
    /// A PIV 9A public key was revoked.
    pub const PIV_REVOKED: &str = "piv_revoked";
    /// SSH key settings were updated.
    pub const SSH_SETTINGS: &str = "ssh_settings_updated";
    /// An SSH private key was used to produce a signature.
    pub const SSH_SIGN: &str = "ssh_sign";
    /// A caller-provided SSH public key was signed by the SSH CA.
    pub const SSH_CA_SIGN: &str = "ssh_ca_sign";
    /// An agent keypair + SSH certificate was minted by the SSH CA.
    pub const SSH_CA_MINT: &str = "ssh_ca_mint";
}
