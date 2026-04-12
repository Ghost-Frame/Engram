//! Cred CLI - YubiKey-encrypted credential manager.
//!
//! Compatible with private cred's data format when using legacy mode.

use std::io::{self, BufRead, Read, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use engram_cred::crypto::{derive_key_legacy, KEY_SIZE};
use engram_cred::yubikey;
use engram_cred::types::SecretData;
use engram_cred::storage;
use engram_lib::db::Database;

/// YubiKey-encrypted credential manager.
#[derive(Parser)]
#[command(name = "cred", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize: generate HMAC secret, program YubiKey, create recovery kit
    Init,
    /// Store a secret (prompts interactively)
    Store {
        /// Service name (e.g., authentik, grafana)
        service: String,
        /// Key name (e.g., zan, api-key)
        key: String,
        /// Secret type: api-key, login, oauth-app, ssh-key, note, environment
        #[arg(short = 't', long, default_value = "api-key")]
        secret_type: String,
    },
    /// Retrieve a secret
    Get {
        /// Service name
        service: String,
        /// Key name
        key: String,
        /// Extract a specific field (e.g., password, username, key)
        #[arg(short, long)]
        field: Option<String>,
        /// Print raw value only (for piping)
        #[arg(short, long)]
        raw: bool,
    },
    /// List all stored secrets (values redacted)
    List {
        /// Filter by service name
        #[arg(short, long)]
        service: Option<String>,
    },
    /// Delete a secret
    Delete {
        /// Service name
        service: String,
        /// Key name
        key: String,
        /// Skip confirmation prompt
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// Recover: decrypt recovery file and program a new YubiKey
    Recover {
        /// Path to recovery.enc file
        #[arg(short, long, default_value = "~/.config/cred/recovery.enc")]
        from: String,
    },
    /// Bulk import secrets from stdin (service<TAB>key<TAB>value)
    Import {
        /// Dry run: show what would be imported without storing
        #[arg(short = 'n', long)]
        dry_run: bool,
    },
    /// Export all secrets as JSON (for backup/migration)
    Export,
}

fn config_dir() -> PathBuf {
    directories::ProjectDirs::from("", "", "cred")
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| {
            directories::BaseDirs::new()
                .map(|d| d.home_dir().join(".config").join("cred"))
                .unwrap_or_else(|| PathBuf::from(".").join(".config").join("cred"))
        })
}

fn db_path() -> PathBuf {
    config_dir().join("cred.db")
}

fn shellexpand(path: &str) -> String {
    shellexpand::tilde(path).into_owned()
}

/// Derive master key from YubiKey using legacy (private cred compatible) KDF.
fn derive_master_key() -> Result<[u8; KEY_SIZE]> {
    let challenge = yubikey::get_or_create_challenge()
        .context("failed to get challenge file")?;

    let response = yubikey::challenge_response(&challenge)
        .context("failed to get YubiKey challenge-response -- is the YubiKey plugged in?")?;

    Ok(derive_key_legacy(&response))
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init => cmd_init().await,
        Commands::Recover { from } => cmd_recover(&from).await,
        // All other commands need YubiKey
        cmd => {
            eprintln!("unlocking with YubiKey...");
            let key = derive_master_key()?;
            eprintln!("unlocked.");

            let db = Database::connect(&db_path().to_string_lossy()).await
                .context("failed to open database")?;

            match cmd {
                Commands::Store { service, key: secret_key, secret_type } => {
                    cmd_store(&db, &key, &service, &secret_key, &secret_type).await
                }
                Commands::Get { service, key: secret_key, field, raw } => {
                    cmd_get(&db, &key, &service, &secret_key, field.as_deref(), raw).await
                }
                Commands::List { service } => {
                    cmd_list(&db, &key, service.as_deref()).await
                }
                Commands::Delete { service, key: secret_key, yes } => {
                    cmd_delete(&db, &key, &service, &secret_key, yes).await
                }
                Commands::Import { dry_run } => {
                    cmd_import(&db, &key, dry_run).await
                }
                Commands::Export => {
                    cmd_export(&db, &key).await
                }
                Commands::Init | Commands::Recover { .. } => unreachable!(),
            }
        }
    }
}

