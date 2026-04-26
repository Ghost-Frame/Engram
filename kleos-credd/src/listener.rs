//! Listener selection: Unix socket, TCP, or both.
//!
//! Driven by env vars at startup so the same binary serves Linux agents
//! over a 0600 Unix socket (zero-network-attack-surface, owner-UID only)
//! while still answering Windows clients over loopback TCP.
//!
//! - `CREDD_SOCKET`: Unix domain socket path (Linux/macOS). Bound at 0600.
//! - `CREDD_BIND`: TCP listen address (e.g. `127.0.0.1:4400`).
//! - Neither: defaults to TCP `127.0.0.1:4400` for backwards compatibility.
//! - Both: serves on each concurrently via `tokio::try_join!`.

use std::net::SocketAddr;

use anyhow::Context;
use axum::{
    extract::{ConnectInfo, Request},
    middleware::Next,
    Router,
};
use tracing::{info, warn};

use crate::auth::IsUnixSocket;

/// Bind both Unix and TCP listeners as configured. Blocks until one returns.
pub async fn serve(app: Router) -> anyhow::Result<()> {
    let sock = std::env::var("CREDD_SOCKET").ok();
    let bind = std::env::var("CREDD_BIND").ok();

    match (sock, bind) {
        (Some(sock), None) => listen_unix(&sock, app).await,
        (None, Some(bind)) => listen_tcp(&bind, app).await,
        (Some(sock), Some(bind)) => {
            let app_unix = app.clone();
            let app_tcp = app;
            tokio::try_join!(listen_unix(&sock, app_unix), listen_tcp(&bind, app_tcp))?;
            Ok(())
        }
        (None, None) => listen_tcp("127.0.0.1:4400", app).await,
    }
}

/// Bind a Unix domain socket at `path` with mode 0600 and serve `app`.
///
/// Stale socket file (left over from a previous credd that did not shut
/// down cleanly) is removed first. Mode 0600 means only the owning UID
/// can connect, so we mark all incoming requests with `IsUnixSocket(true)`
/// to skip the IP-based rate limiter.
#[cfg(unix)]
async fn listen_unix(path: &str, app: Router) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    // Remove stale socket file if present.
    let _ = std::fs::remove_file(path);

    let listener = tokio::net::UnixListener::bind(path)
        .with_context(|| format!("failed to bind Unix socket at {}", path))?;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("failed to chmod 600 {}", path))?;

    info!("credd listening on unix:{}", path);

    // Inject ConnectInfo<SocketAddr> (axum extractors require it) and
    // IsUnixSocket(true) so the rate limiter knows to skip.
    let fake_addr: SocketAddr = "127.0.0.1:0"
        .parse()
        .expect("hardcoded loopback addr is valid");

    let unix_app = app.layer(axum::middleware::from_fn(
        move |mut req: Request, next: Next| async move {
            req.extensions_mut().insert(ConnectInfo(fake_addr));
            req.extensions_mut().insert(IsUnixSocket(true));
            next.run(req).await
        },
    ));

    axum::serve(listener, unix_app.into_make_service())
        .await
        .context("Unix socket server error")
}

#[cfg(not(unix))]
async fn listen_unix(path: &str, _app: Router) -> anyhow::Result<()> {
    anyhow::bail!(
        "CREDD_SOCKET set to {} but Unix sockets are not supported on this platform",
        path
    )
}

/// Bind a TCP socket at `addr` and serve `app`.
async fn listen_tcp(bind_str: &str, app: Router) -> anyhow::Result<()> {
    let addr: SocketAddr = bind_str
        .parse()
        .with_context(|| format!("invalid CREDD_BIND address: {}", bind_str))?;

    if !addr.ip().is_loopback() {
        warn!(
            "credd is binding to non-loopback address {}. \
             Ensure network access is restricted (firewall, VPN, etc.).",
            addr
        );
    }

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind TCP on {}", addr))?;

    info!("credd listening on tcp:{}", addr);

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .context("TCP server error")
}
