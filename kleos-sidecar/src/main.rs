mod auth;
mod metrics;
mod routes;
mod session;
mod state;
mod syntheos;
mod watcher;

use axum::extract::DefaultBodyLimit;
use clap::Parser;
use kleos_lib::llm::{local::LocalModelClient, OllamaConfig};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use session::SessionManager;
pub use state::SidecarState;

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
    #[serde(alias = "engram_url")]
    kleos_url: Option<String>,
    #[serde(alias = "engram_api_key")]
    kleos_api_key: Option<String>,
    watch: Option<bool>,
    watch_dir: Option<String>,
    watcher_state_path: Option<String>,
    batch_size: Option<usize>,
    batch_interval_ms: Option<u64>,
    max_pending_per_session: Option<usize>,
    compress_passthrough_bytes: Option<usize>,
    compress_max_input_bytes: Option<usize>,
    compress_timeout_ms: Option<u64>,
    session_idle_ttl_secs: Option<u64>,
    log_format: Option<String>,
}

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

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser, Debug, Clone)]
#[command(
    name = "engram-sidecar",
    about = "Engram memory sidecar for agent sessions"
)]
struct Cli {
    /// Path to a TOML config file. Keys mirror CLI flags.
    /// Config file values are overridden by env vars and CLI flags.
    #[arg(long, env = "ENGRAM_SIDECAR_CONFIG")]
    config: Option<String>,

    #[arg(short, long, env = "ENGRAM_SIDECAR_PORT")]
    port: Option<u16>,

    #[arg(long, env = "ENGRAM_SIDECAR_HOST")]
    host: Option<String>,

    #[arg(long)]
    session_id: Option<String>,

    #[arg(long, env = "ENGRAM_SIDECAR_SOURCE")]
    source: Option<String>,

    #[arg(long, env = "ENGRAM_SIDECAR_USER_ID")]
    user_id: Option<i64>,

    /// Shared-secret token clients must send as `Authorization: Bearer <token>`.
    /// If unset, a fresh token is generated at startup.
    #[arg(long, env = "ENGRAM_SIDECAR_TOKEN")]
    token: Option<String>,

    /// Kleos server URL for memory storage/retrieval.
    #[arg(long, env = "KLEOS_URL")]
    kleos_url: Option<String>,

    /// API key for authenticating with the Kleos server.
    #[arg(long, env = "KLEOS_API_KEY")]
    kleos_api_key: Option<String>,

    /// Enable file watcher for Claude Code session JSONL files.
    #[arg(long, env = "ENGRAM_SIDECAR_WATCH")]
    watch: bool,

    /// Directory to watch for session files (default: ~/.claude/projects).
    #[arg(long, env = "CLAUDE_SESSIONS_DIR")]
    watch_dir: Option<String>,

    /// Path to the watcher position checkpoint JSON file.
    #[arg(long, env = "ENGRAM_SIDECAR_WATCHER_STATE_PATH")]
    watcher_state_path: Option<PathBuf>,

    /// Size-based flush threshold.
    #[arg(long, env = "ENGRAM_SIDECAR_BATCH_SIZE")]
    batch_size: Option<usize>,

    /// Time-based flush interval (milliseconds).
    #[arg(long, env = "ENGRAM_SIDECAR_BATCH_INTERVAL_MS")]
    batch_interval_ms: Option<u64>,

    /// Maximum observations held in pending before /observe returns 503.
    #[arg(long, env = "ENGRAM_SIDECAR_MAX_PENDING")]
    max_pending_per_session: Option<usize>,

    /// Byte threshold below which /compress passes content through without LLM.
    #[arg(long, env = "ENGRAM_SIDECAR_COMPRESS_PASSTHROUGH_BYTES")]
    compress_passthrough_bytes: Option<usize>,

    /// Maximum input bytes sent to the LLM for compression. Requests larger than
    /// this are rejected with 413 rather than silently truncated.
    #[arg(long, env = "ENGRAM_SIDECAR_COMPRESS_MAX_INPUT_BYTES")]
    compress_max_input_bytes: Option<usize>,

    /// LLM call timeout for /compress (milliseconds).
    #[arg(long, env = "ENGRAM_SIDECAR_COMPRESS_TIMEOUT_MS")]
    compress_timeout_ms: Option<u64>,

    /// Sessions idle longer than this are removed from memory (seconds). Default 86400.
    #[arg(long, env = "ENGRAM_SIDECAR_SESSION_IDLE_TTL_SECS")]
    session_idle_ttl_secs: Option<u64>,

    /// Log output format: "text" (default, human-readable) or "json" (structured).
    #[arg(long, env = "ENGRAM_SIDECAR_LOG_FORMAT", default_value = "text")]
    log_format: String,
}

// ---------------------------------------------------------------------------
// Resolved config -- the merged result of config file + env + CLI.
// ---------------------------------------------------------------------------

