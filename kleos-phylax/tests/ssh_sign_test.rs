//! Integration test for the pure server-side SSH signing helper.

use std::fs;

// Must be in scope for `public_key.verify(...)` to resolve.
use signature::Verifier;

/// Read the throwaway ed25519 fixture key, sign a challenge, and verify the
/// returned blob decodes back to a valid ssh_key::Signature and that the
/// signature is cryptographically valid against the corresponding public key.
#[test]
fn sign_ed25519_roundtrip() {
    let pem = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/test_ed25519"
    ))
    .expect("test fixture private key not found");

    let pub_pem = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/test_ed25519.pub"
    ))
    .expect("test fixture public key not found");

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

    // Load the public key and cryptographically verify the signature.
    // `Verifier<ssh_key::Signature>` is implemented for `ssh_key::public::KeyData`
    // (and transitively for `ssh_key::PublicKey` via the same dispatch chain).
    // `PublicKey` has an inherent `verify` method for `SshSig` that would shadow
    // the trait call, so we call through `key_data()` directly to reach the
    // `Verifier<ssh_key::Signature>` impl that mirrors the `Signer` path used above.
    // This proves the bytes are a valid ed25519 signature over `challenge` --
    // not merely that the algorithm tag is correct.
    let public_key: ssh_key::PublicKey = pub_pem.trim().parse()
        .expect("test fixture public key must parse");
    Verifier::verify(public_key.key_data(), challenge, &sig)
        .expect("signature must cryptographically verify against the public key");
}

/// Passing a malformed PEM string must produce a Parse error, not a panic.
#[test]
fn sign_malformed_pem_returns_parse_error() {
    let result = kleos_phylax::handlers::ssh_sign::sign_with_pem("not a key", b"x", 0);
    assert!(
        matches!(result, Err(kleos_phylax::handlers::ssh_sign::SshSignError::Parse(_))),
        "expected SshSignError::Parse, got {:?}",
        result
    );
}