async fn cmd_init() -> Result<()> {
    eprintln!("cred init - YubiKey credential manager setup");
    eprintln!();

    // Check for existing setup
    let config = config_dir();
    let challenge_path = config.join("challenge");

    if challenge_path.exists() {
        eprintln!("WARNING: cred is already initialized.");
        eprintln!("challenge file exists at: {}", challenge_path.display());
        print!("Continue anyway? This will overwrite existing setup. [y/N] ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            eprintln!("aborted.");
            return Ok(());
        }
    }

    // Create config directory
    std::fs::create_dir_all(&config)?;

    // Generate HMAC secret
    eprintln!("generating 20-byte HMAC-SHA1 secret...");
    let mut secret = [0u8; 20];
    rand::rngs::OsRng.fill_bytes(&mut secret);
    let secret_hex = hex::encode(&secret);

    eprintln!();
    eprintln!("HMAC secret (save this in Bitwarden NOW):");
    eprintln!("  {}", secret_hex);
    eprintln!();

    // Check for YubiKey
    if yubikey::is_available() {
        print!("YubiKey detected. Program slot 2 with this secret? [Y/n] ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if input.trim().is_empty() || input.trim().eq_ignore_ascii_case("y") {
            // Program YubiKey
            eprintln!("programming YubiKey slot 2...");
            program_yubikey_slot2(&secret_hex)?;
            eprintln!("YubiKey programmed.");
        }
    } else {
        eprintln!("No YubiKey detected. Program it manually:");
        eprintln!("  ykman otp chalresp 2 --force {}", secret_hex);
    }

    // Generate challenge file
    eprintln!();
    eprintln!("generating challenge file...");
    let challenge = yubikey::get_or_create_challenge()?;
    eprintln!("challenge file created: {}", challenge_path.display());

    // Create recovery file
    eprintln!();
    eprintln!("creating recovery file...");
    let passphrase = rpassword::prompt_password("recovery passphrase: ")
        .context("failed to read passphrase")?;
    let passphrase_confirm = rpassword::prompt_password("confirm passphrase: ")
        .context("failed to read passphrase")?;

    if passphrase != passphrase_confirm {
        anyhow::bail!("passphrases do not match");
    }

    let recovery_data = encrypt_recovery(&passphrase, &secret)?;
    let recovery_path = config.join("recovery.enc");
    std::fs::write(&recovery_path, &recovery_data)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&recovery_path, std::fs::Permissions::from_mode(0o600))?;
    }

    eprintln!("recovery file written to: {}", recovery_path.display());

    // Initialize database
    eprintln!();
    eprintln!("initializing database...");
    let db = Database::connect(&db_path().to_string_lossy()).await?;
    init_schema(&db).await?;
    eprintln!("database initialized: {}", db_path().display());

    eprintln!();
    eprintln!("setup complete!");
    eprintln!();
    eprintln!("IMPORTANT:");
    eprintln!("  1. Save the HMAC secret to Bitwarden");
    eprintln!("  2. Copy recovery.enc to a safe backup location");
    eprintln!("  3. Remember your recovery passphrase");

    Ok(())
}

