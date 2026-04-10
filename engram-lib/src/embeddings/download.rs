use crate::{EngError, Result};
use std::path::{Path, PathBuf};
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

/// Ensure a file exists at `path`. If missing, download from `url`.
/// Uses atomic write: downloads to .tmp, then renames.
async fn ensure_file(path: &Path, url: &str) -> Result<()> {
    if path.exists() {
        return Ok(());
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

    let response = reqwest::get(url)
        .await
        .map_err(|e| EngError::Internal(format!("failed to download {}: {}", url, e)))?;

    if !response.status().is_success() {
        return Err(EngError::Internal(format!(
            "download failed with status {}: {}",
            response.status(),
            url
        )));
    }

    let bytes = response.bytes().await.map_err(|e| {
        EngError::Internal(format!("failed to read response body from {}: {}", url, e))
    })?;

    let tmp_path = path.with_extension("tmp");
    tokio::fs::write(&tmp_path, &bytes).await.map_err(|e| {
        EngError::Internal(format!("failed to write {}: {}", tmp_path.display(), e))
    })?;

    tokio::fs::rename(&tmp_path, path).await.map_err(|e| {
        EngError::Internal(format!(
            "failed to rename {} -> {}: {}",
            tmp_path.display(),
            path.display(),
            e
        ))
    })?;

    let size_mb = bytes.len() as f64 / (1024.0 * 1024.0);
    info!(size_mb = format!("{:.1}", size_mb), dest = %path.display(), "model file downloaded");

    Ok(())
}

/// Ensure all bge-m3 embedding model files are present in `model_dir`.
/// Downloads from HuggingFace if missing.
pub async fn ensure_embedding_model(
    model_dir: &Path,
    onnx_file: &str,
) -> Result<(PathBuf, PathBuf)> {
    let tokenizer_path = model_dir.join("tokenizer.json");
    let model_path = model_dir.join(onnx_file);

    ensure_file(&tokenizer_path, BGE_M3_TOKENIZER_URL).await?;

    let model_url = if onnx_file == "model.onnx" {
        BGE_M3_MODEL_FP32_URL
    } else {
        BGE_M3_MODEL_QUANTIZED_URL
    };
    ensure_file(&model_path, model_url).await?;

    Ok((tokenizer_path, model_path))
}

/// Ensure all granite reranker model files are present in `model_dir`.
pub async fn ensure_reranker_model(model_dir: &Path) -> Result<(PathBuf, PathBuf)> {
    let tokenizer_path = model_dir.join("tokenizer.json");
    let model_path = model_dir.join("model_quantized.onnx");

    ensure_file(&tokenizer_path, GRANITE_TOKENIZER_URL).await?;
    ensure_file(&model_path, GRANITE_MODEL_URL).await?;

    Ok((tokenizer_path, model_path))
}
