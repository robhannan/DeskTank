//! Stable state hash: 64-bit FNV-1a, independent of platform and process
//! (std's `DefaultHasher` is randomly seeded and may change between Rust
//! releases).

const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

/// Incremental FNV-1a 64 hasher.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Fnv64(u64);

impl Fnv64 {
    pub(crate) fn new() -> Self {
        Self(OFFSET)
    }

    pub(crate) fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(PRIME);
        }
    }

    pub(crate) fn finish(self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_reference_vectors() {
        let h = |s: &str| {
            let mut f = Fnv64::new();
            f.write(s.as_bytes());
            f.finish()
        };
        assert_eq!(h(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(h("a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(h("foobar"), 0x8594_4171_f739_67e8);
    }
}
