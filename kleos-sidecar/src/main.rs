mod auth;
mod metrics;
mod routes;
mod session;
mod state;
mod syntheos;

use agent_forge::code_context::CodeIndex;
use axum::extract::DefaultBodyLimit;
use clap::Parser;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::RwLock;

use session::SessionManager;
pub use state::{CodeContextMode, SidecarState};

// ---------------------------------------------------------------------------
// Config file schema -- keys mirror the CLI flags.
// Precedence: CLI flag > env var > config file > built-in default.
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ConfigFile {
    port: Option<u16>,
    host: Option<String>,
    session_id: Option<String>,
    source: Option<String>,
    user_id: Option<i64>,
    token: Option<String>,
    #[serde(alias = "kleos_url")]
    kleos_url: Option<String>,
    #[serde(alias = "kleos_api_key")]
    kleos_api_key: Option<String>,
    code_context_mode: Option<CodeContextMode>,
    code_max_tokens: Option<usize>,
    batch_size: Option<usize>,
    batch_interval_ms: Option<u64>,
    max_pending_per_session: Option<usize>,
    retain_every_n_turns: Option<usize>,
    retain_overlap_turns: Option<usize>,
    retain_roles: Option<String>,
    retain_tool_calls: Option<bool>,
    session_idle_ttl_secs: Option<u64>,
    log_format: Option<String>,
}

/// Loads sidecar TOML configuration, falling back to defaults on read or parse errors.
fn load_config_file(path: &str) -> ConfigFile {
    match std::fs::read_to_string(path) {
        Ok(text) => match toml::from_str::<ConfigFile>(&text) {
            Ok(cfg) => {
                tracing::debug!(path = %path, "loaded config file");
                cfg
            }
            Err(e) => {
                eprintln!("warning: could not parse config file {}: {}", path, e);
                ConfigFile::default()
            }
        },
        Err(e) => {
            eprintln!("warning: could not read config file {}: {}", path, e);
            ConfigFile::default()
        }
    }
}

/// Whether a bind host keeps the listener loopback-only, i.e. safe to run
/// without a shared secret. "localhost" and any address that parses to a
/// loopback IP qualify; bind-all ("0.0.0.0"/"::"), concrete external IPs, and
/// unresolved hostnames are treated as non-loopback so they require a token.
fn host_is_loopback(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

// --- CLI ---

#[derive(Parser, Debug, Clone)]
#[command(
    name = "kleos-sidecar",
    about = "Kleos memory sidecar for agent sessions"
)]
/// Defines command-line and environment configuration accepted by the sidecar.
struct Cli {
    /// Path to a TOML config file. Keys mirror CLI flags.
    /// Config file values are overridden by env vars and CLI flags.
    #[arg(long, env = "KLEOS_SIDECAR_CONFIG")]
    config: Option<String>,

    #[arg(short, long, env = "KLEOS_SIDECAR_PORT")]
    port: Option<u16>,

    #[arg(long, env = "KLEOS_SIDECAR_HOST")]
    host: Option<String>,

    #[arg(long)]
    session_id: Option<String>,

    #[arg(long, env = "KLEOS_SIDECAR_SOURCE")]
    source: Option<String>,

    #[arg(long, env = "KLEOS_SIDECAR_USER_ID")]
    user_id: Option<i64>,

    /// Shared-secret token clients must send as `Authorization: Bearer <token>`.
    /// If unset, a fresh token is generated at startup.
    #[arg(long, env = "KLEOS_SIDECAR_TOKEN")]
    token: Option<String>,

    /// Kleos server URL for memory storage/retrieval.
    #[arg(long, env = "KLEOS_URL")]
    kleos_url: Option<String>,

    /// API key for authenticating with the Kleos server.
    #[arg(long, env = "KLEOS_API_KEY")]
    kleos_api_key: Option<String>,

    /// Local code retrieval policy: off, shadow, or inject.
    #[arg(long, env = "KLEOS_CODE_CONTEXT_MODE", value_enum)]
    code_context_mode: Option<CodeContextMode>,

    /// Maximum approximate code tokens selected for a prompt.
    #[arg(long, env = "KLEOS_CODE_MAX_TOKENS")]
    code_max_tokens: Option<usize>,

