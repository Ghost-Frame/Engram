use serde::{Deserialize, Serialize};

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
    pub embedding_chunk_max_chars: usize,
    pub embedding_chunk_overlap: usize,
    pub embedding_chunk_max_chunks: usize,
    pub reranker_enabled: bool,
    pub reranker_top_k: usize,
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
            embedding_chunk_max_chars: 1440,
            embedding_chunk_overlap: 160,
            embedding_chunk_max_chunks: 6,
            reranker_enabled: true,
            reranker_top_k: 12,
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
            if let Ok(n) = v.parse() { config.embedding_max_seq = n; }
        }
        if let Ok(v) = std::env::var("ENGRAM_EMBEDDING_MODEL_DIR") {
            config.embedding_model_dir = Some(v);
        }
        if let Ok(v) = std::env::var("ENGRAM_ONNX_MODEL_FILE") {
            config.embedding_onnx_file = v;
        }
        if let Ok(v) = std::env::var("ENGRAM_EMBEDDING_CHUNK_MAX_CHARS") {
            if let Ok(n) = v.parse() { config.embedding_chunk_max_chars = n; }
        }
        if let Ok(v) = std::env::var("ENGRAM_EMBEDDING_CHUNK_OVERLAP") {
            if let Ok(n) = v.parse() { config.embedding_chunk_overlap = n; }
        }
        if let Ok(v) = std::env::var("ENGRAM_EMBEDDING_CHUNK_MAX_CHUNKS") {
            if let Ok(n) = v.parse() { config.embedding_chunk_max_chunks = n; }
        }
        if let Ok(v) = std::env::var("ENGRAM_CROSS_ENCODER") {
            config.reranker_enabled = v != "0";
        }
        if let Ok(v) = std::env::var("ENGRAM_RERANKER_TOP_K") {
            if let Ok(n) = v.parse() { config.reranker_top_k = n; }
        }
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
