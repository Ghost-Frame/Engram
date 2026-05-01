use std::path::PathBuf;

pub struct Config {
    pub watch_dir: PathBuf,
    pub kleos_url: String,
    pub api_key: Option<String>,
    pub summary_idle_secs: u64,
    pub novelty_drop_threshold: f64,
    pub ledger_path: PathBuf,
    pub host: String,
}

impl Config {
    pub fn from_env() -> Self {
        let watch_dir = std::env::var("KLEOS_INGEST_WATCH_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".claude")
                    .join("projects")
            });

        let kleos_url = std::env::var("KLEOS_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:4200".to_string());

        let api_key = std::env::var("KLEOS_API_KEY").ok().filter(|k| !k.is_empty());

        let summary_idle_secs: u64 = std::env::var("KLEOS_INGEST_IDLE_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(300);

        let ledger_path = std::env::var("KLEOS_INGEST_LEDGER")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                dirs::data_local_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("kleos-ingest")
                    .join("ledger.db")
            });

        let host = std::env::var("KLEOS_INGEST_HOST")
            .unwrap_or_else(|_| {
                std::fs::read_to_string("/etc/hostname")
                    .unwrap_or_else(|_| "unknown".to_string())
                    .trim()
                    .to_string()
            });

        Config {
            watch_dir,
            kleos_url,
            api_key,
            summary_idle_secs,
            novelty_drop_threshold: 0.92,
            ledger_path,
            host,
        }
    }
}
