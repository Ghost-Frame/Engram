//! Phylax: agent-native credential authority.
//!
//! Extends credd with approval workflows, single-use leases, ECDH
//! per-request auth, namespace isolation, access policies, and SSH
//! key management. If no policies are configured, behavior is
//! identical to plain credd.

pub mod audit;
pub mod handlers;
pub mod middleware;
pub mod migrate;
pub mod models;
pub mod router;
pub mod ssh_ca_signer;
pub mod state;
