use ed25519_dalek::SigningKey;
use kleos_lib::mcp_token::{decode, verify_signature};
use kleos_phylax::handlers::kleos_token::mint_token_with_key;

/// Verify that a freshly minted token round-trips through decode and passes
/// signature verification under the same minting key.
#[test]
fn mint_roundtrips_and_verifies_under_minting_key() {
    let sk = SigningKey::from_bytes(&[7u8; 32]);
    let kid = "test-fingerprint";
    let token = mint_token_with_key(&sk, kid, 1, 300, "read,write").expect("mint ok");
    assert!(token.starts_with("kleos."));
    let decoded = decode(&token).expect("decode");
    verify_signature(&sk.verifying_key(), &decoded).expect("verifies under minting key");
}

/// admin scope exceeds the read,write cap -- must be rejected.
#[test]
fn mint_rejects_admin_scope() {
    let sk = SigningKey::from_bytes(&[7u8; 32]);
    assert!(mint_token_with_key(&sk, "kid", 1, 300, "admin").is_err());
}

/// Completely unknown scope must be rejected by the strict scope parser.
#[test]
fn mint_rejects_unknown_scope() {
    let sk = SigningKey::from_bytes(&[7u8; 32]);
    assert!(mint_token_with_key(&sk, "kid", 1, 300, "superuser").is_err());
}
