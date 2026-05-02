use crate::{EngError, Result};
use futures::StreamExt;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tracing::info;

const BGE_M3_TOKENIZER_URL: &str =
    "https://huggingface.co/Xenova/bge-m3/resolve/main/tokenizer.json";
const BGE_M3_MODEL_QUANTIZED_URL: &str =
    "https://huggingface.co/Xenova/bge-m3/resolve/main/onnx/model_quantized.onnx";
const BGE_M3_MODEL_FP32_URL: &str =
    "https://huggingface.co/Xenova/bge-m3/resolve/main/onnx/model.onnx";

const GRANITE_TOKENIZER_URL: &str =
    "https://huggingface.co/keisuke-miyako/granite-embedding-reranker-english-r2-onnx-int8/resolve/main/tokenizer.json";
const GRANITE_MODEL_URL: &str =
    "https://huggingface.co/keisuke-miyako/granite-embedding-reranker-english-r2-onnx-int8/resolve/main/model_quantized.onnx";

/// Default cap on a single model download. Operators can raise this via the
/// `ENGRAM_EMBEDDING_MODEL_MAX_BYTES` env var; sane production values stay
/// well under 4 GiB because the largest BGE FP32 model is ~2.3 GiB.
const DEFAULT_MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// Process-lifetime download client: 30-minute overall timeout, redirect cap
/// from `safe_client_builder`. Failing fast on build error is correct here:
/// if TLS init breaks at startup, we should surface that instead of silently
/// downgrading to an unconfigured client (closes H-009 for this site).
static DOWNLOAD_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    crate::net::safe_client_builder()
        .timeout(Duration::from_secs(30 * 60))
        .build()
        .expect("safe_client_builder failed at embeddings download startup")
});

fn max_download_bytes() -> u64 {
    std::env::var("ENGRAM_EMBEDDING_MODEL_MAX_BYTES")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_MAX_BYTES)
}

/// Ensure a file exists at `path`. If missing, download from `url`.
/// Uses atomic write: downloads to .tmp, then renames.
///
/// When `offline_only` is true the function never contacts the network.
/// A missing file returns a hard error pointing operators at
/// `ENGRAM_EMBEDDING_MODEL_DIR`, which is the correct signal for
/// air-gapped or locked-down deployments.
async fn ensure_file(path: &Path, url: &str, offline_only: bool) -> Result<()> {
    if path.exists() {
        return Ok(());
    }

    if offline_only {
        return Err(EngError::Internal(format!(
            "model file {} is missing and ENGRAM_EMBEDDING_OFFLINE_ONLY is set; \
             pre-stage the file (source: {}) into ENGRAM_EMBEDDING_MODEL_DIR",
            path.display(),
            url
        )));
    }

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            EngError::Internal(format!(
                "failed to create model dir {}: {}",
                parent.display(),
                e
            ))
        })?;
    }

    info!(url = url, dest = %path.display(), "downloading model file");

    let max_bytes = max_download_bytes();
    let response = DOWNLOAD_CLIENT
        .get(url)
        .send()
        .await
        .map_err(|e| EngError::Internal(format!("failed to download {}: {}", url, e)))?;

    if !response.status().is_success() {
        return Err(EngError::Internal(format!(
            "download failed with status {}: {}",
            response.status(),
            url
        )));
    }

    // Reject up-front when the upstream advertises a body larger than the cap.
    if let Some(content_len) = response.content_length() {
        if content_len > max_bytes {
            return Err(EngError::Internal(format!(
                "download from {} reports {} bytes, exceeds cap {}",
                url, content_len, max_bytes
            )));
        }
    }

    let tmp_path = path.with_extension("tmp");
    let mut file = tokio::fs::File::create(&tmp_path).await.map_err(|e| {
        EngError::Internal(format!("failed to create {}: {}", tmp_path.display(), e))
    })?;

    let mut total: u64 = 0;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk
            .map_err(|e| EngError::Internal(format!("stream read failed from {}: {}", url, e)))?;
        total = total.saturating_add(bytes.len() as u64);
        if total > max_bytes {
            // Best-effort cleanup of the partial tmp file.
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(EngError::Internal(format!(
                "download from {} exceeded cap of {} bytes mid-stream",
                url, max_bytes
            )));
        }
        file.write_all(&bytes).await.map_err(|e| {
            EngError::Internal(format!("write to {} failed: {}", tmp_path.display(), e))
        })?;
    }

    file.flush().await.map_err(|e| {
        EngError::Internal(format!("flush of {} failed: {}", tmp_path.display(), e))
    })?;
    drop(file);

    tokio::fs::rename(&tmp_path, path).await.map_err(|e| {
        EngError::Internal(format!(
            "failed to rename {} -> {}: {}",
            tmp_path.display(),
            path.display(),
            e
        ))
    })?;

    let size_mb = total as f64 / (1024.0 * 1024.0);
    info!(size_mb = format!("{:.1}", size_mb), dest = %path.display(), "model file downloaded");

    Ok(())
}

/// Ensure all bge-m3 embedding model files are present in `model_dir`.
/// Downloads from HuggingFace if missing and `offline_only` is false.
#[tracing::instrument(skip(model_dir), fields(model_dir = %model_dir.display(), onnx_file = %onnx_file, offline_only))]
pub async fn ensure_embedding_model(
    model_dir: &Path,
    onnx_file: &str,
    offline_only: bool,
) -> Result<(PathBuf, PathBuf)> {
    let tokenizer_path = model_dir.join("tokenizer.json");
    let model_path = model_dir.join(onnx_file);

    ensure_file(&tokenizer_path, BGE_M3_TOKENIZER_URL, offline_only).await?;

    let model_url = if onnx_file == "model.onnx" {
        BGE_M3_MODEL_FP32_URL
    } else {
        BGE_M3_MODEL_QUANTIZED_URL
    };
    ensure_file(&model_path, model_url, offline_only).await?;

    Ok((tokenizer_path, model_path))
}

/// Ensure all granite reranker model files are present in `model_dir`.
#[tracing::instrument(skip(model_dir), fields(model_dir = %model_dir.display(), offline_only))]
pub async fn ensure_reranker_model(
    model_dir: &Path,
    offline_only: bool,
) -> Result<(PathBuf, PathBuf)> {
    let tokenizer_path = model_dir.join("tokenizer.json");
    let model_path = model_dir.join("model_quantized.onnx");

    ensure_file(&tokenizer_path, GRANITE_TOKENIZER_URL, offline_only).await?;
    ensure_file(&model_path, GRANITE_MODEL_URL, offline_only).await?;

    Ok((tokenizer_path, model_path))
}