async fn cmd_recover(from: &str) -> Result<()> {
    let path = shellexpand(from);

    if !std::path::Path::new(&path).exists() {
        anyhow::bail!("recovery file not found: {}", path);
    }

    eprintln!("reading recovery file: {}", path);
    let data = std::fs::read(&path)?;

    let passphrase = rpassword::prompt_password("recovery passphrase: ")
        .context("failed to read passphrase")?;

    let secret = decrypt_recovery(&passphrase, &data)
        .context("decryption failed -- wrong passphrase?")?;

    eprintln!("secret recovered ({} bytes)", secret.len());
    eprintln!();

    // Check if YubiKey is present
    if yubikey::is_available() {
        print!("YubiKey detected. Program it with the recovered secret? [y/N] ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if input.trim().eq_ignore_ascii_case("y") {
            program_yubikey_slot2(&hex::encode(&secret))?;
            eprintln!("YubiKey programmed.");

            // Make sure challenge file exists
            let _challenge = yubikey::get_or_create_challenge()?;
            eprintln!("ready to use.");
            return Ok(());
        }
    }

    // If no YubiKey or user declined, show the hex
    eprintln!("HMAC secret (hex): {}", hex::encode(&secret));
    eprintln!("program a YubiKey manually:");
    eprintln!("  ykman otp chalresp 2 --force {}", hex::encode(&secret));

    Ok(())
}

async fn cmd_store(
    db: &Database,
    master_key: &[u8; KEY_SIZE],
    service: &str,
    key: &str,
    secret_type: &str,
) -> Result<()> {
    let data = prompt_secret_data(secret_type)?;

    storage::store_secret(db, 0, service, key, &data, master_key).await
        .context("failed to store secret")?;

    eprintln!("stored: {}/{}", service, key);
    Ok(())
}

async fn cmd_get(
    db: &Database,
    master_key: &[u8; KEY_SIZE],
    service: &str,
    key: &str,
    field: Option<&str>,
    raw: bool,
) -> Result<()> {
    let (_row, data) = storage::get_secret(db, 0, service, key, master_key).await
        .context("secret not found")?;

    if raw {
        // Print raw value for piping
        if let Some(field_name) = field {
            if let Some(value) = data.get_field(field_name) {
                print!("{}", value);
            }
        } else if let Some(value) = data.bare_value() {
            print!("{}", value);
        } else {
            print!("{}", serde_json::to_string(&data)?);
        }
    } else {
        // Pretty print
        println!("{}/{}", service, key);
        println!("{}", serde_json::to_string_pretty(&data)?);
    }

    Ok(())
}

async fn cmd_list(
    db: &Database,
    _master_key: &[u8; KEY_SIZE],
    service_filter: Option<&str>,
) -> Result<()> {
    let secrets = storage::list_secrets(db, 0, service_filter).await?;

    if secrets.is_empty() {
        println!("no secrets stored");
        return Ok(());
    }

    // Find column widths
    let max_svc = secrets.iter().map(|s| s.category.len()).max().unwrap_or(7).max(7);
    let max_key = secrets.iter().map(|s| s.name.len()).max().unwrap_or(3).max(3);

    println!(
        "{:<width_s$}  {:<width_k$}  TYPE",
        "SERVICE", "KEY",
        width_s = max_svc,
        width_k = max_key,
    );
    println!(
        "{:-<width_s$}  {:-<width_k$}  {:-<10}",
        "", "", "",
        width_s = max_svc,
        width_k = max_key,
    );

    for row in &secrets {
        println!(
            "{:<width_s$}  {:<width_k$}  {}",
            row.category,
            row.name,
            row.secret_type.as_str(),
            width_s = max_svc,
            width_k = max_key,
        );
    }

    println!("\n{} secret(s)", secrets.len());
    Ok(())
}

async fn cmd_delete(
    db: &Database,
    master_key: &[u8; KEY_SIZE],
    service: &str,
    key: &str,
    skip_confirm: bool,
) -> Result<()> {
    // Verify it exists first
    let _ = storage::get_secret(db, 0, service, key, master_key).await
        .context("secret not found")?;

    if !skip_confirm {
        print!("delete {}/{}? [y/N] ", service, key);
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            eprintln!("aborted.");
            return Ok(());
        }
    }

    storage::delete_secret(db, 0, service, key).await?;
    eprintln!("deleted: {}/{}", service, key);
    Ok(())
}

