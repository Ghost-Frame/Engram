//! Cred CLI - YubiKey-encrypted credential manager.
//!
//! Compatible with private cred's data format when using legacy mode.

use std::io::{self, BufRead, Read, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use kleos_cred::crypto::{
    decrypt_recovery, derive_key_legacy, encrypt_recovery, generate_hmac_secret, KEY_SIZE,
};
use kleos_cred::storage;
use kleos_cred::types::SecretData;
use kleos_cred::yubikey;
use kleos_lib::db::Database;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState};
use zeroize::Zeroize;

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
    /// Manage agent keys for service authentication
    AgentKey {
        #[command(subcommand)]
        action: AgentKeyAction,
    },
    /// Interactive TUI for browsing and managing secrets
    Tui,
}

#[derive(Subcommand)]
enum AgentKeyAction {
    /// Generate a new agent key
    Generate {
        /// Agent name/identifier
        name: String,
        /// Description of what this key is for
        #[arg(short, long, default_value = "")]
        description: String,
    },
    /// List all agent keys
    List,
    /// Revoke an agent key
    Revoke {
        /// Agent name to revoke
        name: String,
        /// Skip confirmation
        #[arg(short = 'y', long)]
        yes: bool,
    },
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
    let challenge = yubikey::get_or_create_challenge().context("failed to get challenge file")?;

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

            let db = Database::connect(&db_path().to_string_lossy())
                .await
                .context("failed to open database")?;

