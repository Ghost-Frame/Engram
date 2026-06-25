//! Configuration types and generation for the Kleos installer wizard.

use std::path::{Path, PathBuf};

use crate::error::InstallError;

/// Core server configuration values collected during the wizard.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ServerConfig {
    /// Bind address for the HTTP server (e.g. "127.0.0.1").
    pub host: String,
    /// TCP port the server listens on.
    pub port: u16,
    /// Absolute path to the directory where Kleos stores runtime data.
    pub data_dir: PathBuf,
    /// Filename (relative to data_dir) of the SQLCipher database.
    pub db_path: String,
    /// Optional comma-separated list of allowed CORS origins.
    pub cors_origins: Option<String>,
}

impl Default for ServerConfig {
    /// Return a sensible default server configuration.
    ///
    /// Binds to localhost on port 4200, stores data in `./data`, and uses
    /// `kleos.db` as the database filename.
    fn default() -> Self {
        ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 4200,
            data_dir: PathBuf::from("./data"),
            db_path: "kleos.db".to_string(),
            cors_origins: None,
        }
    }
}

/// Configuration for the embedding provider used by the server.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum EmbeddingConfig {
    /// Use a locally downloaded ONNX model.
    LocalOnnx {
        /// Model identifier / name (e.g. "BAAI/bge-m3").
        model: String,
        /// Output embedding dimension expected from the model.
        dimension: u32,
        /// Whether the installer should automatically download the model files.
        auto_download: bool,
    },
    /// Use a remote HTTP embedding endpoint (OpenAI-compatible).
    Remote {
        /// Base URL of the embedding API endpoint.
        url: String,
        /// API key sent in the `Authorization: Bearer` header.
        api_key: String,
        /// Optional model name passed in the request body.
        model: Option<String>,
        /// Expected output dimension of the remote model.
        dimension: u32,
    },
}

/// Configuration for the reranker component used during retrieval.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum RerankerConfig {
    /// Use a locally downloaded ONNX reranker model.
    LocalOnnx,
    /// Use a remote HTTP reranker endpoint.
    Remote {
        /// Base URL of the reranker API endpoint.
        endpoint: String,
        /// API key for the remote reranker.
        api_key: String,
        /// Model name to request from the remote endpoint.
        model: String,
    },
    /// Disable reranking entirely.
    Disabled,
}

/// Security-related secrets and access-control settings.
#[derive(Debug, Clone)]
pub struct SecurityConfig {
    /// 64-char hex key used to encrypt the SQLCipher database.
    pub encryption_key: String,
    /// 64-char hex pepper mixed into API key hashes before storage.
    pub api_key_pepper: String,
    /// The initial API key generated for the first user, prefixed with "kleos_".
    pub initial_api_key: String,
    /// 64-char hex secret used to sign HMAC tokens.
    pub hmac_secret: String,
    /// When `true`, the server allows unauthenticated access (development mode).
    pub open_access: bool,
}

/// Complete installer configuration assembled from all wizard steps.
///
/// This is the canonical in-memory representation from which engram.toml and
/// .env are generated.
#[derive(Debug, Clone)]
pub struct InstallerConfig {
    /// Server bind / storage configuration, present when the server is being installed.
    pub server: Option<ServerConfig>,
    /// Embedding provider selection and parameters.
    pub embedding: Option<EmbeddingConfig>,
    /// Reranker selection and parameters.
    pub reranker: Option<RerankerConfig>,
    /// Security secrets -- always required.
    pub security: SecurityConfig,
}

/// Render a string as a fully-escaped TOML basic-string literal (including the
/// surrounding quotes). Prevents a config value containing a quote or newline
/// from breaking out of its field and injecting extra TOML -- e.g. a forged
/// `[security]` table that flips `open_access`.
fn toml_string(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

/// Reject a config value that would corrupt the line-oriented `.env` format: a
/// newline injects an additional `KEY=VALUE` line.
fn env_value(s: &str) -> Result<&str, InstallError> {
    if s.contains('\n') || s.contains('\r') {
        return Err(InstallError::Config(
            "config value contains a newline; refusing to write .env".to_string(),
        ));
    }
    Ok(s)
}

/// Write a secrets file with owner-only permissions (0600 on Unix). The default
/// umask leaves `std::fs::write` world-readable, which would expose the `.env`
/// holding ENGRAM_DB_KEY, the API-key pepper, and the HMAC secret.
fn write_private_file(path: &Path, contents: &str) -> Result<(), InstallError> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        // mode() only applies on creation; tighten an already-existing file too.
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        f.write_all(contents.as_bytes())?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, contents)?;
        Ok(())
    }
}