    /// Size-based flush threshold.
    #[arg(long, env = "KLEOS_SIDECAR_BATCH_SIZE")]
    batch_size: Option<usize>,

    /// Time-based flush interval (milliseconds).
    #[arg(long, env = "KLEOS_SIDECAR_BATCH_INTERVAL_MS")]
    batch_interval_ms: Option<u64>,

    /// Maximum observations held in pending before /observe returns 503.
    #[arg(long, env = "KLEOS_SIDECAR_MAX_PENDING")]
    max_pending_per_session: Option<usize>,

    /// Flush pending observations every N retained turns.
    #[arg(long, env = "KLEOS_RETAIN_EVERY_N_TURNS")]
    retain_every_n_turns: Option<usize>,

    /// Keep this many trailing observations buffered across automatic flushes.
    #[arg(long, env = "KLEOS_RETAIN_OVERLAP_TURNS")]
    retain_overlap_turns: Option<usize>,

    /// Comma-separated roles that should be retained by /observe.
    #[arg(long, env = "KLEOS_RETAIN_ROLES")]
    retain_roles: Option<String>,

    /// Whether raw tool-role observations are retained. Defaults to false: raw
    /// tool output is uninterpreted noise that floods the memory store and
    /// degrades recall (every tool result shipped verbatim as a "discovery"
    /// memory). When false, tool-role observations are filtered out at /observe.
    /// Set true to retain and persist them verbatim.
    #[arg(long, env = "KLEOS_RETAIN_TOOL_CALLS")]
    retain_tool_calls: Option<bool>,

    /// Sessions idle longer than this are removed from memory (seconds). Default 86400.
    #[arg(long, env = "KLEOS_SIDECAR_SESSION_IDLE_TTL_SECS")]
    session_idle_ttl_secs: Option<u64>,

    /// Log output format: "text" (default, human-readable) or "json" (structured).
    #[arg(long, env = "KLEOS_SIDECAR_LOG_FORMAT", default_value = "text")]
    log_format: String,
}

// --- Resolved config -- the merged result of config file + env + CLI. ---

struct ResolvedConfig {
    port: u16,
    host: String,
    session_id: Option<String>,
    source: String,
    user_id: i64,
    token: Option<String>,
    kleos_url: String,
    kleos_api_key: Option<String>,
    code_context_mode: CodeContextMode,
    code_max_tokens: usize,
    batch_size: usize,
    batch_interval_ms: u64,
    max_pending_per_session: usize,
    retain_every_n_turns: usize,
    retain_overlap_turns: usize,
    retain_roles: String,
    retain_tool_calls: bool,
    session_idle_ttl_secs: u64,
    log_format: String,
}

/// Merge CLI > config file > built-in defaults. clap already handles env var
/// resolution so any Some() value on the Cli struct already reflects
/// CLI-or-env. We only fall back to config file when the CLI/env produced None.
fn resolve_config(cli: Cli, cfg: ConfigFile) -> ResolvedConfig {
    macro_rules! pick {
        ($cli_val:expr, $cfg_val:expr, $default:expr) => {
            $cli_val.or($cfg_val).unwrap_or($default)
        };
    }

    let kleos_url = pick!(
        cli.kleos_url,
        cfg.kleos_url,
        String::from("http://127.0.0.1:4200")
    );

    ResolvedConfig {
        port: pick!(cli.port, cfg.port, 7711),
        host: pick!(cli.host, cfg.host, String::from("127.0.0.1")),
        session_id: cli.session_id.or(cfg.session_id),
        source: pick!(cli.source, cfg.source, String::from("sidecar")),
        user_id: pick!(cli.user_id, cfg.user_id, 1_i64),
        token: cli.token.or(cfg.token),
        kleos_api_key: cli.kleos_api_key.or(cfg.kleos_api_key),
        code_context_mode: pick!(
            cli.code_context_mode,
            cfg.code_context_mode,
            CodeContextMode::Shadow
        ),
        code_max_tokens: pick!(cli.code_max_tokens, cfg.code_max_tokens, 2_000_usize),
        batch_size: pick!(cli.batch_size, cfg.batch_size, 10_usize),
        batch_interval_ms: pick!(cli.batch_interval_ms, cfg.batch_interval_ms, 2000_u64),
        max_pending_per_session: pick!(
            cli.max_pending_per_session,
            cfg.max_pending_per_session,
            5000_usize
        ),
        retain_every_n_turns: pick!(cli.retain_every_n_turns, cfg.retain_every_n_turns, 10_usize),
        retain_overlap_turns: pick!(cli.retain_overlap_turns, cfg.retain_overlap_turns, 2_usize),
        retain_roles: pick!(
            cli.retain_roles,
            cfg.retain_roles,
            String::from("user,assistant,tool")
        ),
        retain_tool_calls: pick!(cli.retain_tool_calls, cfg.retain_tool_calls, false),
        session_idle_ttl_secs: pick!(
            cli.session_idle_ttl_secs,
            cfg.session_idle_ttl_secs,
            86_400_u64
        ),
        log_format: {
            let from_cfg = cfg.log_format.unwrap_or_default();
            // cli.log_format always has a value (default_value = "text"); only override
            // with config file when the CLI is still at its default and config says "json".
            if cli.log_format == "text" && from_cfg == "json" {
                "json".to_string()
            } else {
                cli.log_format
            }
        },
        kleos_url,
    }
}

