use secrecy::SecretString;
use serde::{Deserialize, Serialize};

/// How the at-rest encryption key is sourced.
///
/// Default is `None` (no encryption). When set, every SQLite connection
/// issues `PRAGMA key` as its first statement so the database file is
/// encrypted via SQLCipher.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum EncryptionMode {
    /// No encryption -- database opens without PRAGMA key.
    #[default]
    None,
    /// Read a raw 32-byte key from `~/.config/engram/dbkey`.
    Keyfile,
    /// Hex-decode the `ENGRAM_DB_KEY` env var (64 hex chars = 32 bytes).
    Env,
    /// YubiKey HMAC-SHA1 challenge-response on slot 2, derived via Argon2id.
    Yubikey,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct EncryptionConfig {
    #[serde(default)]
    pub mode: EncryptionMode,
}

/// A server entry used for SSH validation and reboot protection.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServerEntry {
    /// Canonical name used to identify this server.
    pub name: String,
    /// Alternate hostnames / aliases for this server.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Human-readable role description.
    #[serde(default)]
    pub role: String,
    /// SSH user to use when connecting.
    #[serde(default)]
    pub ssh_user: String,
    /// SSH port (default 22).
    #[serde(default = "default_ssh_port")]
    pub ssh_port: u16,
    /// When true, a non-default port is required.
    #[serde(default)]
    pub custom_port_required: bool,
    /// When true, reboot/shutdown commands targeting this server are blocked.
    #[serde(default)]
    pub no_reboot: bool,
    /// Operational notes shown to the agent on SSH enrichment.
    #[serde(default)]
    pub notes: String,
}

fn default_ssh_port() -> u16 {
    22
}

