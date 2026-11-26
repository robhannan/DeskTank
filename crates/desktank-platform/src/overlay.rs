//! Overlay window configuration and full-screen app detection (SPEC §3.2,
//! §6.2, O1).
//!
//! The overlay is a transparent, click-through window above normal windows.
//! [`configure_overlay_window`] and friends apply the per-OS window styling to
//! a window the host already created (any `raw-window-handle` provider), and
//! [`FullScreenMonitor`] reports, from OS events with zero polling timers,
//! whether another app is full-screen so the host can hide and pause.
//!
//! Only macOS is implemented so far. On other targets every window function
//! returns [`OverlayError::Unsupported`] and [`FullScreenMonitor`] always
//! reports `false`.

use core::fmt;

use desktank_render::wgpu::rwh::{HasWindowHandle, RawWindowHandle};

#[cfg(not(target_os = "macos"))]
use self::unsupported as backend;
#[cfg(target_os = "macos")]
use crate::macos as backend;

/// How the overlay window is configured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverlayOptions {
    /// Ask the OS to leave the overlay out of screenshots and screen
    /// recordings (SPEC §3.2; the OS may still capture it on newer releases).
    pub exclude_from_capture: bool,
    /// Let mouse events fall through the overlay to the windows below.
    pub click_through: bool,
}

impl Default for OverlayOptions {
    /// Excluded from capture and click-through, the default overlay.
    fn default() -> Self {
        Self {
            exclude_from_capture: true,
            click_through: true,
        }
    }
}

/// Why an overlay window operation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum OverlayError {
    /// Overlay windows are not implemented for this OS yet.
    Unsupported,
    /// The window handle is not available (for example the window is gone).
    HandleUnavailable,
    /// The window handle is not the kind this OS expects.
    UnsupportedHandle,
    /// The call must be made on the main thread (`AppKit` windows).
    NotMainThread,
    /// The handle's view is not attached to a window.
    NoWindow,
}

impl fmt::Display for OverlayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unsupported => "overlay windows are not supported on this OS",
            Self::HandleUnavailable => "the window handle is unavailable",
            Self::UnsupportedHandle => "the window handle is not of the kind this OS expects",
            Self::NotMainThread => "overlay window calls must run on the main thread",
            Self::NoWindow => "the window handle's view is not attached to a window",
        })
    }
}

impl core::error::Error for OverlayError {}

fn raw_handle(handle: &impl HasWindowHandle) -> Result<RawWindowHandle, OverlayError> {
    handle
        .window_handle()
        .map(|h| h.as_raw())
        .map_err(|_| OverlayError::HandleUnavailable)
}

/// Styles `handle`'s window as the overlay and shows it without taking focus.
///
/// On macOS this sets the floating window level, the transparent background,
/// no shadow, `canJoinAllSpaces | stationary | ignoresCycle` (never
/// `fullScreenAuxiliary`, so the overlay stays out of full-screen Spaces),
/// click-through and capture exclusion from `opts`, then orders the window
/// front without activating the app. Must run on the main thread.
///
/// # Errors
///
/// Returns an [`OverlayError`] if the handle is unavailable or not the kind
/// this OS expects, if called off the main thread, or on an unsupported OS.
pub fn configure_overlay_window(
    handle: &impl HasWindowHandle,
    opts: OverlayOptions,
) -> Result<(), OverlayError> {
    backend::configure_overlay_window(raw_handle(handle)?, opts)
}

/// Excludes (or re-includes) the overlay from screenshots and recordings.
///
/// # Errors
///
/// As [`configure_overlay_window`].
pub fn set_overlay_capture_excluded(
    handle: &impl HasWindowHandle,
    excluded: bool,
) -> Result<(), OverlayError> {
    backend::set_overlay_capture_excluded(raw_handle(handle)?, excluded)
}

/// Turns overlay click-through on or off.
///
/// # Errors
///
/// As [`configure_overlay_window`].
pub fn set_overlay_click_through(
    handle: &impl HasWindowHandle,
    click_through: bool,
) -> Result<(), OverlayError> {
    backend::set_overlay_click_through(raw_handle(handle)?, click_through)
}

/// How the overlay takes input in interact mode (SPEC §2.5, §3.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interactivity {
    /// Click-through: every mouse event falls to the windows below.
    ClickThrough,
    /// Takes the pointer (clicks, drags and moves), without asking for key
    /// focus: the held chord. A click may still make the app active.
    Pointer,
    /// Takes the pointer and key focus, for Esc and typing: the toggle.
    Focused,
}

/// Switches how the overlay takes input.
///
/// On macOS this toggles `ignoresMouseEvents` and, while interactive, adds an
/// always-active mouse-moved tracking area so the brush follows the pointer
/// before the first click. Entering interact mode remembers the app in
/// front; [`Interactivity::Focused`] activates this app and makes the overlay
/// key; going back to [`Interactivity::ClickThrough`] gives focus back to the
/// remembered app if this one took it. Must run on the main thread, and
/// assumes one overlay window.
///
/// # Errors
///
/// As [`configure_overlay_window`].
pub fn set_overlay_interactivity(
    handle: &impl HasWindowHandle,
    mode: Interactivity,
) -> Result<(), OverlayError> {
    backend::set_overlay_interactivity(raw_handle(handle)?, mode)
}

/// Shows the overlay without activating the app or taking key focus.
///
/// # Errors
///
/// As [`configure_overlay_window`].
pub fn show_overlay_without_focus(handle: &impl HasWindowHandle) -> Result<(), OverlayError> {
    backend::show_overlay_without_focus(raw_handle(handle)?)
}

