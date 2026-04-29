//! Probe: load OnnxProvider standalone, embed one string, exit.
//! Used to verify ONNX runtime loads correctly outside kleos-server.

use kleos_lib::config::Config;
use kleos_lib::embeddings::EmbeddingProvider;
use kleos_lib::embeddings::onnx::OnnxProvider;

#[tokio::main]
async fn main() -> kleos_lib::Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();
    let config = Config::from_env();
    println!("loading ONNX from model_dir={:?}", config.model_dir("bge-m3"));
    let t0 = std::time::Instant::now();
    let p = OnnxProvider::new(&config).await?;
    println!("loaded in {:?}", t0.elapsed());
    let v = p.embed("hello world").await?;
    println!("embed got {} dims", v.len());
    Ok(())
}
