/// Compute a 64-bit simhash fingerprint for the given text.
/// Tokenizes on whitespace, hashes each token, and accumulates bit weights.
pub fn simhash(text: &str) -> u64 {
    let mut v: [i32; 64] = [0; 64];

    for token in text.split_whitespace() {
        let h = fnv1a_64(token.as_bytes());
        for bit in 0..64u32 {
            if (h >> bit) & 1 == 1 {
                v[bit as usize] += 1;
            } else {
                v[bit as usize] -= 1;
            }
        }
    }

    let mut fingerprint: u64 = 0;
    for bit in 0..64u32 {
        if v[bit as usize] > 0 {
            fingerprint |= 1u64 << bit;
        }
    }
    fingerprint
}

/// Count differing bits between two simhash values (Hamming distance).
pub fn hamming_distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// FNV-1a 64-bit hash, used as the per-token hash inside simhash.
fn fnv1a_64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 14695981039346656037;
    const PRIME: u64 = 1099511628211;
    let mut hash = OFFSET_BASIS;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_texts_have_zero_distance() {
        let h = simhash("hello world foo bar");
        assert_eq!(hamming_distance(h, h), 0);
    }

    #[test]
    fn different_texts_have_nonzero_distance() {
        let a = simhash("the quick brown fox");
        let b = simhash("completely different text here");
        assert!(hamming_distance(a, b) > 0);
    }

    #[test]
    fn similar_texts_have_low_distance() {
        let a = simhash("the quick brown fox jumps over the lazy dog");
        let b = simhash("the quick brown fox jumps over the lazy cat");
        assert!(hamming_distance(a, b) < 10);
    }
}
