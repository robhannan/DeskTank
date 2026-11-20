//! Linux (X11 and Wayland) platform integration (power and session event
//! sources land in O1-Linux).

use crate::power::PowerSignals;

/// Stub power monitor for Linux; real `UPower` and logind D-Bus wiring lands in
/// `O1-Linux` (SPEC §9 O3).
pub(crate) struct PowerMonitor;

impl PowerMonitor {
    pub(crate) fn new(on_change: impl Fn() + Send + Sync + 'static) -> Self {
        drop(on_change);
        Self
    }

    #[allow(clippy::unused_self)] // macOS reads self; keep uniform backend signature
    pub(crate) fn signals(&self) -> PowerSignals {
        PowerSignals::default()
    }
}
