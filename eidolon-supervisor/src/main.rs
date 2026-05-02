use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing_subscriber::EnvFilter;

mod alert;
mod checks;
mod watch;

use watch::SupervisorState;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = load_config();
    tracing::info!(
        watch_dir = %config.watch_dir.display(),
        rules = config.rules.len(),
        "eidolon-supervisor starting"
    );

    let state = Arc::new(SupervisorState {
        kleos_url: config.kleos_url,
        api_key: config.api_key,
        rules: config.rules,
        cooldowns: RwLock::new(std::collections::HashMap::new()),
        client: reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("failed to build HTTP client"),
    });

    watch::run(state, config.watch_dir).await;
}

struct Config {
    watch_dir: PathBuf,
    kleos_url: String,
    api_key: Option<String>,
    rules: Vec<checks::Rule>,
}

fn load_config() -> Config {
    let watch_dir = std::env::var("CLAUDE_SESSIONS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".claude")
                .join("projects")
        });

    let kleos_url = std::env::var("KLEOS_SERVER_URL")
        .or_else(|_| std::env::var("ENGRAM_EIDOLON_URL"))
        .unwrap_or_else(|_| "http://127.0.0.1:4200".to_string());

    let api_key = std::env::var("KLEOS_API_KEY")
        .or_else(|_| std::env::var("EIDOLON_KEY"))
        .ok()
        .filter(|k| !k.is_empty());

    let rules = load_rules();

    Config {
        watch_dir,
        kleos_url,
        api_key,
        rules,
    }
}

fn load_rules() -> Vec<checks::Rule> {
    let config_path = std::env::var("EIDOLON_SUPERVISOR_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".config")
                .join("eidolon")
                .join("supervisor.json")
        });

    if config_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&config_path) {
            if let Ok(rules) = serde_json::from_str::<Vec<checks::Rule>>(&content) {
                return rules;
            }
            tracing::warn!(path = %config_path.display(), "failed to parse rules, using defaults");
        }
    }

    checks::default_rules()
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}
