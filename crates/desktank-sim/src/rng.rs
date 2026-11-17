//! Deterministic randomness: one PCG stream per chunk per tick (SPEC §2.1).
//!
//! A stream depends only on `(seed, tick, chunk id)`, never on update order.
//! That is one prerequisite for a deterministic parallel chunk pass; the
//! cross-chunk row sweep and displaced-liquid chains (SPEC §2.1) are others.

use rand_pcg::Pcg32;

/// `SplitMix64` finaliser: a cheap, well-mixed 64-bit hash step.
pub(crate) fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The random stream for `chunk` during `tick`.
pub(crate) fn chunk_stream(seed: u64, tick: u64, chunk: u64) -> Pcg32 {
    let state = splitmix64(seed ^ splitmix64(tick ^ splitmix64(chunk)));
    Pcg32::new(state, chunk)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand_core::Rng;

    #[test]
    fn streams_depend_only_on_inputs() {
        let a: Vec<u32> = {
            let mut r = chunk_stream(7, 3, 5);
            (0..4).map(|_| r.next_u32()).collect()
        };
        let b: Vec<u32> = {
            let mut r = chunk_stream(7, 3, 5);
            (0..4).map(|_| r.next_u32()).collect()
        };
        assert_eq!(a, b);
        let mut other = chunk_stream(7, 4, 5);
        assert_ne!(a[0], other.next_u32());
    }
}
