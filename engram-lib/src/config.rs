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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateConfig {
    pub blocked_patterns: Vec<String>,
    pub reserved_targets: Vec<String>,
    pub approval_timeout_secs: u64,
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
            reserved_targets: vec!["ovh".to_string(), "hetzner-prod".to_string()],
            approval_timeout_secs: 300,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
        let mut c = Self {
            enabled: std::env::var("ENGRAM_EIDOLON_ENABLED")
                .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes"))
                .unwrap_or(false),
            url: std::env::var("ENGRAM_EIDOLON_URL").ok(),
            api_key: std::env::var("ENGRAM_EIDOLON_API_KEY")
                .ok()
                .map(SecretString::new),
            credd: CreddConfig::default(),
            gate: GateConfig::default(),
            growth: GrowthConfig::default(),
            sessions: SessionsConfig::default(),
            prompt: PromptConfig::default(),
        };
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
        c
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
            encryption: EncryptionConfig::default(),
            eidolon: EidolonConfig::default(),
            trusted_proxies: Vec::new(),
        }
    }
}

impl Config {
    pub fn from_env() -> Self {
        let mut config = Self::default();
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
        if let Ok(v) = std::env::var("ENGRAM_CROSS_ENCODER") {
            config.reranker_enabled = v != "0";
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
        config.eidolon = EidolonConfig::from_env();
        config
    }

    /// Returns the resolved model directory for a given model name.
    /// Priority: ENGRAM_EMBEDDING_MODEL_DIR env > data_dir/engram/models/<model_short_name>
    pub fn model_dir(&self, model_short_name: &str) -> std::path::PathBuf {
        if let Some(ref dir) = self.embedding_model_dir {
            std::path::PathBuf::from(dir)
        } else {
            dirs::data_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("engram")
                .join("models")
                .join(model_short_name)
        }
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
}
