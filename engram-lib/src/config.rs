use serde::{Deserialize, Serialize};

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
            reserved_targets: vec![
                "ovh".to_string(),
                "hetzner-prod".to_string(),
            ],
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
}

impl Default for SessionsConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 64,
            buffer_size: 4096,
            stream_timeout_secs: 1800,
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EidolonConfig {
    pub enabled: bool,
    pub url: Option<String>,
    pub api_key: Option<String>,
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
            api_key: std::env::var("ENGRAM_EIDOLON_API_KEY").ok(),
            gate: GateConfig::default(),
            growth: GrowthConfig::default(),
            sessions: SessionsConfig::default(),
            prompt: PromptConfig::default(),
        };
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
            c.prompt.default_include_memories =
                matches!(v.as_str(), "1" | "true" | "TRUE" | "yes");
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
    pub api_key: Option<String>,
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
    pub gui_password: Option<String>,
    pub gui_build_dir: Option<String>,
    pub pagerank_refresh_interval_secs: u64,
    pub pagerank_dirty_threshold: u32,
    pub pagerank_max_concurrent: usize,
    pub pagerank_enabled: bool,
    #[serde(default)]
    pub eidolon: EidolonConfig,
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
            eidolon: EidolonConfig::default(),
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
            if let Ok(p) = v.parse() {
                config.port = p;
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_API_KEY") {
            config.api_key = Some(v);
        }
        if let Ok(v) = std::env::var("ENGRAM_EMBEDDING_DIM") {
            if let Ok(d) = v.parse() {
                config.embedding_dim = d;
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_DEFAULT_RETENTION") {
            if let Ok(r) = v.parse() {
                config.default_retention = r;
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_EMBEDDING_MODEL") {
            config.embedding_model = v;
        }
        if let Ok(v) = std::env::var("ENGRAM_EMBEDDING_MAX_SEQ") {
            if let Ok(n) = v.parse() {
                config.embedding_max_seq = n;
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
            if let Ok(n) = v.parse() {
                config.embedding_chunk_max_chars = n;
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_EMBEDDING_CHUNK_OVERLAP") {
            if let Ok(n) = v.parse() {
                config.embedding_chunk_overlap = n;
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_EMBEDDING_CHUNK_MAX_CHUNKS") {
            if let Ok(n) = v.parse() {
                config.embedding_chunk_max_chunks = n;
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_CROSS_ENCODER") {
            config.reranker_enabled = v != "0";
        }
        if let Ok(v) = std::env::var("ENGRAM_RERANKER_TOP_K") {
            if let Ok(n) = v.parse() {
                config.reranker_top_k = n;
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_DATA_DIR") {
            config.data_dir = v;
        }
        if let Ok(v) = std::env::var("ENGRAM_LANCE_INDEX_PATH") {
            config.lance_index_path = Some(v);
        }
        if let Ok(v) = std::env::var("ENGRAM_VECTOR_DIMENSIONS") {
            if let Ok(n) = v.parse() {
                config.vector_dimensions = n;
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_USE_LANCE_INDEX") {
            config.use_lance_index = v != "0" && !v.eq_ignore_ascii_case("false");
        }
        if let Ok(v) = std::env::var("ENGRAM_GUI_PASSWORD") {
            config.gui_password = Some(v);
        }
        if let Ok(v) = std::env::var("ENGRAM_GUI_BUILD_DIR") {
            config.gui_build_dir = Some(v);
        }
        if let Ok(v) = std::env::var("ENGRAM_PAGERANK_REFRESH_INTERVAL") {
            if let Ok(n) = v.parse() {
                config.pagerank_refresh_interval_secs = n;
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_PAGERANK_DIRTY_THRESHOLD") {
            if let Ok(n) = v.parse() {
                config.pagerank_dirty_threshold = n;
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_PAGERANK_MAX_CONCURRENT") {
            if let Ok(n) = v.parse() {
                config.pagerank_max_concurrent = n;
            }
        }
        if let Ok(v) = std::env::var("ENGRAM_PAGERANK_ENABLED") {
            config.pagerank_enabled = v != "0" && !v.eq_ignore_ascii_case("false");
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
        assert!(c.gate.blocked_patterns.iter().any(|p| p.contains("rm -rf /")));
        assert_eq!(c.gate.approval_timeout_secs, 300);
        assert_eq!(c.growth.reflection_interval_secs, 3600);
        assert_eq!(c.growth.observation_limit, 100);
        assert_eq!(c.sessions.max_concurrent, 64);
        assert_eq!(c.sessions.buffer_size, 4096);
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
