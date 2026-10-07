//! Deterministic hashing used for shard routing.
//!
//! Neurone must never route the same market identity to different shards
//! between runs or across processes, so we use a fixed, well-specified
//! hash (FNV-1a, 64-bit) rather than any per-process randomized hasher.
//! `std`'s `RandomState` is explicitly unsuitable here.

/// FNV-1a 64-bit hash.
#[inline]
pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = OFFSET;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(PRIME);
    }
    h
}

/// Map a market identity to a shard index in `0..num_shards`.
///
/// Deterministic, cheap, and stable across restarts. `num_shards` is
/// expected to be non-zero; a zero value is coerced to 1 to avoid a panic
/// in the hot path.
#[inline]
pub fn shard_for(key: &[u8; 32], num_shards: usize) -> usize {
    let n = num_shards.max(1);
    (fnv1a_64(key) % n as u64) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_is_stable() {
        assert_eq!(fnv1a_64(b""), 0xcbf2_9ce4_8422_2325);
        // Reference values computed independently (Python reference impl).
        assert_eq!(fnv1a_64(b"neurone"), 0x5a96_096f_50dd_eec9);
        assert_eq!(fnv1a_64(b"hello"), 0xa430_d846_80aa_bd0b);
    }

    #[test]
    fn routing_is_deterministic_and_in_range() {
        let key = [7u8; 32];
        let a = shard_for(&key, 8);
        let b = shard_for(&key, 8);
        assert_eq!(a, b);
        assert!(a < 8);
    }

    #[test]
    fn zero_shards_coerced() {
        assert_eq!(shard_for(&[1u8; 32], 0), 0);
    }
}
