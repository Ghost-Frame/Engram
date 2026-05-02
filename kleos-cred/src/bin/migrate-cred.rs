//! Migrate credentials from JSON backup to engram-cred database.
//!
//! Usage: migrate-cred <json-file> <db-path>
//!
//! Requires YubiKey touch on slot 2 to derive encryption key.

use std::path::PathBuf;

use clap::Parser;
use kleos_cred::crypto::{derive_key, KEY_SIZE};
use kleos_cred::types::SecretData;
use kleos_lib::db::Database;
use serde::Deserialize;

#[derive(Parser)]
#[command(name = "migrate-cred")]
#[command(about = "Migrate credentials from JSON backup to engram-cred database")]
struct Args {
    /// Path to JSON backup file
    json_file: PathBuf,

    /// Path to output database (will be created if missing)
    db_path: PathBuf,

    /// Use software HMAC instead of YubiKey (for testing only)
    #[arg(long)]
    software_hmac: bool,
}

/// JSON backup format from old cred
#[derive(Debug, Deserialize)]
struct BackupEntry {
    service: String,
    key: String,
    value: BackupValue,
}

#[derive(Debug, Deserialize)]
struct BackupValue {
    #[serde(rename = "type")]
    secret_type: String,
    // Login fields
    username: Option<String>,
    password: Option<String>,
    url: Option<String>,
    // ApiKey fields
    #[serde(rename = "key")]
    api_key: Option<String>,
    endpoint: Option<String>,
    // OAuthApp fields
    client_id: Option<String>,
    client_secret: Option<String>,
    redirect_uri: Option<String>,
    scopes: Option<Vec<String>>,
    // Note fields
    content: Option<String>,
    // Generic notes field
    notes: Option<String>,
}

impl BackupEntry {
    fn to_secret_data(&self) -> Option<SecretData> {
        match self.value.secret_type.as_str() {
            "login" => Some(SecretData::Login {
                username: self.value.username.clone().unwrap_or_default(),
                password: self.value.password.clone().unwrap_or_default(),
                url: self.value.url.clone(),
                totp_seed: None,
                notes: self.value.notes.clone(),
            }),
            "api_key" => Some(SecretData::ApiKey {
                key: self
                    .value
                    .api_key
                    .clone()
                    .or(self.value.password.clone())
                    .unwrap_or_default(),
                endpoint: self.value.endpoint.clone().or(self.value.url.clone()),
                notes: self.value.notes.clone(),
            }),
            "o_auth_app" | "oauth_app" => Some(SecretData::OAuthApp {
                client_id: self.value.client_id.clone().unwrap_or_default(),
                client_secret: self.value.client_secret.clone().unwrap_or_default(),
                redirect_uri: self.value.redirect_uri.clone(),
                scopes: self.value.scopes.clone(),
            }),
            "note" => Some(SecretData::Note {
                content: self.value.content.clone().unwrap_or_default(),
            }),
            _ => {
                eprintln!("Unknown secret type: {}", self.value.secret_type);
                None
            }
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Read JSON backup
    println!("Reading backup from {:?}...", args.json_file);
    let json_data = std::fs::read_to_string(&args.json_file)?;
    let entries: Vec<BackupEntry> = serde_json::from_str(&json_data)?;
    println!("Found {} entries", entries.len());

    // Derive encryption key
    let encryption_key: [u8; KEY_SIZE] = if args.software_hmac {
        println!("Using software HMAC (testing mode)");
        let challenge = kleos_cred::yubikey::get_or_create_challenge()?;
        let response = kleos_cred::yubikey::software_hmac(b"test-secret", &challenge);
        derive_key(1, b"", Some(&response))
    } else {
        println!("Touch YubiKey slot 2 to derive encryption key...");
        let challenge = kleos_cred::yubikey::get_or_create_challenge()?;
        let response = kleos_cred::yubikey::challenge_response(&challenge)?;
        derive_key(1, b"", Some(&response))
    };
    println!("Encryption key derived");

    // Connect to database
    println!("Opening database at {:?}...", args.db_path);
    let db_path_str = args.db_path.to_string_lossy();
    let db = Database::connect(&db_path_str).await?;

    // Ensure cred_secrets table exists
    db.write(|conn| {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS cred_secrets (
                id INTEGER PRIMARY KEY,
                user_id INTEGER NOT NULL,
                name TEXT NOT NULL,
                category TEXT NOT NULL,
                secret_type TEXT NOT NULL,
                encrypted_data BLOB NOT NULL,
                nonce BLOB NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                UNIQUE(user_id, category, name)
            );",
        )
        .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
    })
    .await?;

    // Import each entry
    let mut imported = 0;
    let mut skipped = 0;

    for entry in &entries {
        let category = &entry.service;
        let name = &entry.key;

        match entry.to_secret_data() {
            Some(data) => {
                match kleos_cred::storage::store_secret(
                    &db,
                    0,
                    category,
                    name,
                    &data,
                    &encryption_key,
                )
                .await
                {
                    Ok(id) => {
                        println!("  [OK] {}/{} -> id={}", category, name, id);
                        imported += 1;
                    }
                    Err(e) => {
                        // Might be duplicate - try update instead
                        if e.to_string().contains("UNIQUE constraint") {
                            match kleos_cred::storage::update_secret(
                                &db,
                                0,
                                category,
                                name,
                                &data,
                                &encryption_key,
                            )
                            .await
                            {
                                Ok(()) => {
                                    println!("  [UPDATE] {}/{}", category, name);
                                    imported += 1;
                                }
                                Err(e2) => {
                                    eprintln!("  [ERROR] {}/{}: {}", category, name, e2);
                                    skipped += 1;
                                }
                            }
                        } else {
                            eprintln!("  [ERROR] {}/{}: {}", category, name, e);
                            skipped += 1;
                        }
                    }
                }
            }
            None => {
                eprintln!("  [SKIP] {}/{}: unknown type", category, name);
                skipped += 1;
            }
        }
    }

    println!(
        "\nMigration complete: {} imported, {} skipped",
        imported, skipped
    );
    Ok(())
}