impl InstallerConfig {
    /// Generate a complete `engram.toml` configuration file as a string.
    ///
    /// Serializes all populated sections into TOML format. Returns
    /// `InstallError::Config` if serialization fails.
    pub fn generate_toml(&self) -> Result<String, InstallError> {
        let mut out = String::new();

        if let Some(srv) = &self.server {
            out.push_str("[server]\n");
            out.push_str(&format!("host = {}\n", toml_string(&srv.host)));
            out.push_str(&format!("port = {}\n", srv.port));
            out.push_str(&format!(
                "data_dir = {}\n",
                toml_string(&srv.data_dir.display().to_string())
            ));
            out.push_str(&format!("db_path = {}\n", toml_string(&srv.db_path)));
            if let Some(origins) = &srv.cors_origins {
                out.push_str(&format!("cors_origins = {}\n", toml_string(origins)));
            }
            out.push('\n');
        }

        match &self.embedding {
            Some(EmbeddingConfig::LocalOnnx {
                model,
                dimension,
                auto_download,
            }) => {
                out.push_str("[embedding]\n");
                out.push_str("provider = \"local_onnx\"\n");
                out.push_str(&format!("model = {}\n", toml_string(model)));
                out.push_str(&format!("dimension = {dimension}\n"));
                out.push_str(&format!("auto_download = {auto_download}\n"));
                out.push('\n');
            }
            Some(EmbeddingConfig::Remote {
                url,
                model,
                dimension,
                ..
            }) => {
                out.push_str("[embedding]\n");
                out.push_str("provider = \"remote\"\n");
                out.push_str(&format!("url = {}\n", toml_string(url)));
                if let Some(m) = model {
                    out.push_str(&format!("model = {}\n", toml_string(m)));
                }
                out.push_str(&format!("dimension = {dimension}\n"));
                out.push('\n');
            }
            None => {}
        }

        match &self.reranker {
            Some(RerankerConfig::LocalOnnx) => {
                out.push_str("[reranker]\n");
                out.push_str("provider = \"local_onnx\"\n");
                out.push('\n');
            }
            Some(RerankerConfig::Remote {
                endpoint, model, ..
            }) => {
                out.push_str("[reranker]\n");
                out.push_str("provider = \"remote\"\n");
                out.push_str(&format!("endpoint = {}\n", toml_string(endpoint)));
                out.push_str(&format!("model = {}\n", toml_string(model)));
                out.push('\n');
            }
            Some(RerankerConfig::Disabled) | None => {
                out.push_str("[reranker]\n");
                out.push_str("provider = \"disabled\"\n");
                out.push('\n');
            }
        }

        out.push_str("[security]\n");
        out.push_str(&format!("open_access = {}\n", self.security.open_access));
        out.push('\n');

        Ok(out)
    }

    /// Generate a `.env` file string containing all secret environment variables.
    ///
    /// Secrets (keys, pepper, HMAC) are kept in the environment file rather than
    /// engram.toml so that the config file can be committed to version control
    /// without leaking credentials. Returns `InstallError::Config` if
    /// generation fails.
    pub fn generate_env(&self) -> Result<String, InstallError> {
        let mut out = String::new();

        out.push_str("# Kleos environment secrets -- keep this file private\n");
        out.push_str(&format!("ENGRAM_DB_KEY={}\n", self.security.encryption_key));
        out.push_str(&format!(
            "ENGRAM_API_KEY_PEPPER={}\n",
            self.security.api_key_pepper
        ));
        out.push_str(&format!(
            "ENGRAM_HMAC_SECRET={}\n",
            self.security.hmac_secret
        ));

        if let Some(srv) = &self.server {
            out.push_str(&format!("ENGRAM_HOST={}\n", env_value(&srv.host)?));
            out.push_str(&format!("ENGRAM_PORT={}\n", srv.port));
        }

        if let Some(EmbeddingConfig::Remote { url, api_key, .. }) = &self.embedding {
            out.push_str(&format!("KLEOS_EMBEDDING_URL={}\n", env_value(url)?));
            out.push_str(&format!(
                "KLEOS_EMBEDDING_API_KEY={}\n",
                env_value(api_key)?
            ));
        }

        if let Some(RerankerConfig::Remote {
            api_key,
            endpoint,
            model,
            ..
        }) = &self.reranker
        {
            out.push_str("ENGRAM_RERANKER_BACKEND=http\n");
            out.push_str(&format!(
                "ENGRAM_RERANKER_HTTP_ENDPOINT={}\n",
                env_value(endpoint)?
            ));
            out.push_str(&format!(
                "ENGRAM_RERANKER_HTTP_API_KEY={}\n",
                env_value(api_key)?
            ));
            out.push_str(&format!(
                "ENGRAM_RERANKER_HTTP_MODEL={}\n",
                env_value(model)?
            ));
        }

        Ok(out)
    }

    /// Write `engram.toml` and `.env` into `config_dir`.
    ///
    /// Creates `config_dir` if it does not exist. Returns `InstallError::Config`
    /// if TOML / env generation fails, or `InstallError::Io` if writing fails.
    pub fn write_config(&self, config_dir: &Path) -> Result<(), InstallError> {
        std::fs::create_dir_all(config_dir)?;

        let toml_content = self.generate_toml()?;
        std::fs::write(config_dir.join("engram.toml"), toml_content)?;

        let env_content = self.generate_env()?;
        write_private_file(&config_dir.join(".env"), &env_content)?;

        Ok(())
    }
}

#[cfg(test)]
mod injection_tests {
    use super::{env_value, toml_string};

    // A value crafted to break out of its quoted field and inject a forged
    // table must round-trip back to exactly the original value -- whatever
    // escaping form toml chooses -- and must not introduce a new key.
    #[test]
    fn toml_string_prevents_injection() {
        let evil = "x\"\nopen_access = true";
        let rendered = toml_string(evil);
        let parsed: toml::Value = format!("v = {rendered}").parse().unwrap();
        assert_eq!(parsed["v"].as_str().unwrap(), evil);
        assert!(parsed.get("open_access").is_none());
    }

    // A newline in an env value would inject an extra KEY=VALUE line.
    #[test]
    fn env_value_rejects_newlines() {
        assert!(env_value("ok-value").is_ok());
        assert!(env_value("inject\nKEY=val").is_err());
        assert!(env_value("inject\rKEY=val").is_err());
    }
}
