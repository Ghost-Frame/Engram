use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(name = "engram-sidecar", about = "Agent scoring proxy for Engram")]
pub struct SidecarConfig {
    /// Port the sidecar HTTP server listens on
    #[arg(long, env = "ENGRAM_SIDECAR_PORT", default_value = "3001")]
    pub port: u16,

    /// Base URL of the upstream Engram server
    #[arg(
        long,
        env = "ENGRAM_SIDECAR_ENGRAM_URL",
        default_value = "http://127.0.0.1:3000"
    )]
    pub engram_url: String,

    /// Agent identifier used when tagging stored memories
    #[arg(long, env = "ENGRAM_SIDECAR_AGENT", default_value = "default")]
    pub agent: String,

    /// Optional operating mode passed to scoring logic
    #[arg(long, env = "ENGRAM_SIDECAR_MODE")]
    pub mode: Option<String>,
}
