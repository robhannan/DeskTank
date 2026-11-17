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
