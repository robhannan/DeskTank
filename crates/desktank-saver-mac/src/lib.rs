//! DeskTank macOS screensaver: a `ScreenSaverView` subclass loaded by
//! `legacyScreenSaver` from the `.saver` bundle.
//!
//! macOS only. On other targets this crate compiles to an empty library so
//! that `cargo build --workspace` works on every OS.
// FFI crate: `unsafe` is allowed here, only at FFI boundaries, and every block
// needs a `// SAFETY:` comment (SPEC §4.1).
#![allow(unsafe_code)]

#[cfg(target_os = "macos")]
mod saver;

// Dependency edge from SPEC §4.1, kept ahead of first use.
use desktank_render as _;

/// The line the saver logs once when it starts.
#[must_use]
pub fn startup_line() -> String {
    format!("DeskTank saver {}", desktank_build_info::VERSION)
}

/// Logs [`startup_line`] to stderr, which `legacyScreenSaver` sends to the
/// system log. The `ScreenSaverView` subclass calls this once from its
/// initialiser (the saver has no other logging yet).
pub fn log_startup() {
    eprintln!("{}", startup_line());
}

#[cfg(test)]
mod tests {
    #[test]
    fn startup_line_carries_the_commit() {
        assert!(super::startup_line().ends_with(&format!("({})", desktank_build_info::COMMIT)));
    }
}
