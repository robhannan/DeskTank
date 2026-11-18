//! DeskTank platform layer: per-OS surface hosts (wallpaper, overlay, idle and
//! saver windows), power, visibility, full-screen, idle and modifier-key
//! signals, interact input switching, and where and how tanks are saved.
//!
//! Each OS lives in its own `cfg(target_os = …)` module; on other targets the
//! module is not compiled, so the crate builds everywhere.
// FFI crate: `unsafe` is allowed here, only at FFI boundaries, and every block
// needs a `// SAFETY:` comment (SPEC §4.1).
#![allow(unsafe_code)]

mod stats;
mod store;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

pub use stats::{ProcessStats, process_stats};
pub use store::{KEEP, Loaded, SaveStore, data_dir};

// Dependency edge from SPEC §4.1, kept ahead of first use.
use desktank_render as _;