/// Hides the overlay window.
///
/// # Errors
///
/// As [`configure_overlay_window`].
pub fn hide_overlay(handle: &impl HasWindowHandle) -> Result<(), OverlayError> {
    backend::hide_overlay(raw_handle(handle)?)
}

/// Watches whether another app is full-screen and invokes a callback when
/// that changes (SPEC §6.2).
///
/// On macOS this listens for `NSWorkspace` Space, activate, deactivate, hide
/// and unhide notifications and then compares on-screen window bounds with the
/// display bounds (no window titles, so no Screen Recording permission). It
/// has no timers, and unregisters every observer on drop.
pub struct FullScreenMonitor {
    backend: backend::FullScreenMonitor,
}

impl FullScreenMonitor {
    /// Starts watching for full-screen apps.
    ///
    /// `on_change` runs only when [`is_fullscreen_app_active`] changes, and
    /// may run on an OS notification thread, so implementations should only
    /// signal their own event loop.
    ///
    /// [`is_fullscreen_app_active`]: Self::is_fullscreen_app_active
    pub fn new(on_change: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            backend: backend::FullScreenMonitor::new(on_change),
        }
    }

    /// Whether a full-screen app owned by another process is in front.
    #[must_use]
    pub fn is_fullscreen_app_active(&self) -> bool {
        self.backend.is_fullscreen_app_active()
    }

    /// Re-queries the OS immediately and updates the cached full-screen state.
    ///
    /// Useful for a one-shot re-check after a Space transition animation settles.
    #[must_use]
    pub fn refresh(&self) -> bool {
        self.backend.refresh()
    }
}

impl fmt::Debug for FullScreenMonitor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FullScreenMonitor")
            .field("fullscreen_app_active", &self.is_fullscreen_app_active())
            .finish()
    }
}

/// Backend for targets without an overlay implementation yet.
#[cfg(not(target_os = "macos"))]
mod unsupported {
    use desktank_render::wgpu::rwh::RawWindowHandle;

    use super::{Interactivity, OverlayError, OverlayOptions};

    pub(super) fn configure_overlay_window(
        _handle: RawWindowHandle,
        _opts: OverlayOptions,
    ) -> Result<(), OverlayError> {
        Err(OverlayError::Unsupported)
    }

    pub(super) fn set_overlay_capture_excluded(
        _handle: RawWindowHandle,
        _excluded: bool,
    ) -> Result<(), OverlayError> {
        Err(OverlayError::Unsupported)
    }

    pub(super) fn set_overlay_click_through(
        _handle: RawWindowHandle,
        _click_through: bool,
    ) -> Result<(), OverlayError> {
        Err(OverlayError::Unsupported)
    }

    pub(super) fn set_overlay_interactivity(
        _handle: RawWindowHandle,
        _mode: Interactivity,
    ) -> Result<(), OverlayError> {
        Err(OverlayError::Unsupported)
    }

    pub(super) fn show_overlay_without_focus(_handle: RawWindowHandle) -> Result<(), OverlayError> {
        Err(OverlayError::Unsupported)
    }

    pub(super) fn hide_overlay(_handle: RawWindowHandle) -> Result<(), OverlayError> {
        Err(OverlayError::Unsupported)
    }

    /// Full-screen detection lands per OS in O1-Win and O1-Linux (SPEC §6.2).
    pub(super) struct FullScreenMonitor;

    impl FullScreenMonitor {
        pub(super) fn new(on_change: impl Fn() + Send + Sync + 'static) -> Self {
            drop(on_change);
            Self
        }

        #[allow(clippy::unused_self)] // macOS reads self; keep uniform backend signature
        pub(super) fn is_fullscreen_app_active(&self) -> bool {
            false
        }

        #[allow(clippy::unused_self)] // macOS reads self; keep uniform backend signature
        pub(super) fn refresh(&self) -> bool {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_options_exclude_from_capture_and_click_through() {
        let opts = OverlayOptions::default();
        assert!(opts.exclude_from_capture);
        assert!(opts.click_through);
    }

    #[test]
    fn errors_display_a_message() {
        for err in [
            OverlayError::Unsupported,
            OverlayError::HandleUnavailable,
            OverlayError::UnsupportedHandle,
            OverlayError::NotMainThread,
            OverlayError::NoWindow,
        ] {
            assert_ne!(err.to_string(), "");
        }
    }

    #[cfg(not(target_os = "macos"))]
    mod unsupported {
        use core::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        use desktank_render::wgpu::rwh::{HandleError, WindowHandle};

        use super::*;

        struct NoWindow;

        impl HasWindowHandle for NoWindow {
            fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
                Err(HandleError::Unavailable)
            }
        }

        #[test]
        fn unavailable_handle_is_reported() {
            assert_eq!(
                hide_overlay(&NoWindow),
                Err(OverlayError::HandleUnavailable)
            );
        }

        #[test]
        fn monitor_reports_no_full_screen_app_and_never_calls_back() {
            let calls = Arc::new(AtomicUsize::new(0));
            let calls_cb = Arc::clone(&calls);
            {
                let monitor = FullScreenMonitor::new(move || {
                    calls_cb.fetch_add(1, Ordering::Relaxed);
                });
                assert!(!monitor.is_fullscreen_app_active());
                assert!(!monitor.refresh());
            }
            assert_eq!(calls.load(Ordering::Relaxed), 0);
        }
    }
}