fn default_backup_retention_daily() -> usize {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GateConfig {
    pub blocked_patterns: Vec<String>,
    pub reserved_targets: Vec<String>,
    pub approval_timeout_secs: u64,
    /// Services that must not be stopped or restarted without explicit confirmation.
    #[serde(default)]
    pub protected_services: Vec<String>,
    /// Known server inventory used for SSH validation and reboot protection.
    #[serde(default)]
    pub servers: Vec<ServerEntry>,
}

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            blocked_patterns: vec![
                "rm -rf /".to_string(),
                "rm -rf ~".to_string(),
                "mkfs".to_string(),
                "dd if=".to_string(),
                ":(){ :|:& };:".to_string(),
                "reboot".to_string(),
                "shutdown".to_string(),
                "halt".to_string(),
                "> /dev/sda".to_string(),
                "chmod -R 777 /".to_string(),
            ],
            reserved_targets: Vec::new(),
            approval_timeout_secs: 300,
            protected_services: Vec::new(),
            servers: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GrowthConfig {
    pub reflection_interval_secs: u64,
    pub observation_limit: usize,
}

impl Default for GrowthConfig {
    fn default() -> Self {
        Self {
            reflection_interval_secs: 3600,
            observation_limit: 100,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionsConfig {
    pub max_concurrent: usize,
    pub buffer_size: usize,
    pub stream_timeout_secs: u64,
    pub scrub_secrets: bool,
}

impl Default for SessionsConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 64,
            buffer_size: 4096,
            stream_timeout_secs: 1800,
            scrub_secrets: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PromptConfig {
    pub default_max_tokens: usize,
    pub personality_weight: f32,
    pub default_include_memories: bool,
    pub default_include_personality: bool,
    pub max_tokens_cap: usize,
}

impl Default for PromptConfig {
    fn default() -> Self {
        Self {
            default_max_tokens: 4000,
            personality_weight: 0.3,
            default_include_memories: true,
            default_include_personality: true,
            max_tokens_cap: 128000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CreddConfig {
    pub url: String,
    pub agent_key_env: String,
    pub allow_raw: bool,
    pub cache_ttl_secs: u64,
}

impl Default for CreddConfig {
    fn default() -> Self {
        Self {
            url: "http://127.0.0.1:4400".to_string(),
            agent_key_env: "CREDD_AGENT_KEY".to_string(),
            allow_raw: false,
            cache_ttl_secs: 60,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EidolonConfig {
    pub enabled: bool,
    pub url: Option<String>,
    #[serde(skip, default)]
    pub api_key: Option<SecretString>,
    #[serde(default)]
    pub credd: CreddConfig,
    #[serde(default)]
    pub gate: GateConfig,
    #[serde(default)]
    pub growth: GrowthConfig,
    #[serde(default)]
    pub sessions: SessionsConfig,
    #[serde(default)]
    pub prompt: PromptConfig,
}

impl EidolonConfig {
    pub fn from_env() -> Self {
        Self::default().apply_env()
    }

    /// Apply environment-variable overrides on top of `self`. Used to layer
    /// env on top of a TOML-loaded base so env always wins.
    pub fn apply_env(mut self) -> Self {
        if let Ok(v) = std::env::var("ENGRAM_EIDOLON_ENABLED") {
            self.enabled = matches!(v.as_str(), "1" | "true" | "TRUE" | "yes");
        }
        if let Ok(v) = std::env::var("ENGRAM_EIDOLON_URL") {
            self.url = Some(v);
        }
        if let Ok(v) = std::env::var("ENGRAM_EIDOLON_API_KEY") {
            self.api_key = Some(SecretString::new(v));
        }
        let c = &mut self;
        if let Ok(v) = std::env::var("CREDD_URL") {
            c.credd.url = v;
        }
        if let Ok(v) = std::env::var("ENGRAM_CREDD_AGENT_KEY_ENV") {
            c.credd.agent_key_env = v;
        }
        if let Ok(v) = std::env::var("ENGRAM_CREDD_ALLOW_RAW") {
            c.credd.allow_raw = matches!(v.as_str(), "1" | "true" | "TRUE" | "yes");
        }
        if let Ok(v) = std::env::var("ENGRAM_CREDD_CACHE_TTL_SECS") {
            if let Ok(n) = v.parse() {
                c.credd.cache_ttl_secs = n;
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_EIDOLON_GATE_APPROVAL_TIMEOUT") {
            if let Ok(n) = v.parse() {
                c.gate.approval_timeout_secs = n;
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_EIDOLON_GATE_BLOCKED_PATTERNS") {
            c.gate.blocked_patterns = v.split(',').map(|s| s.trim().to_string()).collect();
        }
        if let Ok(v) = std::env::var("ENGRAM_EIDOLON_GATE_RESERVED_TARGETS") {
            c.gate.reserved_targets = v.split(',').map(|s| s.trim().to_string()).collect();
        }
        if let Ok(v) = std::env::var("ENGRAM_EIDOLON_GROWTH_INTERVAL") {
            if let Ok(n) = v.parse() {
                c.growth.reflection_interval_secs = n;
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_EIDOLON_GROWTH_OBSERVATION_LIMIT") {
            if let Ok(n) = v.parse() {
                c.growth.observation_limit = n;
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_EIDOLON_SESSIONS_MAX") {
            if let Ok(n) = v.parse() {
                c.sessions.max_concurrent = n;
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_EIDOLON_SESSIONS_BUFFER") {
            if let Ok(n) = v.parse() {
                c.sessions.buffer_size = n;
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_EIDOLON_SESSIONS_STREAM_TIMEOUT") {
            if let Ok(n) = v.parse() {
                c.sessions.stream_timeout_secs = n;
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_EIDOLON_SESSIONS_SCRUB_SECRETS") {
            c.sessions.scrub_secrets = matches!(v.as_str(), "1" | "true" | "TRUE" | "yes");
        }
        if let Ok(v) = std::env::var("ENGRAM_EIDOLON_PROMPT_MAX_TOKENS") {
            if let Ok(n) = v.parse() {
                c.prompt.default_max_tokens = n;
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_EIDOLON_PROMPT_MAX_TOKENS_CAP") {
            if let Ok(n) = v.parse() {
                c.prompt.max_tokens_cap = n;
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_EIDOLON_PROMPT_PERSONALITY_WEIGHT") {
            if let Ok(n) = v.parse() {
                c.prompt.personality_weight = n;
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_EIDOLON_PROMPT_INCLUDE_MEMORIES") {
            c.prompt.default_include_memories = matches!(v.as_str(), "1" | "true" | "TRUE" | "yes");
        }
        if let Ok(v) = std::env::var("ENGRAM_EIDOLON_PROMPT_INCLUDE_PERSONALITY") {
            c.prompt.default_include_personality =
                matches!(v.as_str(), "1" | "true" | "TRUE" | "yes");
        }
        self
    }
}

/// Safety constraints injected into living prompts as mandatory rules.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SafetyConfig {
    #[serde(default)]
    pub rules: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub db_path: String,
    pub host: String,
    pub port: u16,
    #[serde(skip, default)]
    pub api_key: Option<SecretString>,
    pub embedding_dim: usize,
    pub default_retention: f32,
    pub embedding_model: String,
    pub embedding_max_seq: usize,
    pub embedding_model_dir: Option<String>,
    pub embedding_onnx_file: String,
    /// When true, refuse to download model weights from HuggingFace at
    /// boot. Files must already exist in `embedding_model_dir`. Use this
    /// for air-gapped deployments or to stop a restart storm from
    /// quietly pulling the wrong model.
    pub embedding_offline_only: bool,
    pub embedding_chunk_max_chars: usize,
    pub embedding_chunk_overlap: usize,
    pub embedding_chunk_max_chunks: usize,
    pub reranker_enabled: bool,
    pub reranker_top_k: usize,
    pub reranker_model_dir: Option<String>,
    pub data_dir: String,
    pub lance_index_path: Option<String>,
    pub vector_dimensions: usize,
    pub use_lance_index: bool,
    #[serde(skip, default)]
    pub gui_password: Option<SecretString>,
    pub gui_build_dir: Option<String>,
    pub pagerank_refresh_interval_secs: u64,
    pub pagerank_dirty_threshold: u32,
    pub pagerank_max_concurrent: usize,
    pub pagerank_enabled: bool,
    /// Whether to run the auto-backup background task.
    pub backup_enabled: bool,
    /// Seconds between scheduled backups. Default: 6 hours.
    pub backup_interval_secs: u64,
    /// Directory for backup files. Relative paths resolve under `data_dir`.
    /// Default: `backups`.
    pub backup_dir: String,
    /// Maximum number of hourly backup files to retain. Older backups are
    /// pruned after each successful run. Default: 14 (kept for back-compat;
    /// the disaster-recovery plan calls for 8 hourly + 30 daily).
    pub backup_retention: usize,
    /// Maximum number of daily backup files to retain in `<backup_dir>/daily`.
    /// After each successful run the verified hourly backup is promoted to
    /// the daily directory if no backup for the current UTC date exists.
    /// Default: 30.
    #[serde(default = "default_backup_retention_daily")]
    pub backup_retention_daily: usize,
    /// Grace period (in hours) for an old key after `POST /keys/rotate`.
    /// During this window the old key continues to authenticate so clients
    /// can cut over without downtime. Default: 24.
    pub auth_key_rotation_grace_hours: i64,
    #[serde(default)]
    pub encryption: EncryptionConfig,
    #[serde(default)]
    pub eidolon: EidolonConfig,
    /// SECURITY: IP addresses of trusted reverse proxies. When the request
    /// originates from one of these IPs, X-Forwarded-For is honoured for
    /// rate-limit keying. When empty (default), XFF is never trusted and
    /// the TCP peer address is always used.
    #[serde(default)]
    pub trusted_proxies: Vec<String>,
    /// Optional server reference table shown in living prompts.
    #[serde(default)]
    pub servers: Vec<ServerEntry>,
    /// Safety rules injected into living prompts as mandatory constraints.
    #[serde(default)]
    pub safety: SafetyConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            db_path: "engram.db".to_string(),
            host: "127.0.0.1".to_string(),
            port: 4200,
            api_key: None,
            embedding_dim: 1024,
            default_retention: 0.9,
            embedding_model: "BAAI/bge-m3".to_string(),
            embedding_max_seq: 512,
            embedding_model_dir: None,
            embedding_onnx_file: "model_quantized.onnx".to_string(),
            embedding_offline_only: false,
            embedding_chunk_max_chars: 1440,
            embedding_chunk_overlap: 160,
            embedding_chunk_max_chunks: 6,
            reranker_enabled: true,
            reranker_top_k: 12,
            reranker_model_dir: None,
            data_dir: "./data".to_string(),
            lance_index_path: None,
            vector_dimensions: 1024,
            use_lance_index: true,
            gui_password: None,
            gui_build_dir: None,
            pagerank_refresh_interval_secs: 300,
            pagerank_dirty_threshold: 100,
            pagerank_max_concurrent: 2,
            pagerank_enabled: true,
            backup_enabled: false,
            backup_interval_secs: 6 * 3600,
            backup_dir: "backups".to_string(),
            backup_retention: 14,
            backup_retention_daily: default_backup_retention_daily(),
            auth_key_rotation_grace_hours: 24,
            encryption: EncryptionConfig::default(),
            eidolon: EidolonConfig::default(),
            trusted_proxies: Vec::new(),
            servers: Vec::new(),
            safety: SafetyConfig::default(),
        }
    }
}

impl Config {
    /// Load a `Config` from a TOML file. Missing fields fall back to
    /// their `Default` values via `#[serde(default)]` on most fields.
    ///
    /// Secret fields (`api_key`, `gui_password`, `eidolon.api_key`) are
    /// `#[serde(skip)]` and must be supplied via environment variables.
    pub fn from_file<P: AsRef<std::path::Path>>(path: P) -> Result<Self, String> {
        let path = path.as_ref();
        let text =
            std::fs::read_to_string(path).map_err(|e| format!("read {}: {}", path.display(), e))?;
        toml::from_str(&text).map_err(|e| format!("parse {}: {}", path.display(), e))
    }

    /// Resolve the TOML config path using (in order):
    /// 1. `ENGRAM_CONFIG_FILE` env var
    /// 2. `./engram.toml` in the current directory
    /// 3. `$XDG_CONFIG_HOME/engram/config.toml` (or `~/.config/engram/config.toml`)
    ///
    /// Returns `None` if no config file is found.
    fn resolve_config_path() -> Option<std::path::PathBuf> {
        if let Ok(p) = std::env::var("ENGRAM_CONFIG_FILE") {
            let path = std::path::PathBuf::from(p);
            if path.exists() {
                return Some(path);
            } else {
                tracing::warn!(
                    "ENGRAM_CONFIG_FILE set but file not found: {}",
                    path.display()
                );
            }
        }
        let cwd_path = std::path::PathBuf::from("engram.toml");
        if cwd_path.exists() {
            return Some(cwd_path);
        }
        if let Some(cfg_dir) = dirs::config_dir() {
            let path = cfg_dir.join("engram").join("config.toml");
            if path.exists() {
                return Some(path);
            }
        }
        None
    }

    /// Load config layered: defaults -> TOML file (if present) -> env var overrides.
    ///
    /// This is the preferred entry point for server startup. Env vars always
    /// win so operators can override file values without editing the file.
    pub fn load() -> Self {
        let base = match Self::resolve_config_path() {
            Some(path) => match Self::from_file(&path) {
                Ok(cfg) => {
                    tracing::info!("loaded config from {}", path.display());
                    cfg
                }
                Err(e) => {
                    tracing::warn!("failed to load config file: {}. Using defaults.", e);
                    Self::default()
                }
            },
            None => Self::default(),
        };
        Self::apply_env(base)
    }

    pub fn from_env() -> Self {
        Self::apply_env(Self::default())
    }

    fn apply_env(mut config: Self) -> Self {
        if let Ok(v) = std::env::var("ENGRAM_DB_PATH") {
            config.db_path = v;
        }
        if let Ok(v) = std::env::var("ENGRAM_HOST") {
            config.host = v;
        }
        if let Ok(v) = std::env::var("ENGRAM_PORT") {
            match v.parse() {
                Ok(p) => config.port = p,
                Err(_) => tracing::warn!(
                    "invalid env ENGRAM_PORT={}, using default {}",
                    v,
                    config.port
                ),
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_API_KEY") {
            config.api_key = Some(SecretString::new(v));
        }
        if let Ok(v) = std::env::var("ENGRAM_EMBEDDING_DIM") {
            match v.parse() {
                Ok(d) => config.embedding_dim = d,
                Err(_) => tracing::warn!(
                    "invalid env ENGRAM_EMBEDDING_DIM={}, using default {}",
                    v,
                    config.embedding_dim
                ),
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_DEFAULT_RETENTION") {
            match v.parse() {
                Ok(r) => config.default_retention = r,
                Err(_) => tracing::warn!(
                    "invalid env ENGRAM_DEFAULT_RETENTION={}, using default {}",
                    v,
                    config.default_retention
                ),
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_EMBEDDING_MODEL") {
            config.embedding_model = v;
        }
        if let Ok(v) = std::env::var("ENGRAM_EMBEDDING_MAX_SEQ") {
            match v.parse() {
                Ok(n) => config.embedding_max_seq = n,
                Err(_) => tracing::warn!(
                    "invalid env ENGRAM_EMBEDDING_MAX_SEQ={}, using default {}",
                    v,
                    config.embedding_max_seq
                ),
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_EMBEDDING_MODEL_DIR") {
            config.embedding_model_dir = Some(v);
        }
        if let Ok(v) = std::env::var("ENGRAM_ONNX_MODEL_FILE") {
            config.embedding_onnx_file = v;
        }
        if let Ok(v) = std::env::var("ENGRAM_EMBEDDING_OFFLINE_ONLY") {
            config.embedding_offline_only = matches!(v.as_str(), "1" | "true" | "TRUE" | "yes");
        }
        if let Ok(v) = std::env::var("ENGRAM_EMBEDDING_CHUNK_MAX_CHARS") {
            match v.parse() {
                Ok(n) => config.embedding_chunk_max_chars = n,
                Err(_) => tracing::warn!(
                    "invalid env ENGRAM_EMBEDDING_CHUNK_MAX_CHARS={}, using default {}",
                    v,
                    config.embedding_chunk_max_chars
                ),
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_EMBEDDING_CHUNK_OVERLAP") {
            match v.parse() {
                Ok(n) => config.embedding_chunk_overlap = n,
                Err(_) => tracing::warn!(
                    "invalid env ENGRAM_EMBEDDING_CHUNK_OVERLAP={}, using default {}",
                    v,
                    config.embedding_chunk_overlap
                ),
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_EMBEDDING_CHUNK_MAX_CHUNKS") {
            match v.parse() {
                Ok(n) => config.embedding_chunk_max_chunks = n,
                Err(_) => tracing::warn!(
                    "invalid env ENGRAM_EMBEDDING_CHUNK_MAX_CHUNKS={}, using default {}",
                    v,
                    config.embedding_chunk_max_chunks
                ),
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_RERANKER_ENABLED") {
            config.reranker_enabled = matches!(v.as_str(), "1" | "true" | "TRUE" | "yes");
        } else if let Ok(v) = std::env::var("ENGRAM_CROSS_ENCODER") {
            config.reranker_enabled = v != "0";
        }
        if let Ok(v) = std::env::var("ENGRAM_RERANKER_MODEL_DIR") {
            config.reranker_model_dir = Some(v);
        }
        if let Ok(v) = std::env::var("ENGRAM_RERANKER_TOP_K") {
            match v.parse() {
                Ok(n) => config.reranker_top_k = n,
                Err(_) => tracing::warn!(
                    "invalid env ENGRAM_RERANKER_TOP_K={}, using default {}",
                    v,
                    config.reranker_top_k
                ),
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_DATA_DIR") {
            config.data_dir = v;
        }
        if let Ok(v) = std::env::var("ENGRAM_LANCE_INDEX_PATH") {
            config.lance_index_path = Some(v);
        }
        if let Ok(v) = std::env::var("ENGRAM_VECTOR_DIMENSIONS") {
            match v.parse() {
                Ok(n) => config.vector_dimensions = n,
                Err(_) => tracing::warn!(
                    "invalid env ENGRAM_VECTOR_DIMENSIONS={}, using default {}",
                    v,
                    config.vector_dimensions
                ),
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_USE_LANCE_INDEX") {
            config.use_lance_index = v != "0" && !v.eq_ignore_ascii_case("false");
        }
        if let Ok(v) = std::env::var("ENGRAM_GUI_PASSWORD") {
            config.gui_password = Some(SecretString::new(v));
        }
        if let Ok(v) = std::env::var("ENGRAM_GUI_BUILD_DIR") {
            config.gui_build_dir = Some(v);
        }
        if let Ok(v) = std::env::var("ENGRAM_PAGERANK_REFRESH_INTERVAL") {
            match v.parse() {
                Ok(n) => config.pagerank_refresh_interval_secs = n,
                Err(_) => tracing::warn!(
                    "invalid env ENGRAM_PAGERANK_REFRESH_INTERVAL={}, using default {}",
                    v,
                    config.pagerank_refresh_interval_secs
                ),
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_PAGERANK_DIRTY_THRESHOLD") {
            match v.parse() {
                Ok(n) => config.pagerank_dirty_threshold = n,
                Err(_) => tracing::warn!(
                    "invalid env ENGRAM_PAGERANK_DIRTY_THRESHOLD={}, using default {}",
                    v,
                    config.pagerank_dirty_threshold
                ),
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_PAGERANK_MAX_CONCURRENT") {
            match v.parse() {
                Ok(n) => config.pagerank_max_concurrent = n,
                Err(_) => tracing::warn!(
                    "invalid env ENGRAM_PAGERANK_MAX_CONCURRENT={}, using default {}",
                    v,
                    config.pagerank_max_concurrent
                ),
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_PAGERANK_ENABLED") {
            config.pagerank_enabled = v != "0" && !v.eq_ignore_ascii_case("false");
        }
        if let Ok(v) = std::env::var("ENGRAM_BACKUP_ENABLED") {
            config.backup_enabled = matches!(v.as_str(), "1" | "true" | "TRUE" | "yes");
        }
        if let Ok(v) = std::env::var("ENGRAM_BACKUP_INTERVAL_SECS") {
            match v.parse() {
                Ok(n) => config.backup_interval_secs = n,
                Err(_) => tracing::warn!(
                    "invalid env ENGRAM_BACKUP_INTERVAL_SECS={}, using default {}",
                    v,
                    config.backup_interval_secs
                ),
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_BACKUP_DIR") {
            config.backup_dir = v;
        }
        if let Ok(v) = std::env::var("ENGRAM_BACKUP_RETENTION") {
            match v.parse() {
                Ok(n) => config.backup_retention = n,
                Err(_) => tracing::warn!(
                    "invalid env ENGRAM_BACKUP_RETENTION={}, using default {}",
                    v,
                    config.backup_retention
                ),
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_BACKUP_RETENTION_DAILY") {
            match v.parse() {
                Ok(n) => config.backup_retention_daily = n,
                Err(_) => tracing::warn!(
                    "invalid env ENGRAM_BACKUP_RETENTION_DAILY={}, using default {}",
                    v,
                    config.backup_retention_daily
                ),
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_AUTH_KEY_ROTATION_GRACE_HOURS") {
            match v.parse() {
                Ok(n) if n > 0 => config.auth_key_rotation_grace_hours = n,
                Ok(_) => tracing::warn!(
                    "ENGRAM_AUTH_KEY_ROTATION_GRACE_HOURS must be > 0, using default {}",
                    config.auth_key_rotation_grace_hours
                ),
                Err(_) => tracing::warn!(
                    "invalid env ENGRAM_AUTH_KEY_ROTATION_GRACE_HOURS={}, using default {}",
                    v,
                    config.auth_key_rotation_grace_hours
                ),
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_ENCRYPTION_MODE") {
            config.encryption.mode = match v.to_ascii_lowercase().as_str() {
                "none" => EncryptionMode::None,
                "keyfile" => EncryptionMode::Keyfile,
                "env" => EncryptionMode::Env,
                "yubikey" => EncryptionMode::Yubikey,
                other => {
                    tracing::warn!("unknown ENGRAM_ENCRYPTION_MODE={}, using none", other);
                    EncryptionMode::None
                }
            };
        }
        // SECURITY: comma-separated list of trusted reverse proxy IPs.
        // Only when the TCP peer matches one of these will X-Forwarded-For
        // be honoured for rate-limit keying.
        if let Ok(v) = std::env::var("ENGRAM_TRUSTED_PROXIES") {
            config.trusted_proxies = v
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !config.trusted_proxies.is_empty() {
                tracing::info!("trusted proxies configured: {:?}", config.trusted_proxies);
            }
        }
        config.eidolon = config.eidolon.apply_env();
        config
    }

    /// Returns the resolved model directory for a given model name.
    ///
    /// For the reranker, checks `reranker_model_dir` first.
    /// For embeddings, checks `embedding_model_dir` first.
    /// Falls back to `<data_dir>/engram/models/<model_short_name>`.
    pub fn model_dir(&self, model_short_name: &str) -> std::path::PathBuf {
        // Reranker gets its own config path
        if model_short_name.contains("reranker") || model_short_name.contains("granite") {
            if let Some(ref dir) = self.reranker_model_dir {
                return std::path::PathBuf::from(dir);
            }
        }

        if let Some(ref dir) = self.embedding_model_dir {
            // If the embedding_model_dir points to a specific model (has model
            // name in it), use its parent as the base and append the short name.
            // This lets /opt/engram/data/models/bge-m3 resolve
            // /opt/engram/data/models/granite-reranker for other models.
            let path = std::path::PathBuf::from(dir);
            if path.file_name().is_some() && model_short_name != "bge-m3" {
                if let Some(parent) = path.parent() {
                    return parent.join(model_short_name);
                }
            }
            return path;
        }

        dirs::data_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("engram")
            .join("models")
            .join(model_short_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eidolon_config_defaults_are_populated() {
        let c = EidolonConfig::default();
        assert!(!c.enabled);
        assert_eq!(c.credd.url, "http://127.0.0.1:4400");
        assert_eq!(c.credd.agent_key_env, "CREDD_AGENT_KEY");
        assert!(!c.credd.allow_raw);
        assert!(c
            .gate
            .blocked_patterns
            .iter()
            .any(|p| p.contains("rm -rf /")));
        assert_eq!(c.gate.approval_timeout_secs, 300);
        assert_eq!(c.growth.reflection_interval_secs, 3600);
        assert_eq!(c.growth.observation_limit, 100);
        assert_eq!(c.sessions.max_concurrent, 64);
        assert_eq!(c.sessions.buffer_size, 4096);
        assert!(c.sessions.scrub_secrets);
        assert_eq!(c.prompt.default_max_tokens, 4000);
        assert_eq!(c.prompt.max_tokens_cap, 128000);
        assert!(c.prompt.default_include_memories);
    }

    #[test]
    fn config_exposes_eidolon_field() {
        let c = Config::default();
        assert_eq!(c.eidolon.prompt.default_max_tokens, 4000);
    }

    #[test]
    fn from_file_parses_partial_toml_and_uses_defaults() {
        let dir = std::env::temp_dir().join(format!("engram-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("engram.toml");
        std::fs::write(
            &path,
            r#"
host = "0.0.0.0"
port = 8080
pagerank_enabled = false

[eidolon]
enabled = true

[eidolon.prompt]
default_max_tokens = 8000
"#,
        )
        .unwrap();

        let c = Config::from_file(&path).expect("parse toml");
        assert_eq!(c.host, "0.0.0.0");
        assert_eq!(c.port, 8080);
        assert!(!c.pagerank_enabled);
        // unspecified fields fall back to defaults
        assert_eq!(c.db_path, "engram.db");
        assert_eq!(c.embedding_dim, 1024);
        assert!(c.eidolon.enabled);
        assert_eq!(c.eidolon.prompt.default_max_tokens, 8000);
        // nested default still applied for unspecified sub-field
        assert_eq!(c.eidolon.prompt.max_tokens_cap, 128000);

        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();
    }

    #[test]
    fn from_file_rejects_malformed_toml() {
        let dir = std::env::temp_dir().join(format!("engram-cfg-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("engram.toml");
        std::fs::write(&path, "port = \"not-a-number\"\n").unwrap();
        let err = Config::from_file(&path).unwrap_err();
        assert!(err.contains("parse"));
        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();
    }
}
