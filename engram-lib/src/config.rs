use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub db_path: String,
    pub host: String,
    pub port: u16,
    pub api_key: Option<String>,
    pub embedding_dim: usize,
    pub default_retention: f32,
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
        config
    }
}