struct ResolvedConfig {
    port: u16,
    host: String,
    session_id: Option<String>,
    source: String,
    user_id: i64,
    token: Option<String>,
    kleos_url: String,
    kleos_api_key: Option<String>,
    watch: bool,
    watch_dir: Option<String>,
    watcher_state_path: Option<PathBuf>,
    batch_size: usize,
    batch_interval_ms: u64,
    max_pending_per_session: usize,
    compress_passthrough_bytes: usize,
    compress_max_input_bytes: usize,
    compress_timeout_ms: u64,
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
        watch: cli.watch || cfg.watch.unwrap_or(false),
        watch_dir: cli.watch_dir.or(cfg.watch_dir),
        watcher_state_path: cli
            .watcher_state_path
            .or_else(|| cfg.watcher_state_path.map(PathBuf::from)),
        batch_size: pick!(cli.batch_size, cfg.batch_size, 10_usize),
        batch_interval_ms: pick!(cli.batch_interval_ms, cfg.batch_interval_ms, 2000_u64),
        max_pending_per_session: pick!(
            cli.max_pending_per_session,
            cfg.max_pending_per_session,
            5000_usize
        ),
        compress_passthrough_bytes: pick!(
            cli.compress_passthrough_bytes,
            cfg.compress_passthrough_bytes,
            2000_usize
        ),
        compress_max_input_bytes: pick!(
            cli.compress_max_input_bytes,
            cfg.compress_max_input_bytes,
            50_000_usize
        ),
        compress_timeout_ms: pick!(cli.compress_timeout_ms, cfg.compress_timeout_ms, 10_000_u64),
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

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

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

    let rc = resolve_config(cli, cfg_file);

    // Init tracing. JSON format wires a JSON layer; text uses the default fmt.
    if rc.log_format == "json" {
        init_json_tracing();
    } else {
        let _guard =
            kleos_lib::observability::init_tracing("engram-sidecar", "kleos_sidecar=debug");
        // Note: _guard is intentionally not held for the process lifetime here;
        // the OTel shutdown happens at process exit via the guard's Drop. In
        // json mode we skip OTel to avoid the extra dependency on the recorder.
        std::mem::forget(_guard);
    }

    metrics::init();

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("failed to create HTTP client");

    let llm: Arc<LocalModelClient> = {
        let llm_config = OllamaConfig::from_env();
        let client = LocalModelClient::new(llm_config);
        if client.probe().await {
            tracing::info!("local LLM client ready for sidecar");
        } else {
            tracing::warn!(
                "local LLM unavailable at startup -- will re-probe on first compress request"
            );
        }
        Arc::new(client)
    };

    let session_id = rc
        .session_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    tracing::info!(default_session_id = %session_id, "starting sidecar (multi-session enabled)");

    let token = match rc.token.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(t) => Some(t.to_string()),
        None => {
            let generated = auth::generate_token();
            tracing::warn!(
                host = %rc.host,
                "ENGRAM_SIDECAR_TOKEN not set; generated one-time sidecar token (printed to stderr)"
            );
            // SECURITY (SEC-LOW-5): print token once to stderr so the launching
            // process can capture it. Full value intentionally not in logs.
            eprintln!("SIDECAR_TOKEN={}", generated);
            tracing::debug!(
                token_prefix = &generated[..8.min(generated.len())],
                "sidecar token generated (see stderr for full value)"
            );
            Some(generated)
        }
    };
    if token.is_some() {
        tracing::info!("sidecar shared-secret auth enabled");
    } else {
        tracing::info!(
            host = %rc.host,
            "no ENGRAM_SIDECAR_TOKEN set; running without auth (localhost-only)"
        );
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
        llm,
        sessions: Arc::new(RwLock::new(manager)),
        source: rc.source,
        user_id: rc.user_id,
        token,
        batch_size: rc.batch_size.max(1),
        batch_interval_ms: rc.batch_interval_ms,
        max_pending_per_session: rc.max_pending_per_session.max(1),
        compress_passthrough_bytes: rc.compress_passthrough_bytes,
        compress_max_input_bytes: rc.compress_max_input_bytes,
        compress_timeout_ms: rc.compress_timeout_ms,
        syntheos: syntheos_client,
    };

    tracing::info!(
        batch_size = state.batch_size,
        batch_interval_ms = state.batch_interval_ms,
        max_pending_per_session = state.max_pending_per_session,
        "observation batching configured"
    );

    if rc.watch {
        if let Some(ref dir) = rc.watch_dir {
            std::env::set_var("CLAUDE_SESSIONS_DIR", dir);
        }
        // If a custom checkpoint path was provided, wire it via env so watcher picks it up.
        if let Some(ref cp) = rc.watcher_state_path {
            std::env::set_var("ENGRAM_SIDECAR_WATCHER_STATE_PATH", cp);
        }
        let _watcher_handle = watcher::start(state.clone());
    }

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

    // Soma registration and 60s heartbeat (opt-in via ENGRAM_SIDECAR_SYNTHEOS=1).
    if state.syntheos.enabled {
        state
            .syntheos
            .register_soma_agent(
                "kleos-sidecar",
                "system",
                &["observe", "compress", "recall"],
            )
            .await;

        let hb_syntheos = state.syntheos.clone();
        let hb_sessions = state.sessions.clone();
        let hb_llm = state.llm.clone();
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
                let llm_available = hb_llm.is_available();
                hb_syntheos.soma_heartbeat(
                    "kleos-sidecar",
                    serde_json::json!({
                        "active_sessions": active_sessions,
                        "pending_depth": pending_depth,
                        "llm_available": llm_available,
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

// ---------------------------------------------------------------------------
// Graceful shutdown
// ---------------------------------------------------------------------------

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
async fn wait_for_signal() {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigterm = signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
    tokio::select! {
        _ = sigterm.recv() => {},
        _ = tokio::signal::ctrl_c() => {},
    }
}

#[cfg(windows)]
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
