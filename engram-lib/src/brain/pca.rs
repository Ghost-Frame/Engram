//! PCA dimensionality reduction for brain patterns.
//!
//! Compresses high-dimensional embeddings (RAW_DIM, typically 1024) to a lower
//! dimension (BRAIN_DIM, typically 512) for storage efficiency while preserving
//! cosine similarity relationships.
//!
//! Uses power iteration with deflation to compute principal components without
//! requiring LAPACK or other heavy linear algebra dependencies.

use crate::db::Database;
use crate::{EngError, Result};
use ndarray::{Array1, Array2, Axis};
use serde::{Deserialize, Serialize};

/// Raw embedding dimension (from embedding model, e.g., bge-m3).
pub const RAW_DIM: usize = 1024;

/// Compressed brain pattern dimension.
pub const BRAIN_DIM: usize = 512;

/// PCA transformation model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PcaTransform {
    /// Principal component matrix: n_components x n_features
    pub components: Vec<Vec<f32>>,
    /// Mean vector for centering
    pub mean: Vec<f32>,
    /// Number of components (target dimension)
    pub n_components: usize,
    /// Source dimension
    pub source_dim: usize,
}

impl PcaTransform {
    /// Create an empty (identity-like) transform.
    pub fn new_empty(source_dim: usize) -> Self {
        PcaTransform {
            components: Vec::new(),
            mean: vec![0.0; source_dim],
            n_components: 0,
            source_dim,
        }
    }

    /// Fit PCA on data matrix (n_samples x source_dim).
    /// Uses power iteration with deflation to find top target_dim eigenvectors.
    pub fn fit(data: &[Vec<f32>], target_dim: usize) -> Self {
        if data.is_empty() {
            return Self::new_empty(RAW_DIM);
        }

        let n_samples = data.len();
        let n_features = data[0].len();
        let n_components = target_dim.min(n_samples.saturating_sub(1)).max(1);

        // Convert to ndarray for computation
        let data_arr = Array2::from_shape_fn((n_samples, n_features), |(i, j)| data[i][j]);

        // Compute mean
        let mean = data_arr.mean_axis(Axis(0)).unwrap();

        // Center data
        let centered = &data_arr - &mean;

        // Compute covariance matrix: (X^T X) / (n-1)
        let n = (n_samples as f32 - 1.0).max(1.0);
        let cov = centered.t().dot(&centered) / n;

        // Power iteration with deflation
        let mut components: Vec<Array1<f32>> = Vec::with_capacity(n_components);
        let mut residual = cov.clone();

        for comp_idx in 0..n_components {
            // Deterministic pseudo-random initialization
            let mut v = Array1::<f32>::zeros(n_features);
            for i in 0..n_features {
                let seed = (i as f32 * 1.618_034 + comp_idx as f32 * std::f32::consts::E).sin();
                v[i] = seed;
            }
            // Normalize
            let norm = v.dot(&v).sqrt();
            if norm > 1e-10 {
                v /= norm;
            }

            // Power iteration: 100 max iterations, 1e-6 convergence
            for _ in 0..100 {
                let v_new = residual.dot(&v);
                let norm = v_new.dot(&v_new).sqrt();
                if norm < 1e-10 {
                    break;
                }
                let v_new_normalized = &v_new / norm;
                let diff = (&v_new_normalized - &v).mapv(|x| x.abs()).sum();
                v = v_new_normalized;
                if diff < 1e-6 {
                    break;
                }
            }

            // Gram-Schmidt orthogonalization against previous components
            for prev in &components {
                let proj = v.dot(prev);
                v = &v - &(prev * proj);
            }

            // Renormalize
            let norm = v.dot(&v).sqrt();
            if norm < 1e-10 {
                // Degenerate eigenvector, use unit vector
                v = Array1::zeros(n_features);
                let idx = comp_idx % n_features;
                v[idx] = 1.0;
            } else {
                v /= norm;
            }

            // Deflate: residual -= eigenvalue * v v^T (in-place)
            let eigenvalue = v.dot(&residual.dot(&v));
            for i in 0..n_features {
                for j in 0..n_features {
                    residual[[i, j]] -= eigenvalue * v[i] * v[j];
                }
            }

            components.push(v);
        }

        // Convert to Vec<Vec<f32>> for serialization
        let comp_vecs: Vec<Vec<f32>> = components.iter().map(|c| c.to_vec()).collect();
        let mean_vec = mean.to_vec();

        PcaTransform {
            components: comp_vecs,
            mean: mean_vec,
            n_components,
            source_dim: n_features,
        }
    }