            match cmd {
                Commands::Store {
                    service,
                    key: secret_key,
                    secret_type,
                } => cmd_store(&db, &key, &service, &secret_key, &secret_type).await,
                Commands::Get {
                    service,
                    key: secret_key,
                    field,
                    raw,
                } => cmd_get(&db, &key, &service, &secret_key, field.as_deref(), raw).await,
                Commands::List { service } => cmd_list(&db, &key, service.as_deref()).await,
                Commands::Delete {
                    service,
                    key: secret_key,
                    yes,
                } => cmd_delete(&db, &key, &service, &secret_key, yes).await,
                Commands::Import { dry_run } => cmd_import(&db, &key, dry_run).await,
                Commands::Export => cmd_export(&db, &key).await,
                Commands::AgentKey { action } => cmd_agent_key(&db, action).await,
                Commands::Tui => cmd_tui(&db, &key).await,
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
    let secret = generate_hmac_secret();
    let secret_hex = hex::encode(secret);

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
    let _challenge = yubikey::get_or_create_challenge()?;
    eprintln!("challenge file created: {}", challenge_path.display());

    // Create recovery file
    eprintln!();
    eprintln!("creating recovery file...");
    let passphrase =
        rpassword::prompt_password("recovery passphrase: ").context("failed to read passphrase")?;
    let passphrase_confirm =
        rpassword::prompt_password("confirm passphrase: ").context("failed to read passphrase")?;

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

    let passphrase =
        rpassword::prompt_password("recovery passphrase: ").context("failed to read passphrase")?;

    let secret =
        decrypt_recovery(&passphrase, &data).context("decryption failed -- wrong passphrase?")?;

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

    storage::store_secret(db, 0, service, key, &data, master_key)
        .await
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
    let (_row, data) = storage::get_secret(db, 0, service, key, master_key)
        .await
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
    let max_svc = secrets
        .iter()
        .map(|s| s.category.len())
        .max()
        .unwrap_or(7)
        .max(7);
    let max_key = secrets
        .iter()
        .map(|s| s.name.len())
        .max()
        .unwrap_or(3)
        .max(3);

    println!(
        "{:<width_s$}  {:<width_k$}  TYPE",
        "SERVICE",
        "KEY",
        width_s = max_svc,
        width_k = max_key,
    );
    println!(
        "{:-<width_s$}  {:-<width_k$}  {:-<10}",
        "",
        "",
        "",
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
    let _ = storage::get_secret(db, 0, service, key, master_key)
        .await
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
    eprintln!("reading secrets from stdin");
    eprintln!("accepts JSON (from 'cred export') or TSV (service<TAB>key<TAB>value)");
    eprintln!("press Ctrl-D when done");
    if dry_run {
        eprintln!("(dry run -- nothing will be stored)");
    }
    eprintln!();

    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let input = input.trim();

    if input.is_empty() {
        eprintln!("no input");
        return Ok(());
    }

    // Detect JSON vs TSV
    if input.starts_with('[') {
        cmd_import_json(db, master_key, input, dry_run).await
    } else {
        cmd_import_tsv(db, master_key, input, dry_run).await
    }
}

async fn cmd_import_json(
    db: &Database,
    master_key: &[u8; KEY_SIZE],
    input: &str,
    dry_run: bool,
) -> Result<()> {
    #[derive(serde::Deserialize)]
    struct ImportEntry {
        service: String,
        key: String,
        value: SecretData,
    }

    let entries: Vec<ImportEntry> =
        serde_json::from_str(input).context("failed to parse JSON import")?;

    let mut imported = 0u32;

    for entry in &entries {
        if dry_run {
            eprintln!(
                "  [dry run] would store: {}/{} ({})",
                entry.service,
                entry.key,
                entry.value.type_name()
            );
        } else {
            storage::store_secret(db, 0, &entry.service, &entry.key, &entry.value, master_key)
                .await?;
            eprintln!("  stored: {}/{}", entry.service, entry.key);
        }
        imported += 1;
    }

    eprintln!();
    if dry_run {
        eprintln!("dry run complete: {} would be imported", imported);
    } else {
        eprintln!("import complete: {} stored", imported);
    }
    Ok(())
}

async fn cmd_import_tsv(
    db: &Database,
    master_key: &[u8; KEY_SIZE],
    input: &str,
    dry_run: bool,
) -> Result<()> {
    let mut imported = 0u32;
    let mut skipped = 0u32;

    for (lineno, line) in input.lines().enumerate() {
        let line = line.trim();

        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        if parts.len() != 3 {
            eprintln!(
                "  line {}: skipping (expected 3 tab-separated fields)",
                lineno + 1
            );
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
            eprintln!(
                "  [dry run] would store: {}/{} ({} chars)",
                service,
                key,
                value.len()
            );
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
        eprintln!(
            "dry run complete: {} would be imported, {} skipped",
            imported, skipped
        );
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
                eprintln!(
                    "warning: failed to decrypt {}/{}: {}",
                    row.category, row.name, e
                );
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
                endpoint: if endpoint.is_empty() {
                    None
                } else {
                    Some(endpoint.to_string())
                },
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
                totp_seed: None,
                notes: None,
            })
        }
        "oauth-app" => {
            print!("client id: ");
            io::stdout().flush()?;
            let mut client_id = String::new();
            io::stdin().read_line(&mut client_id)?;

            let client_secret = rpassword::prompt_password("client secret: ")?;

            print!("redirect uri (optional): ");
            io::stdout().flush()?;
            let mut redirect_uri = String::new();
            io::stdin().read_line(&mut redirect_uri)?;
            let redirect_uri = redirect_uri.trim();

            print!("scopes (comma-separated, optional): ");
            io::stdout().flush()?;
            let mut scopes_str = String::new();
            io::stdin().read_line(&mut scopes_str)?;
            let scopes_str = scopes_str.trim();

            Ok(SecretData::OAuthApp {
                client_id: client_id.trim().to_string(),
                client_secret,
                redirect_uri: if redirect_uri.is_empty() {
                    None
                } else {
                    Some(redirect_uri.to_string())
                },
                scopes: if scopes_str.is_empty() {
                    None
                } else {
                    Some(
                        scopes_str
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .collect(),
                    )
                },
            })
        }
        "ssh-key" => {
            eprintln!("enter private key (paste, then Ctrl-D):");
            let mut private_key = String::new();
            io::stdin().read_to_string(&mut private_key)?;

            print!("passphrase (optional, press enter to skip): ");
            io::stdout().flush()?;
            let passphrase = rpassword::prompt_password("")?;

            Ok(SecretData::SshKey {
                private_key,
                public_key: None,
                passphrase: if passphrase.is_empty() {
                    None
                } else {
                    Some(passphrase)
                },
            })
        }
        "environment" | "env" => {
            eprintln!("enter variables (KEY=VALUE per line, Ctrl-D to finish):");
            let mut variables = std::collections::HashMap::new();
            let stdin = io::stdin();
            for line in stdin.lock().lines() {
                let line = line?;
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = line.split_once('=') {
                    variables.insert(k.trim().to_string(), v.trim().to_string());
                } else {
                    eprintln!("  skipping invalid line: {}", line);
                }
            }
            Ok(SecretData::Environment { variables })
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

async fn cmd_agent_key(db: &Database, action: AgentKeyAction) -> Result<()> {
    use kleos_cred::agent_keys;

    match action {
        AgentKeyAction::Generate { name, description } => {
            let perms = kleos_cred::AgentKeyPermissions::default();
            let (key_str, agent_key) = agent_keys::create_agent_key(db, 0, &name, &perms)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            eprintln!("generated agent key for '{}'", name);
            if !description.is_empty() {
                eprintln!("description: {}", description);
            }
            eprintln!();
            eprintln!("key (save this now -- it cannot be retrieved later):");
            println!("{}", key_str);
            eprintln!();
            eprintln!("key id: {}", agent_key.id);
            Ok(())
        }
        AgentKeyAction::List => {
            let keys = agent_keys::list_agent_keys(db, 0)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            if keys.is_empty() {
                println!("no agent keys");
                return Ok(());
            }

            println!(
                "{:<20} {:<10} {:<20} HASH PREFIX",
                "NAME", "STATUS", "CREATED"
            );
            println!("{:-<20} {:-<10} {:-<20} {:-<16}", "", "", "", "");

            for k in &keys {
                let status = if k.is_valid() { "active" } else { "revoked" };
                let hash_prefix = &k.key_hash[..16.min(k.key_hash.len())];
                println!(
                    "{:<20} {:<10} {:<20} {}",
                    k.name, status, k.created_at, hash_prefix
                );
            }
            println!("\n{} key(s)", keys.len());
            Ok(())
        }
        AgentKeyAction::Revoke { name, yes } => {
            if !yes {
                print!("revoke agent key '{}'? [y/N] ", name);
                io::stdout().flush()?;
                let mut input = String::new();
                io::stdin().read_line(&mut input)?;
                if !input.trim().eq_ignore_ascii_case("y") {
                    eprintln!("aborted.");
                    return Ok(());
                }
            }
            agent_keys::revoke_agent_key(db, 0, &name)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            eprintln!("revoked agent key: {}", name);
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// TUI
// ---------------------------------------------------------------------------

/// A secret loaded for TUI display (decrypted).
struct TuiSecret {
    id: i64,
    service: String,
    key: String,
    data: SecretData,
}

struct TuiApp<'a> {
    db: &'a Database,
    master_key: [u8; 32],
    secrets: Vec<TuiSecret>,
    table_state: TableState,
    mode: TuiMode,
    input_buf: String,
    input_field: InputField,
    status_msg: String,
    show_values: bool,
    filter: String,
}

#[derive(PartialEq)]
enum TuiMode {
    Normal,
    Adding,
    Filtering,
    Confirm,
    Detail,
}

#[derive(PartialEq)]
enum InputField {
    Service,
    Key,
    Value,
}

impl<'a> TuiApp<'a> {
    fn new(db: &'a Database, master_key: [u8; 32]) -> Self {
        Self {
            db,
            master_key,
            secrets: Vec::new(),
            table_state: TableState::default(),
            mode: TuiMode::Normal,
            input_buf: String::new(),
            input_field: InputField::Service,
            status_msg: String::new(),
            show_values: false,
            filter: String::new(),
        }
    }

    async fn refresh(&mut self) {
        match storage::list_secrets(self.db, 0, None).await {
            Ok(rows) => {
                let mut secrets = Vec::new();
                for row in rows {
                    match storage::get_secret(
                        self.db,
                        0,
                        &row.category,
                        &row.name,
                        &self.master_key,
                    )
                    .await
                    {
                        Ok((_r, data)) => {
                            secrets.push(TuiSecret {
                                id: row.id,
                                service: row.category,
                                key: row.name,
                                data,
                            });
                        }
                        Err(e) => {
                            self.status_msg = format!("decrypt error: {}", e);
                        }
                    }
                }
                self.secrets = secrets;
                if self.secrets.is_empty() {
                    self.table_state.select(None);
                } else if self.table_state.selected().is_none() {
                    self.table_state.select(Some(0));
                }
            }
            Err(e) => {
                self.status_msg = format!("error: {}", e);
            }
        }
    }

    fn filtered_secrets(&self) -> Vec<&TuiSecret> {
        if self.filter.is_empty() {
            self.secrets.iter().collect()
        } else {
            let f = self.filter.to_lowercase();
            self.secrets
                .iter()
                .filter(|s| {
                    s.service.to_lowercase().contains(&f) || s.key.to_lowercase().contains(&f)
                })
                .collect()
        }
    }

    fn selected_secret(&self) -> Option<&TuiSecret> {
        let filtered = self.filtered_secrets();
        self.table_state
            .selected()
            .and_then(|i| filtered.get(i).copied())
    }
}

async fn cmd_tui(db: &Database, master_key: &[u8; 32]) -> Result<()> {
    let mut app = TuiApp::new(db, *master_key);
    app.refresh().await;

    // Terminal setup
    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    // Temp buffers for add flow
    let mut add_service = String::new();
    let mut add_key = String::new();

    loop {
        terminal.draw(|f| draw_ui(f, &mut app))?;

        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                match app.mode {
                    TuiMode::Normal => match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            break
                        }
                        KeyCode::Char('j') | KeyCode::Down => {
                            let filtered = app.filtered_secrets();
                            if !filtered.is_empty() {
                                let i = app
                                    .table_state
                                    .selected()
                                    .map(|i| (i + 1) % filtered.len())
                                    .unwrap_or(0);
                                app.table_state.select(Some(i));
                            }
                        }
                        KeyCode::Char('k') | KeyCode::Up => {
                            let filtered = app.filtered_secrets();
                            if !filtered.is_empty() {
                                let i = app
                                    .table_state
                                    .selected()
                                    .map(|i| if i == 0 { filtered.len() - 1 } else { i - 1 })
                                    .unwrap_or(0);
                                app.table_state.select(Some(i));
                            }
                        }
                        KeyCode::Char('a') => {
                            app.mode = TuiMode::Adding;
                            app.input_field = InputField::Service;
                            app.input_buf.clear();
                            add_service.clear();
                            add_key.clear();
                            app.status_msg = "enter service name".to_string();
                        }
                        KeyCode::Char('d') if app.selected_secret().is_some() => {
                            app.mode = TuiMode::Confirm;
                            app.status_msg = "delete? (y/n)".to_string();
                        }
                        KeyCode::Char('v') => {
                            app.show_values = !app.show_values;
                            app.status_msg = if app.show_values {
                                "values visible".to_string()
                            } else {
                                "values hidden".to_string()
                            };
                        }
                        KeyCode::Char('/') => {
                            app.mode = TuiMode::Filtering;
                            app.input_buf = app.filter.clone();
                            app.status_msg = "filter:".to_string();
                        }
                        KeyCode::Enter if app.selected_secret().is_some() => {
                            app.mode = TuiMode::Detail;
                        }
                        KeyCode::Char('r') => {
                            app.refresh().await;
                            app.status_msg = "refreshed".to_string();
                        }
                        _ => {}
                    },

                    TuiMode::Adding => match key.code {
                        KeyCode::Esc => {
                            app.input_buf.zeroize();
                            add_service.zeroize();
                            add_key.zeroize();
                            app.mode = TuiMode::Normal;
                            app.status_msg.clear();
                        }
                        KeyCode::Enter => match app.input_field {
                            InputField::Service => {
                                if app.input_buf.is_empty() {
                                    app.status_msg = "service name cannot be empty".to_string();
                                } else {
                                    add_service = app.input_buf.clone();
                                    app.input_buf.clear();
                                    app.input_field = InputField::Key;
                                    app.status_msg = "enter key name".to_string();
                                }
                            }
                            InputField::Key => {
                                if app.input_buf.is_empty() {
                                    app.status_msg = "key name cannot be empty".to_string();
                                } else {
                                    add_key = app.input_buf.clone();
                                    app.input_buf.clear();
                                    app.input_field = InputField::Value;
                                    app.status_msg = "enter api-key value".to_string();
                                }
                            }
                            InputField::Value => {
                                if app.input_buf.is_empty() {
                                    app.status_msg = "value cannot be empty".to_string();
                                } else {
                                    let data = SecretData::ApiKey {
                                        key: app.input_buf.clone(),
                                        endpoint: None,
                                        notes: None,
                                    };
                                    app.input_buf.zeroize();
                                    match storage::store_secret(
                                        app.db,
                                        0,
                                        &add_service,
                                        &add_key,
                                        &data,
                                        &app.master_key,
                                    )
                                    .await
                                    {
                                        Ok(id) => {
                                            app.status_msg = format!(
                                                "stored {}/{} (id={})",
                                                add_service, add_key, id
                                            );
                                            app.refresh().await;
                                        }
                                        Err(e) => {
                                            app.status_msg = format!("error: {}", e);
                                        }
                                    }
                                    add_service.zeroize();
                                    add_key.zeroize();
                                    app.mode = TuiMode::Normal;
                                }
                            }
                        },
                        KeyCode::Backspace => {
                            app.input_buf.pop();
                        }
                        KeyCode::Char(c) => {
                            app.input_buf.push(c);
                        }
                        _ => {}
                    },

                    TuiMode::Filtering => match key.code {
                        KeyCode::Esc => {
                            app.filter.clear();
                            app.mode = TuiMode::Normal;
                            app.status_msg.clear();
                            app.table_state.select(if app.secrets.is_empty() {
                                None
                            } else {
                                Some(0)
                            });
                        }
                        KeyCode::Enter => {
                            app.filter = app.input_buf.clone();
                            app.mode = TuiMode::Normal;
                            app.status_msg = if app.filter.is_empty() {
                                String::new()
                            } else {
                                format!("filter: {}", app.filter)
                            };
                            app.table_state
                                .select(if app.filtered_secrets().is_empty() {
                                    None
                                } else {
                                    Some(0)
                                });
                        }
                        KeyCode::Backspace => {
                            app.input_buf.pop();
                        }
                        KeyCode::Char(c) => {
                            app.input_buf.push(c);
                        }
                        _ => {}
                    },

                    TuiMode::Confirm => match key.code {
                        KeyCode::Char('y') => {
                            if let Some(secret) = app.selected_secret() {
                                let svc = secret.service.clone();
                                let k = secret.key.clone();
                                match storage::delete_secret(app.db, 0, &svc, &k).await {
                                    Ok(()) => {
                                        app.status_msg = format!("deleted {}/{}", svc, k);
                                        app.refresh().await;
                                    }
                                    Err(e) => {
                                        app.status_msg = format!("error: {}", e);
                                    }
                                }
                            }
                            app.mode = TuiMode::Normal;
                        }
                        _ => {
                            app.mode = TuiMode::Normal;
                            app.status_msg = "cancelled".to_string();
                        }
                    },

                    TuiMode::Detail => match key.code {
                        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                            app.mode = TuiMode::Normal;
                        }
                        _ => {}
                    },
                }
            }
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;
    Ok(())
}

fn draw_ui(f: &mut Frame, app: &mut TuiApp) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header
            Constraint::Min(5),    // table
            Constraint::Length(3), // status / input
        ])
        .split(f.area());

    draw_header(f, chunks[0]);
    draw_table(f, app, chunks[1]);
    draw_status(f, app, chunks[2]);

    // Modal overlay for detail view
    if app.mode == TuiMode::Detail {
        if let Some(secret) = app.selected_secret() {
            draw_detail_modal(f, secret, app.show_values);
        }
    }
}

