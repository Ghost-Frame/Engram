//! HTTP server setup for credd.

use kleos_cred::crypto::KEY_SIZE;
use kleos_lib::db::migrations::run_migrations;
use kleos_lib::db::Database;

use crate::build_router;
use crate::listener;
use crate::state::AppState;

/// Run the credd HTTP server.
///
/// `master_key` is the 32-byte AES-256-GCM key credd uses to encrypt and
/// authenticate cred entries. The caller (main.rs) derives it from a YubiKey
/// challenge-response (default) or a password (opt-in), so this function does
/// not care how it was produced.
///
/// `listen` is honored as a fallback `CREDD_BIND` if neither `CREDD_SOCKET`
/// nor `CREDD_BIND` env vars are set; otherwise the env wins. This preserves
/// the `--listen 127.0.0.1:4400` CLI argument behaviour for callers that
/// don't use env-based config.
#[tracing::instrument(skip(master_key, encryption_key), fields(listen = %listen, db_path = %db_path))]
pub async fn run(
    listen: &str,
    db_path: &str,
    master_key: [u8; KEY_SIZE],
    encryption_key: Option<[u8; 32]>,
) -> anyhow::Result<()> {
    // Connect to database (with optional at-rest encryption)
    let db = Database::connect_encrypted(db_path, encryption_key).await?;

    // Run migrations
    db.write(|conn| run_migrations(conn)).await?;

    let state = AppState::new(db, master_key);

    // Build the router via the shared builder so integration tests and the
    // binary server exercise the same middleware stack (including the
    // preauth brute-force throttle).
    let app = build_router(state);

    // If neither CREDD_SOCKET nor CREDD_BIND is set, treat the --listen
    // CLI value as CREDD_BIND so the legacy single-listener flow keeps
    // working.
    if std::env::var("CREDD_SOCKET").is_err() && std::env::var("CREDD_BIND").is_err() {
        std::env::set_var("CREDD_BIND", listen);
    }

    listener::serve(app).await
}