    /// Project a single embedding to lower dimension, L2 normalized.
    pub fn project(&self, embedding: &[f32]) -> Vec<f32> {
        if self.n_components == 0 || embedding.len() != self.source_dim {
            return vec![0.0; BRAIN_DIM];
        }

        // Center
        let centered: Vec<f32> = embedding
            .iter()
            .zip(&self.mean)
            .map(|(e, m)| e - m)
            .collect();

        // Project: dot product with each component
        let mut projected: Vec<f32> = self
            .components
            .iter()
            .map(|comp| comp.iter().zip(&centered).map(|(c, x)| c * x).sum::<f32>())
            .collect();

        // L2 normalize
        l2_normalize_inplace(&mut projected);

        projected
    }

    /// Project a batch of embeddings.
    pub fn project_batch(&self, data: &[Vec<f32>]) -> Vec<Vec<f32>> {
        data.iter().map(|e| self.project(e)).collect()
    }

    /// Inverse transform (approximate reconstruction).
    pub fn inverse_transform(&self, projected: &[f32]) -> Vec<f32> {
        if self.n_components == 0 || projected.len() != self.n_components {
            return vec![0.0; self.source_dim];
        }

        // Reconstruct: sum of (projected[i] * component[i]) + mean
        let mut reconstructed = self.mean.clone();
        for (i, &proj_val) in projected.iter().enumerate() {
            for (j, &comp_val) in self.components[i].iter().enumerate() {
                reconstructed[j] += proj_val * comp_val;
            }
        }

        reconstructed
    }
}