fn draw_header(f: &mut Frame, area: Rect) {
    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            "cred",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" | "),
        Span::styled("a", Style::default().fg(Color::Yellow)),
        Span::raw("dd "),
        Span::styled("d", Style::default().fg(Color::Yellow)),
        Span::raw("elete "),
        Span::styled("v", Style::default().fg(Color::Yellow)),
        Span::raw("alues "),
        Span::styled("/", Style::default().fg(Color::Yellow)),
        Span::raw("filter "),
        Span::styled("r", Style::default().fg(Color::Yellow)),
        Span::raw("efresh "),
        Span::styled("q", Style::default().fg(Color::Yellow)),
        Span::raw("uit"),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );

    f.render_widget(header, area);
}

fn draw_table(f: &mut Frame, app: &mut TuiApp, area: Rect) {
    let filtered = app.filtered_secrets();

    let header = Row::new(vec![
        Cell::from("SERVICE").style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Cell::from("KEY").style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Cell::from("TYPE").style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Cell::from("PREVIEW").style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    ])
    .height(1);

    let rows: Vec<Row> = filtered
        .iter()
        .map(|secret| {
            let preview = if app.show_values {
                secret.data.redacted_preview()
            } else {
                secret.data.type_name().to_string()
            };
            Row::new(vec![
                Cell::from(secret.service.clone()).style(Style::default().fg(Color::Green)),
                Cell::from(secret.key.clone()),
                Cell::from(secret.data.type_name()).style(Style::default().fg(Color::Yellow)),
                Cell::from(preview).style(Style::default().fg(Color::DarkGray)),
            ])
        })
        .collect();

    let widths = [
        Constraint::Percentage(25),
        Constraint::Percentage(25),
        Constraint::Percentage(15),
        Constraint::Percentage(35),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(if app.filter.is_empty() {
                    format!(" secrets ({}) ", app.secrets.len())
                } else {
                    format!(
                        " secrets ({}/{}) [{}] ",
                        filtered.len(),
                        app.secrets.len(),
                        app.filter
                    )
                }),
        )
        .row_highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    f.render_stateful_widget(table, area, &mut app.table_state);
}

