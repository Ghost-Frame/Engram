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
use axum::Router;
#[cfg(unix)]
use axum::{
    extract::{ConnectInfo, Request},
    middleware::Next,
    serve::IncomingStream,
};
use tracing::{info, warn};

#[cfg(unix)]
use crate::auth::IsUnixSocket;

/// Per-connection info captured from the Unix socket via SO_PEERCRED.
///
/// Implements `axum::extract::connect_info::Connected` so that axum's
/// `into_make_service_with_connect_info` plumbing injects it as a request
/// extension before any middleware runs.
#[cfg(unix)]
#[derive(Clone, Copy, Debug)]
struct UdsConnectInfo {
    /// Kernel-verified peer identity; `None` if `peer_cred()` fails.
    peer: Option<crate::peercred::PeerIdentity>,
}

#[cfg(unix)]
impl axum::extract::connect_info::Connected<IncomingStream<'_, tokio::net::UnixListener>>
    for UdsConnectInfo
{
    /// Called once per accepted connection with a reference to the raw stream.
    /// Reads SO_PEERCRED from the kernel and maps it to `PeerIdentity`.
    fn connect_info(stream: IncomingStream<'_, tokio::net::UnixListener>) -> Self {
        let peer = stream
            .io()
            .peer_cred()
            .ok()
            .map(|ucred| crate::peercred::PeerIdentity {
                uid: ucred.uid(),
                pid: ucred.pid().unwrap_or(0),
            });
        Self { peer }
    }
}

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
///
/// Per-connection SO_PEERCRED is captured via `UdsConnectInfo` and injected
/// as a `crate::peercred::PeerIdentity` extension for handlers that need it.
///
/// Exposed as `pub` so that integration tests can drive the full listener
/// stack (including SO_PEERCRED injection) end-to-end.
#[cfg(unix)]
pub async fn listen_unix_test_only(path: &str, app: Router) -> anyhow::Result<()> {
    listen_unix(path, app).await
}

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

    // Inject ConnectInfo<SocketAddr> (axum extractors require it),
    // IsUnixSocket(true) so the rate limiter knows to skip, and
    // PeerIdentity sourced from the per-connection UdsConnectInfo.
    let fake_addr: SocketAddr = "127.0.0.1:0"
        .parse()
        .expect("hardcoded loopback addr is valid");

    let unix_app = app.layer(axum::middleware::from_fn(
        move |mut req: Request, next: Next| async move {
            // Extract the peer identity injected by UdsConnectInfo at connection
            // time and forward it as a plain PeerIdentity extension so handlers
            // can extract it without knowing about the connect-info wrapper.
            if let Some(uds_info) = req.extensions().get::<ConnectInfo<UdsConnectInfo>>() {
                if let Some(peer) = uds_info.0.peer {
                    req.extensions_mut().insert(peer);
                }
            }
            // Preserve the existing fake ConnectInfo<SocketAddr> and IsUnixSocket
            // extensions that other middleware (rate limiter, auth) depend on.
            req.extensions_mut().insert(ConnectInfo(fake_addr));
            req.extensions_mut().insert(IsUnixSocket(true));
            next.run(req).await
        },
    ));

    axum::serve(
        listener,
        unix_app.into_make_service_with_connect_info::<UdsConnectInfo>(),
    )
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
