//! Integration test for the pure server-side SSH signing helper.

// Must be in scope for `public_key.verify(...)` to resolve.
use signature::Verifier;

/// Generate an ephemeral ed25519 key at runtime, sign a challenge, and verify
/// the returned blob decodes back to a valid ssh_key::Signature and that the
/// signature is cryptographically valid against the generated key's public half.
#[test]
fn sign_ed25519_roundtrip() {
    // Generate a fresh ephemeral key for this test run.
    // ssh-key 0.6 requires rand_core 0.6.x; rand 0.9 (workspace) uses rand_core 0.9,
    // so we pull rand_core 0.6 directly as a dev-dependency.
    let key =
        ssh_key::private::PrivateKey::random(&mut rand_core::OsRng, ssh_key::Algorithm::Ed25519)
            .expect("ephemeral ed25519 key generation must succeed");

    // Encode the private key as OpenSSH PEM (Zeroizing<String> -- deref to &str).
    let pem = key
        .to_openssh(ssh_key::LineEnding::LF)
        .expect("private key must encode to OpenSSH PEM");

    // Retain the public half for signature verification.
    let public_key = key.public_key().clone();

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

    // Cryptographically verify the signature against the generated public key.
    // `Verifier<ssh_key::Signature>` is implemented for `ssh_key::public::KeyData`
    // (and transitively for `ssh_key::PublicKey` via the same dispatch chain).
    // `PublicKey` has an inherent `verify` method for `SshSig` that would shadow
    // the trait call, so we call through `key_data()` directly to reach the
    // `Verifier<ssh_key::Signature>` impl that mirrors the `Signer` path used above.
    // This proves the bytes are a valid ed25519 signature over `challenge` --
    // not merely that the algorithm tag is correct.
    Verifier::verify(public_key.key_data(), challenge, &sig)
        .expect("signature must cryptographically verify against the generated public key");
}

/// Passing a malformed PEM string must produce a Parse error, not a panic.
#[test]
fn sign_malformed_pem_returns_parse_error() {
    let result = kleos_phylax::handlers::ssh_sign::sign_with_pem("not a key", b"x", 0);
    assert!(
        matches!(
            result,
            Err(kleos_phylax::handlers::ssh_sign::SshSignError::Parse(_))
        ),
        "expected SshSignError::Parse, got {:?}",
        result
    );
}