fn draw_status(f: &mut Frame, app: &TuiApp, area: Rect) {
    let content = match app.mode {
        TuiMode::Adding => {
            let field_name = match app.input_field {
                InputField::Service => "service",
                InputField::Key => "key",
                InputField::Value => "value",
            };
            let display = if app.input_field == InputField::Value {
                "*".repeat(app.input_buf.len())
            } else {
                app.input_buf.clone()
            };
            format!("[add] {}: {}|", field_name, display)
        }
        TuiMode::Filtering => {
            format!("/{}", app.input_buf)
        }
        _ => app.status_msg.clone(),
    };

    let status = Paragraph::new(content).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(status, area);
}

fn draw_detail_modal(f: &mut Frame, secret: &TuiSecret, show_value: bool) {
    let area = f.area();
    let modal_width = 60.min(area.width - 4);
    let modal_height = 10.min(area.height - 4);
    let modal_area = Rect::new(
        (area.width - modal_width) / 2,
        (area.height - modal_height) / 2,
        modal_width,
        modal_height,
    );

    f.render_widget(Clear, modal_area);

    let preview = if show_value {
        secret.data.redacted_preview()
    } else {
        "[hidden -- press v to show]".to_string()
    };

    let fields_str = secret.data.field_names().join(", ");

    let lines = vec![
        Line::from(vec![
            Span::styled("Service: ", Style::default().fg(Color::Cyan)),
            Span::raw(&secret.service),
        ]),
        Line::from(vec![
            Span::styled("Key:     ", Style::default().fg(Color::Cyan)),
            Span::raw(&secret.key),
        ]),
        Line::from(vec![
            Span::styled("Type:    ", Style::default().fg(Color::Cyan)),
            Span::raw(secret.data.type_name()),
        ]),
        Line::from(vec![
            Span::styled("Fields:  ", Style::default().fg(Color::Cyan)),
            Span::raw(&fields_str),
        ]),
        Line::from(vec![
            Span::styled("Preview: ", Style::default().fg(Color::Cyan)),
            Span::raw(preview),
        ]),
        Line::from(vec![
            Span::styled("ID:      ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("#{}", secret.id)),
        ]),
        Line::raw(""),
        Line::from(Span::styled(
            "press ESC to close, v to toggle values",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let detail = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(" detail "),
    );
    f.render_widget(detail, modal_area);
}
