//! Which build is this? The short git commit hash, stamped at compile time by
//! `build.rs` with the same rules as `scripts/build-web.ts` (the web build has
//! its own stamp). A leaf crate, so a new commit rebuilds only this crate and
//! the binaries that link it, not the simulation or renderer.
#![forbid(unsafe_code)]

/// The short commit hash: 7 hex digits, with `-dirty` appended for a build
/// from a tree with uncommitted changes, or `unknown` when git was unavailable.
pub const COMMIT: &str = env!("DESKTANK_COMMIT");

/// The crate version and commit, as `0.1.0 (abc1234)`.
pub const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("DESKTANK_COMMIT"),
    ")"
);

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether `commit` is a 7-digit hash (optionally `-dirty`) or `unknown`.
    fn is_label(commit: &str) -> bool {
        let hash = commit.strip_suffix("-dirty").unwrap_or(commit);
        commit == "unknown"
            || (hash.len() == 7
                && hash
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
    }

    #[test]
    fn commit_is_a_short_hash_or_unknown() {
        assert!(is_label(COMMIT), "unexpected commit label {COMMIT:?}");
    }

    #[test]
    fn version_pairs_the_crate_version_with_the_commit() {
        assert_eq!(VERSION, format!("{} ({COMMIT})", env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn label_checker_accepts_and_rejects() {
        assert!(is_label("abc1234"));
        assert!(is_label("abc1234-dirty"));
        assert!(is_label("unknown"));
        assert!(!is_label("abc123"));
        assert!(!is_label("ABC1234"));
        assert!(!is_label("abc1234-dirt"));
        assert!(!is_label(""));
    }
}
