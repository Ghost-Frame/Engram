//! Helper to derive and write the database encryption key from YubiKey.
//!
//! Usage: derive-db-key [OUTPUT_PATH]
//!
//! By default, writes the hex-encoded key to OUTPUT_PATH (default: db.key)
//! with mode 0600 so it is never printed to stdout accidentally.
//!
//! Pass `--stdout` as the first argument to print the key to stdout instead
//! (for piping into scripts). A warning is emitted to stderr in that case.

use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

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

    let key = kleos_cred::crypto::derive_key(0, b"", Some(&response));
    let hex_key = hex::encode(key);

    let args: Vec<String> = std::env::args().collect();

    if args.contains(&"--stdout".to_string()) {
        eprintln!("WARNING: writing key material to stdout");
        println!("{}", hex_key);
    } else {
        let path = args
            .iter()
            .skip(1)
            .find(|a| !a.starts_with('-'))
            .map(|s| s.as_str())
            .unwrap_or("db.key");

        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        opts.mode(0o600);
        let mut f = opts.open(path).unwrap_or_else(|e| {
            eprintln!("failed to open output file {path}: {e}");
            std::process::exit(1);
        });

        writeln!(f, "{}", hex_key).unwrap_or_else(|e| {
            eprintln!("failed to write key to {path}: {e}");
            std::process::exit(1);
        });

        eprintln!("Key written to {path}");
    }
}