async fn cmd_import(db: &Database, master_key: &[u8; KEY_SIZE], dry_run: bool) -> Result<()> {
    eprintln!("reading secrets from stdin (one per line)");
    eprintln!("format: service<TAB>key<TAB>value");
    eprintln!("lines starting with # are ignored");
    eprintln!("press Ctrl-D when done");
    if dry_run {
        eprintln!("(dry run -- nothing will be stored)");
    }
    eprintln!();

    let stdin = io::stdin();
    let mut imported = 0u32;
    let mut skipped = 0u32;

    for (lineno, line) in stdin.lock().lines().enumerate() {
        let line = line?;
        let line = line.trim();

        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        if parts.len() != 3 {
            eprintln!("  line {}: skipping (expected 3 tab-separated fields)", lineno + 1);
            skipped += 1;
            continue;
        }

        let (service, key, value) = (parts[0].trim(), parts[1].trim(), parts[2].trim());

        if service.is_empty() || key.is_empty() || value.is_empty() {
            eprintln!("  line {}: skipping (empty field)", lineno + 1);
            skipped += 1;
            continue;
        }

        if dry_run {
            eprintln!("  [dry run] would store: {}/{} ({} chars)", service, key, value.len());
        } else {
            let data = SecretData::ApiKey {
                key: value.to_string(),
                endpoint: None,
                notes: None,
            };
            storage::store_secret(db, 0, service, key, &data, master_key).await?;
            eprintln!("  stored: {}/{}", service, key);
        }
        imported += 1;
    }

    eprintln!();
    if dry_run {
        eprintln!("dry run complete: {} would be imported, {} skipped", imported, skipped);
    } else {
        eprintln!("import complete: {} stored, {} skipped", imported, skipped);
    }

    Ok(())
}

async fn cmd_export(db: &Database, master_key: &[u8; KEY_SIZE]) -> Result<()> {
    let rows = storage::list_secrets(db, 0, None).await?;

    if rows.is_empty() {
        eprintln!("no secrets to export");
        return Ok(());
    }

    #[derive(serde::Serialize)]
    struct ExportEntry {
        service: String,
        key: String,
        value: SecretData,
    }

    let mut entries = Vec::new();
    for row in rows {
        // Decrypt each secret
        match storage::get_secret(db, 0, &row.category, &row.name, master_key).await {
            Ok((_row, data)) => {
                entries.push(ExportEntry {
                    service: row.category,
                    key: row.name,
                    value: data,
                });
            }
            Err(e) => {
                eprintln!("warning: failed to decrypt {}/{}: {}", row.category, row.name, e);
            }
        }
    }

    let json = serde_json::to_string_pretty(&entries)?;
    println!("{}", json);

    eprintln!("\nexported {} secret(s)", entries.len());
    Ok(())
}

// Helper functions

fn prompt_secret_data(secret_type: &str) -> Result<SecretData> {
    match secret_type {
        "api-key" => {
            let key = rpassword::prompt_password("api key: ")?;
            print!("endpoint (optional): ");
            io::stdout().flush()?;
            let mut endpoint = String::new();
            io::stdin().read_line(&mut endpoint)?;
            let endpoint = endpoint.trim();

            Ok(SecretData::ApiKey {
                key,
                endpoint: if endpoint.is_empty() { None } else { Some(endpoint.to_string()) },
                notes: None,
            })
        }
        "note" => {
            eprintln!("enter note (Ctrl-D to finish):");
            let mut content = String::new();
            io::stdin().read_to_string(&mut content)?;
            Ok(SecretData::Note { content })
        }
        "login" => {
            print!("url: ");
            io::stdout().flush()?;
            let mut url = String::new();
            io::stdin().read_line(&mut url)?;

            print!("username: ");
            io::stdout().flush()?;
            let mut username = String::new();
            io::stdin().read_line(&mut username)?;

            let password = rpassword::prompt_password("password: ")?;

            Ok(SecretData::Login {
                username: username.trim().to_string(),
                password,
                url: Some(url.trim().to_string()),
                notes: None,
            })
        }
        _ => {
            // Default to api-key for unknown types
            let key = rpassword::prompt_password("value: ")?;
            Ok(SecretData::ApiKey {
                key,
                endpoint: None,
                notes: None,
            })
        }
    }
}

