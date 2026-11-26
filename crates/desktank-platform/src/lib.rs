//! DeskTank platform layer: per-OS surface hosts (wallpaper, overlay, idle and
//! saver windows), power, visibility, full-screen, idle and modifier-key
//! signals, interact input switching, and where and how tanks are saved.
//!
//! Each OS lives in its own `cfg(target_os = …)` module; on other targets the
//! module is not compiled, so the crate builds everywhere.
// FFI crate: `unsafe` is allowed here, only at FFI boundaries, and every block
// needs a `// SAFETY:` comment (SPEC §4.1).
#![allow(unsafe_code)]

mod location;
mod overlay;
mod power;
mod stats;
mod store;
mod time;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

pub use location::{Coordinates, LocationProvider};
pub use overlay::{
    FullScreenMonitor, OverlayError, OverlayOptions, configure_overlay_window, hide_overlay,
    set_overlay_capture_excluded, set_overlay_click_through, show_overlay_without_focus,
};
pub use power::{PauseReason, PowerDecision, PowerMonitor, PowerPolicy, PowerSignals, PowerSource};
pub use stats::{ProcessStats, process_stats};
pub use store::{KEEP, Loaded, SaveStore, config_dir, data_dir, runtime_dir};
pub use time::{LocalDateTime, local_date_time, local_minute};
#[cfg(target_os = "windows")]
pub use windows::current_user_sid;

// Dependency edge from SPEC §4.1, kept ahead of first use.
use desktank_render as _;