// --- main ---

#[tokio::main]
async fn main() {
    kleos_lib::config::migrate_env_prefix();

    let cli = Cli::parse();

    // Load config file before tracing init so format is known.
    let cfg_file = cli
        .config
        .as_deref()
        .map(load_config_file)
        .unwrap_or_default();

    let mut rc = resolve_config(cli, cfg_file);

    // Init tracing. JSON format wires a JSON layer; text uses the default fmt.
    if rc.log_format == "json" {
        init_json_tracing();
    } else {
        let _guard = kleos_lib::observability::init_tracing("kleos-sidecar", "kleos_sidecar=debug");
        // Note: _guard is intentionally not held for the process lifetime here;
        // the OTel shutdown happens at process exit via the guard's Drop. In
        // json mode we skip OTel to avoid the extra dependency on the recorder.
        std::mem::forget(_guard);
    }

    // Build the tiered request signer (PIV > ed25519 > none) as the PRIMARY
    // Kleos auth. agent_label is the audit identity the server records, so the
    // sidecar's stored memories are attributed to "kleos-sidecar" regardless of
    // which tier (PIV on Master's box, ed25519 on no-YubiKey hosts) authenticates.
    let host_label = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("HOST"))
        .unwrap_or_else(|_| "unknown".to_string());
    let agent_label =
        std::env::var("KLEOS_AGENT_LABEL").unwrap_or_else(|_| "kleos-sidecar".to_string());
    let signer =
        kleos_lib::auth_piv::RequestSigner::from_env_or_file(&host_label, &agent_label, "daemon")
            .ok()
            .flatten()
            .map(std::sync::Arc::new);
    if let Some(ref s) = signer {
        tracing::info!(
            tier = s.tier(),
            agent_label = %s.agent_label(),
            "sidecar request signer initialized (primary auth)"
        );
    } else {
        tracing::info!("no request signer; using bearer API key auth");
    }

    // Bearer fallback: resolve a key from credd ONLY when there is no signer.
    // When a signer exists it is the sole auth path, so we must NOT enter the
    // credd ECDH/PIV bootstrap -- that path consumes a PIV PIN attempt and is
    // exactly what burns Master's YubiKey PIN on keyless starts.
    if rc.kleos_api_key.is_none() && signer.is_none() {
        let slot = kleos_lib::cred::bootstrap::current_agent_slot();
        match kleos_lib::cred::bootstrap::resolve_api_key(&slot).await {
            Ok(k) => {
                tracing::debug!(slot = %slot, "resolved kleos API key from credd");
                rc.kleos_api_key = Some(k);
            }
            Err(e) => tracing::warn!("could not resolve kleos API key from credd: {}", e),
        }
    }

    metrics::init();

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("failed to create HTTP client");

    let code_index = match CodeIndex::open_default() {
        Ok(index) => Some(Arc::new(index)),
        Err(error) => {
            tracing::warn!(%error, "local code index unavailable; retrieval will fail open");
            None
        }
    };

    let session_id = rc
        .session_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    tracing::info!(default_session_id = %session_id, "starting sidecar (multi-session enabled)");

    let token = rc
        .token
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    if token.is_some() {
        tracing::info!("sidecar shared-secret auth enabled");
    } else if host_is_loopback(&rc.host) {
        tracing::info!(
            host = %rc.host,
            "no KLEOS_SIDECAR_TOKEN set; running without auth (loopback-only)"
        );
    } else {
        // The sidecar holds a tenant-scoped DB handle; binding it to a
        // non-loopback address with no shared secret exposes that tenant's
        // memories to the network unauthenticated (the require_token
        // middleware skips auth when no token is configured). Fail fast rather
        // than serve open.
        eprintln!(
            "refusing to start: sidecar bound to non-loopback host '{}' without \
             KLEOS_SIDECAR_TOKEN. Set a token or bind to 127.0.0.1.",
            rc.host
        );
        std::process::exit(1);
    }

    let manager = SessionManager::new(session_id);

    let syntheos_client = Arc::new(syntheos::SyntheosClient::new_from_env(
        client.clone(),
        rc.kleos_url.clone(),
        rc.kleos_api_key.clone(),
    ));

    let state = SidecarState {
        client,
        kleos_url: rc.kleos_url,
        kleos_api_key: rc.kleos_api_key,
        signer,
        code_index,
        code_context_mode: rc.code_context_mode,
        code_max_tokens: rc.code_max_tokens.max(1),
        refreshing_repositories: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        sessions: Arc::new(RwLock::new(manager)),
        source: rc.source,
        user_id: rc.user_id,
        token,
        batch_size: rc.batch_size.max(1),
        batch_interval_ms: rc.batch_interval_ms,
        max_pending_per_session: rc.max_pending_per_session.max(1),
        retain_every_n: rc.retain_every_n_turns.max(1),
        overlap_turns: rc.retain_overlap_turns,
        retain_roles: rc
            .retain_roles
            .split(',')
            .map(str::trim)
            .filter(|role| !role.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        retain_tool_calls: rc.retain_tool_calls,
        syntheos: syntheos_client,
    };

    tracing::info!(
        batch_size = state.batch_size,
        batch_interval_ms = state.batch_interval_ms,
        max_pending_per_session = state.max_pending_per_session,
        retain_every_n = state.retain_every_n,
        overlap_turns = state.overlap_turns,
        retain_roles = ?state.retain_roles,
        retain_tool_calls = state.retain_tool_calls,
        code_context_mode = state.code_context_mode.as_str(),
        code_index_available = state.code_index.is_some(),
        code_max_tokens = state.code_max_tokens,
        "observation batching configured"
    );

    // Time-based batch flusher.
    if state.batch_interval_ms > 0 {
        let flusher_state = state.clone();
        let interval_ms = state.batch_interval_ms;
        let tick_ms = interval_ms.div_ceil(2).max(100);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_millis(tick_ms));
            tick.tick().await;
            let threshold = std::time::Duration::from_millis(interval_ms);
            loop {
                tick.tick().await;
                let candidates: Vec<String> = {
                    let guard = flusher_state.sessions.read().await;
                    guard
                        .list()
                        .into_iter()
                        .filter(|info| info.pending_count > 0 && !info.ended)
                        .map(|info| info.id)
                        .collect()
                };
                for sid in candidates {
                    let due = {
                        let guard = flusher_state.sessions.read().await;
                        guard
                            .get(&sid)
                            .and_then(|s| s.pending_since)
                            .map(|t| t.elapsed() >= threshold)
                            .unwrap_or(false)
                    };
                    if due {
                        let flushed = routes::flush_pending(&flusher_state, &sid).await;
                        if flushed > 0 {
                            tracing::debug!(
                                session_id = %sid,
                                flushed,
                                "time-based batch flush"
                            );
                        }
                    }
                }
            }
        });
        tracing::info!(
            interval_ms = state.batch_interval_ms,
            tick_ms,
            "time-based batch flusher started"
        );
    } else {
        tracing::info!("time-based batch flusher disabled (batch_interval_ms=0)");
    }

    // Idle session sweep -- runs every 5 minutes, expires sessions idle > ttl.
    {
        let sweep_sessions = state.sessions.clone();
        let idle_ttl = std::time::Duration::from_secs(rc.session_idle_ttl_secs);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(300));
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let expired = {
                    let mut guard = sweep_sessions.write().await;
                    guard.expire_idle(idle_ttl)
                };
                if expired > 0 {
                    tracing::info!(expired, "idle session sweep removed expired sessions");
                }
            }
        });
        tracing::info!(
            idle_ttl_secs = rc.session_idle_ttl_secs,
            "idle session sweep started (5-minute interval)"
        );
    }

    let app = routes::router(state.clone()).layer(DefaultBodyLimit::max(8 * 1024 * 1024));

    let addr = format!("{}:{}", rc.host, rc.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("failed to bind");

    tracing::info!(addr = %addr, "sidecar listening");

    // Soma registration and 60s heartbeat (opt-in via KLEOS_SIDECAR_SYNTHEOS=1).
    if state.syntheos.enabled {
        state
            .syntheos
            .register_soma_agent(
                "kleos-sidecar",
                "system",
                &["observe", "recall", "code-context"],
            )
            .await;

        let hb_syntheos = state.syntheos.clone();
        let hb_sessions = state.sessions.clone();
        let hb_code_index_available = state.code_index.is_some();
        let hb_code_context_mode = state.code_context_mode.as_str();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let (active_sessions, pending_depth) = {
                    let guard = hb_sessions.read().await;
                    let active = guard.active_count();
                    let pending: usize = guard.list().iter().map(|s| s.pending_count).sum();
                    (active, pending)
                };
                hb_syntheos.soma_heartbeat(
                    "kleos-sidecar",
                    serde_json::json!({
                        "active_sessions": active_sessions,
                        "pending_depth": pending_depth,
                        "code_context_mode": hb_code_context_mode,
                        "code_index_available": hb_code_index_available,
                    }),
                );
            }
        });
    }

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(state.clone()))
        .await
        .expect("server error");
}