fn program_yubikey_slot2(secret_hex: &str) -> Result<()> {
    #[cfg(windows)]
    {
        std::process::Command::new("ykman")
            .args(["otp", "chalresp", "2", "--force", secret_hex])
            .status()
            .context("failed to run ykman")?;
    }

    #[cfg(not(windows))]
    {
        std::process::Command::new("ykman")
            .args(["otp", "chalresp", "2", "--force", secret_hex])
            .status()
            .context("failed to run ykman")?;
    }

    Ok(())
}

fn encrypt_recovery(passphrase: &str, secret: &[u8]) -> Result<Vec<u8>> {
    use aes_gcm::{aead::Aead, Aes256Gcm, KeyInit, Nonce};
    use argon2::{Algorithm, Argon2, Params, Version};

    // Generate random salt
    let mut salt = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut salt);

    // Derive key from passphrase -- use the same strong params as the main vault
    // (64 MiB memory, 3 iterations) to resist offline brute-force.
    let params = Params::new(65536, 3, 1, Some(32))
        .map_err(|e| anyhow::anyhow!("argon2 params error: {}", e))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; 32];
    argon2.hash_password_into(passphrase.as_bytes(), &salt, &mut key)
        .map_err(|e| anyhow::anyhow!("key derivation failed: {}", e))?;

    // Encrypt
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| anyhow::anyhow!("cipher init failed: {}", e))?;
    let mut nonce_bytes = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher.encrypt(nonce, secret)
        .map_err(|e| anyhow::anyhow!("encryption failed: {}", e))?;

    // Format: salt (16) || nonce (12) || ciphertext
    let mut output = Vec::with_capacity(16 + 12 + ciphertext.len());
    output.extend_from_slice(&salt);
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&ciphertext);

    Ok(output)
}

fn decrypt_recovery(passphrase: &str, data: &[u8]) -> Result<Vec<u8>> {
    use aes_gcm::{aead::Aead, Aes256Gcm, KeyInit, Nonce};
    use argon2::{Algorithm, Argon2, Params, Version};

    if data.len() < 28 {
        anyhow::bail!("recovery file too short");
    }

    let salt = &data[..16];
    let nonce_bytes = &data[16..28];
    let ciphertext = &data[28..];

    // Derive key from passphrase
    let params = Params::new(65536, 3, 1, Some(32))
        .map_err(|e| anyhow::anyhow!("argon2 params error: {}", e))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; 32];
    argon2.hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| anyhow::anyhow!("key derivation failed: {}", e))?;

    // Decrypt
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| anyhow::anyhow!("cipher init failed: {}", e))?;
    let nonce = Nonce::from_slice(nonce_bytes);

    cipher.decrypt(nonce, ciphertext)
        .map_err(|e| anyhow::anyhow!("decryption failed: {}", e))
}

async fn init_schema(db: &Database) -> Result<()> {
    db.write(|conn| {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS cred_secrets (
                id INTEGER PRIMARY KEY,
                user_id INTEGER NOT NULL DEFAULT 0,
                name TEXT NOT NULL,
                category TEXT NOT NULL,
                secret_type TEXT NOT NULL,
                encrypted_data BLOB NOT NULL,
                nonce BLOB NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                UNIQUE(user_id, category, name)
            );
            CREATE INDEX IF NOT EXISTS idx_cred_secrets_user_category
                ON cred_secrets(user_id, category);",
        )?;
        Ok(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("failed to init schema: {}", e))
}

use rand::RngCore;
