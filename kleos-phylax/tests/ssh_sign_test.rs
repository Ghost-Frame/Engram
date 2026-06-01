//! Integration test for the pure server-side SSH signing helper.

use std::fs;

/// Read the throwaway ed25519 fixture key, sign a challenge, and verify the
/// returned blob decodes back to a valid ssh_key::Signature whose algorithm
/// matches the ed25519 key.
#[test]
fn sign_ed25519_roundtrip() {
    let pem = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/test_ed25519"
    ))
    .expect("test fixture key not found");

    let challenge = b"challenge-bytes-to-sign";

    let blob = kleos_phylax::handlers::ssh_sign::sign_with_pem(&pem, challenge, 0)
        .expect("sign_with_pem must succeed for a valid ed25519 key");

    assert!(!blob.is_empty(), "signature blob must not be empty");

    // Decode the wire-format blob back into a Signature.
    let sig = ssh_key::Signature::try_from(blob.as_slice())
        .expect("blob must decode as a valid ssh_key::Signature");

    // Algorithm on the decoded signature must match ed25519.
    assert_eq!(
        sig.algorithm(),
        ssh_key::Algorithm::Ed25519,
        "algorithm must be Ed25519"
    );
}