// ---------------------------------------------------------------------------
// JSON tracing init -- separate from kleos_lib's init_tracing so we don't
// need to fork that crate for a single-line format change.
// ---------------------------------------------------------------------------

fn init_json_tracing() {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("kleos_sidecar=debug,info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().json())
        .init();
}

// --- Graceful shutdown ---

/// Wait for SIGTERM or Ctrl-C. On signal, flush all pending sessions with a
/// 10s deadline before returning so in-flight observations reach Kleos.
async fn shutdown_signal(state: SidecarState) {
    wait_for_signal().await;
    tracing::info!("shutdown signal received; flushing pending observations");
    match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        routes::flush_all_sessions(&state),
    )
    .await
    {
        Ok(()) => tracing::info!("graceful shutdown: all sessions flushed"),
        Err(_) => tracing::warn!("graceful shutdown: flush timed out after 10s"),
    }
}

#[cfg(unix)]
/// Waits for SIGTERM or Ctrl-C on Unix before graceful shutdown.
async fn wait_for_signal() {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigterm = signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
    tokio::select! {
        _ = sigterm.recv() => {},
        _ = tokio::signal::ctrl_c() => {},
    }
}

#[cfg(windows)]
/// Waits for Windows console shutdown signals before graceful shutdown.
async fn wait_for_signal() {
    use tokio::signal::windows;

    let mut ctrl_c = windows::ctrl_c().expect("failed to install Ctrl-C handler");
    let mut ctrl_break = windows::ctrl_break().expect("failed to install Ctrl-Break handler");
    let mut ctrl_close = windows::ctrl_close().expect("failed to install Ctrl-Close handler");
    let mut ctrl_shutdown =
        windows::ctrl_shutdown().expect("failed to install Ctrl-Shutdown handler");

    tokio::select! {
        _ = ctrl_c.recv() => {},
        _ = ctrl_break.recv() => {},
        _ = ctrl_close.recv() => {},
        _ = ctrl_shutdown.recv() => {},
    }
}
