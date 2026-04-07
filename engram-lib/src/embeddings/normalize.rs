/// L2-normalize a vector in place. After normalization, ||v|| = 1.
pub fn l2_normalize(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// Mean-pool over non-padding token positions in a 3D output tensor.
/// `hidden` is flattened [seq_len * dim], `mask` indicates which positions are real tokens.
/// Returns a Vec<f32> of length `dim`.
pub fn mean_pool(hidden: &[f32], mask: &[i64], seq_len: usize, dim: usize) -> Vec<f32> {
    let mut pooled = vec![0.0f32; dim];
    let mut count: usize = 0;
    for i in 0..seq_len {
        if mask[i] == 0 {
            continue;
        }
        count += 1;
        let offset = i * dim;
        for d in 0..dim {
            pooled[d] += hidden[offset + d];
        }
    }
    if count > 0 {
        let c = count as f32;
        for d in 0..dim {
            pooled[d] /= c;
        }
    }
    pooled
}

/// Weighted mean of multiple embeddings by chunk lengths. Result is L2-normalized.
pub fn weighted_mean_pool(embeddings: &[Vec<f32>], weights: &[f32]) -> Vec<f32> {
    if embeddings.is_empty() {
        return vec![];
    }
    let dim = embeddings[0].len();
    let total_weight: f32 = weights.iter().sum();
    let mut pooled = vec![0.0f32; dim];
    for (emb, &w) in embeddings.iter().zip(weights.iter()) {
        let normalized_w = w / total_weight;
        for (d, val) in emb.iter().enumerate() {
            pooled[d] += val * normalized_w;
        }
    }
    l2_normalize(&mut pooled);
    pooled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l2_normalize_unit_vector() {
        let mut v = vec![3.0, 4.0];
        l2_normalize(&mut v);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "norm was {}", norm);
        assert!((v[0] - 0.6).abs() < 1e-5);
        assert!((v[1] - 0.8).abs() < 1e-5);
    }

    #[test]
    fn l2_normalize_zero_vector() {
        let mut v = vec![0.0, 0.0, 0.0];
        l2_normalize(&mut v);
        assert!(v.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn mean_pool_basic() {
        // 3 tokens, 2 dims. hidden = [[1,2], [3,4], [0,0]]. mask = [1,1,0].
        let hidden = vec![1.0, 2.0, 3.0, 4.0, 0.0, 0.0];
        let mask = vec![1i64, 1, 0];
        let result = mean_pool(&hidden, &mask, 3, 2);
        assert_eq!(result.len(), 2);
        assert!((result[0] - 2.0).abs() < 1e-5);
        assert!((result[1] - 3.0).abs() < 1e-5);
    }

    #[test]
    fn weighted_mean_pool_basic() {
        let emb1 = vec![1.0, 0.0];
        let emb2 = vec![0.0, 1.0];
        let result = weighted_mean_pool(&[emb1, emb2], &[1.0, 1.0]);
        let expected = 1.0 / (2.0f32).sqrt();
        assert!((result[0] - expected).abs() < 1e-4);
        assert!((result[1] - expected).abs() < 1e-4);
    }

    #[test]
    fn weighted_mean_pool_unequal_weights() {
        let emb1 = vec![1.0, 0.0];
        let emb2 = vec![0.0, 1.0];
        let result = weighted_mean_pool(&[emb1, emb2], &[3.0, 1.0]);
        assert!(result[0] > result[1], "emb1 should dominate");
    }
}
