//! Helper to derive and print the database encryption key from YubiKey.
//!
//! Usage: derive-db-key
//! Requires YubiKey touch on slot 2.

fn main() {
    let challenge = kleos_cred::yubikey::get_or_create_challenge().unwrap_or_else(|e| {
        eprintln!("failed to load challenge: {e}");
        std::process::exit(1);
    });

    eprintln!("Touch YubiKey slot 2...");
    let response = kleos_cred::yubikey::challenge_response(&challenge).unwrap_or_else(|e| {
        eprintln!("challenge-response failed: {e}");
        std::process::exit(1);
    });

    // Same derivation as engram-server: user_id=0, password=empty, yubikey=response
    let key = kleos_cred::crypto::derive_key(0, b"", Some(&response));
    println!("{}", hex::encode(key));
}