/// L2 normalize a vector in place.
pub fn l2_normalize_inplace(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-10 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// L2 normalize a vector, returning a new vector.
pub fn l2_normalize(v: &[f32]) -> Vec<f32> {
    let mut result = v.to_vec();
    l2_normalize_inplace(&mut result);
    result
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

/// Stored PCA model metadata.
#[derive(Debug, Clone)]
pub struct PcaModelRow {
    pub id: i64,
    pub source_dim: i64,
    pub target_dim: i64,
    pub fit_at: String,
    pub model_blob: Vec<u8>,
}

/// Store a PCA model to the database.
pub async fn store_pca_model(
    db: &Database,
    source_dim: usize,
    target_dim: usize,
    transform: &PcaTransform,
) -> Result<i64> {
    let blob = serde_json::to_vec(transform).map_err(|e| EngError::Internal(e.to_string()))?;
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

    db.conn
        .execute(
            "INSERT INTO brain_pca_models (source_dim, target_dim, fit_at, model_blob)
             VALUES (?1, ?2, ?3, ?4)",
            libsql::params![source_dim as i64, target_dim as i64, now, blob],
        )
        .await?;

    let mut rows = db.conn.query("SELECT last_insert_rowid()", ()).await?;

    let id: i64 = match rows.next().await? {
        Some(row) => row.get(0)?,
        None => 0,
    };

    Ok(id)
}

/// Load the most recent PCA model for given dimensions.
pub async fn load_pca_model(
    db: &Database,
    source_dim: usize,
    target_dim: usize,
) -> Result<Option<PcaTransform>> {
    let mut rows = db
        .conn
        .query(
            "SELECT model_blob FROM brain_pca_models
             WHERE source_dim = ?1 AND target_dim = ?2
             ORDER BY fit_at DESC LIMIT 1",
            libsql::params![source_dim as i64, target_dim as i64],
        )
        .await?;

    match rows.next().await? {
        Some(row) => {
            let blob: Vec<u8> = row.get(0)?;
            let transform: PcaTransform =
                serde_json::from_slice(&blob).map_err(|e| EngError::Internal(e.to_string()))?;
            Ok(Some(transform))
        }
        None => Ok(None),
    }
}

/// Delete old PCA models, keeping only the most recent per (source_dim, target_dim).
pub async fn cleanup_old_models(db: &Database) -> Result<usize> {
    let affected = db
        .conn
        .execute(
            "DELETE FROM brain_pca_models
             WHERE id NOT IN (
                 SELECT MAX(id) FROM brain_pca_models
                 GROUP BY source_dim, target_dim
             )",
            (),
        )
        .await?;

    Ok(affected as usize)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_data(n: usize, d: usize) -> Vec<Vec<f32>> {
        (0..n)
            .map(|i| {
                (0..d)
                    .map(|j| ((i as f32 * 0.1 + j as f32 * 0.01) * std::f32::consts::PI).sin())
                    .collect()
            })
            .collect()
    }

    fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if na < 1e-10 || nb < 1e-10 {
            return 0.0;
        }
        dot / (na * nb)
    }

    #[test]
    fn pca_basic() {
        let data = make_test_data(64, RAW_DIM);
        let pca = PcaTransform::fit(&data, BRAIN_DIM);
        // n_components = min(512, 64-1) = 63
        assert!(pca.n_components > 0);
        assert!(pca.n_components <= BRAIN_DIM);
        assert_eq!(pca.components.len(), pca.n_components);
        assert_eq!(pca.mean.len(), RAW_DIM);
    }

    #[test]
    fn pca_preserves_similarity() {
        // Two similar vectors should have high cosine sim after projection
        let mut data = make_test_data(32, RAW_DIM);
        // Make rows 0 and 1 nearly identical
        let row0 = data[0].clone();
        let noisy: Vec<f32> = row0
            .iter()
            .enumerate()
            .map(|(i, &v)| v + (i as f32 * 0.001).sin() * 0.01)
            .collect();
        data[1] = noisy;

        let pca = PcaTransform::fit(&data, BRAIN_DIM);
        let p0 = pca.project(&data[0]);
        let p1 = pca.project(&data[1]);
        let sim = cosine_sim(&p0, &p1);
        assert!(sim > 0.95, "cosine similarity after PCA: {}", sim);
    }

    #[test]
    fn pca_project_normalized() {
        let data = make_test_data(32, RAW_DIM);
        let pca = PcaTransform::fit(&data, BRAIN_DIM);
        let p = pca.project(&data[0]);
        let norm: f32 = p.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "norm: {}", norm);
    }

    #[test]
    fn pca_roundtrip() {
        let data = make_test_data(64, RAW_DIM);
        let pca = PcaTransform::fit(&data, 32); // Use smaller target for faster test

        let original = &data[0];
        let projected = pca.project(original);
        let reconstructed = pca.inverse_transform(&projected);

        // Reconstruction won't be perfect due to dimensionality reduction,
        // but should preserve the general direction
        let sim = cosine_sim(original, &reconstructed);
        assert!(sim > 0.5, "reconstruction similarity: {}", sim);
    }

    #[test]
    fn pca_serialization() {
        let data = make_test_data(32, 64); // Smaller for faster test
        let pca = PcaTransform::fit(&data, 16);

        let json = serde_json::to_string(&pca).unwrap();
        let restored: PcaTransform = serde_json::from_str(&json).unwrap();

        assert_eq!(pca.n_components, restored.n_components);
        assert_eq!(pca.source_dim, restored.source_dim);
        assert_eq!(pca.mean.len(), restored.mean.len());
    }
}
