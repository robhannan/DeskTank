//! DeskTank platform layer: per-OS surface hosts (wallpaper, overlay, idle and
//! saver windows), power, visibility, full-screen, idle and modifier-key
//! signals, interact input switching, and where and how tanks are saved.
//!
//! Each OS lives in its own `cfg(target_os = …)` module; on other targets the
//! module is not compiled, so the crate builds everywhere.
// FFI crate: `unsafe` is allowed here, only at FFI boundaries, and every block
// needs a `// SAFETY:` comment (SPEC §4.1).
#![allow(unsafe_code)]

mod modifiers;
mod overlay;
mod power;
mod stats;
mod store;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

pub use modifiers::{Modifiers, held_modifiers};
pub use overlay::{
    FullScreenMonitor, Interactivity, OverlayError, OverlayOptions, configure_overlay_window,
    hide_overlay, set_overlay_capture_excluded, set_overlay_click_through,
    set_overlay_interactivity, show_overlay_without_focus,
};
pub use power::{PauseReason, PowerDecision, PowerMonitor, PowerPolicy, PowerSignals, PowerSource};
pub use stats::{ProcessStats, process_stats};
pub use store::{KEEP, Loaded, SaveStore, config_dir, data_dir, runtime_dir};

/// Local time of day as minutes after midnight (`0..1440`), if the OS reports it.
#[must_use]
pub fn local_minute() -> Option<u16> {
    #[cfg(unix)]
    {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs();
        let time = libc::time_t::try_from(secs).ok()?;
        // SAFETY: `libc::tm` is plain C data, so all-zero is a valid initial
        // value, and `localtime_r` writes into `tm` without using static state.
        let mut tm: libc::tm = unsafe { core::mem::zeroed() };
        // SAFETY: `time` and `tm` are valid pointers for `localtime_r`.
        let ptr = unsafe { libc::localtime_r(&raw const time, &raw mut tm) };
        if ptr.is_null() {
            return None;
        }
        let hour = u16::try_from(tm.tm_hour).ok()?;
        let min = u16::try_from(tm.tm_min).ok()?;
        (hour < 24 && min < 60).then_some(hour * 60 + min)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

// Dependency edge from SPEC §4.1, kept ahead of first use.
use desktank_render as _;
