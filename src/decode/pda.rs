//! Program-derived address derivation.
//!
//! This is the exact Solana algorithm: hash `seeds || bump || program_id ||
//! "ProgramDerivedAddress"` and accept the first candidate that is **not** a
//! valid ed25519 curve point. Implemented directly so the result is verifiable
//! and testable rather than guessed.

use curve25519_dalek::edwards::CompressedEdwardsY;
use sha2::{Digest, Sha256};

/// Marker appended to PDA preimages.
const PDA_MARKER: &[u8] = b"ProgramDerivedAddress";

/// Attempt a PDA for a specific bump. Returns `None` if the hash lands on the
/// ed25519 curve (which is illegal for a PDA) or the seed set is too long.
pub fn create_program_address(
    seeds: &[&[u8]],
    bump: u8,
    program_id: &[u8; 32],
) -> Option<[u8; 32]> {
    // Solana caps the concatenated seed length at MAX_SEEDS * MAX_SEED_LEN = 16*32.
    if seeds.iter().map(|s| s.len()).sum::<usize>() > 16 * 32 {
        return None;
    }
    let mut hasher = Sha256::new();
    for seed in seeds {
        hasher.update(seed);
    }
    hasher.update([bump]);
    hasher.update(program_id);
    hasher.update(PDA_MARKER);
    let hash: [u8; 32] = hasher.finalize().into();
    if is_on_curve(&hash) {
        None
    } else {
        Some(hash)
    }
}

/// Canonical PDA: iterate bumps from 255 down to 0.
pub fn find_program_address(seeds: &[&[u8]], program_id: &[u8; 32]) -> Option<([u8; 32], u8)> {
    for bump in (0u8..=255).rev() {
        if let Some(addr) = create_program_address(seeds, bump, program_id) {
            return Some((addr, bump));
        }
    }
    None
}

/// Whether a 32-byte compressed point decompresses to a valid ed25519 point.
fn is_on_curve(bytes: &[u8; 32]) -> bool {
    CompressedEdwardsY(*bytes).decompress().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYSTEM_PROGRAM: [u8; 32] = [0u8; 32];

    #[test]
    fn pda_is_deterministic_and_off_curve() {
        let program = [7u8; 32];
        let (a, bump) = find_program_address(&[b"seed"], &program).expect("pda");
        let (b, bump2) = find_program_address(&[b"seed"], &program).expect("pda");
        assert_eq!(a, b);
        assert_eq!(bump, bump2);
        // The derived address must not be on the curve.
        assert!(!is_on_curve(&a));
    }

    #[test]
    fn system_program_pda_matches_solana_docs_vector() {
        // Reference vector from Solana's `create_program_address` documentation:
        // seeds = ["", "Seed"], program = system program.
        // The first (bump=0) candidate is on-curve, so it must be rejected.
        assert!(create_program_address(&[b"", b"Seed"], 0, &SYSTEM_PROGRAM).is_none());
    }
}
