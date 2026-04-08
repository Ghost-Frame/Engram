use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(name = "kleos-sidecar", about = "Agent scoring proxy for Kleos")]
pub struct SidecarConfig {
    /// Port the sidecar HTTP server listens on
    #[arg(long, env = "KLEOS_SIDECAR_PORT", default_value = "3001")]
    pub port: u16,

    /// Base URL of the upstream Kleos server
    #[arg(
        long,
        env = "KLEOS_SIDECAR_KLEOS_URL",
        default_value = "http://127.0.0.1:3000"
    )]
    pub kleos_url: String,

    /// Agent identifier used when tagging stored memories
    #[arg(long, env = "KLEOS_SIDECAR_AGENT", default_value = "default")]
    pub agent: String,

    /// Optional operating mode passed to scoring logic
    #[arg(long, env = "KLEOS_SIDECAR_MODE")]
    pub mode: Option<String>,
}
