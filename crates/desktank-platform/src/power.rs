//! Power policy and OS-agnostic power/session signal monitoring (SPEC §6.2).
//!
//! Each OS backend watches the platform's event sources with zero polling
//! timers and surfaces the latest [`PowerSignals`] snapshot plus a wake-up
//! callback whenever those signals change. Hosts feed those signals and their
//! own surface visibility into [`PowerPolicy::evaluate`] to decide whether to
//! run (at 30 fps on AC or 15 fps on battery) or pause.

use core::fmt;

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
use self::fallback::PowerMonitor as Backend;
#[cfg(target_os = "linux")]
use crate::linux::PowerMonitor as Backend;
#[cfg(target_os = "macos")]
use crate::macos::PowerMonitor as Backend;
#[cfg(target_os = "windows")]
use crate::windows::PowerMonitor as Backend;

/// Active electrical power source of the machine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PowerSource {
    /// Connected to external AC power, or a machine without a battery.
    #[default]
    Ac,
    /// Running on battery or UPS power, with remaining percentage (`0..=100`)
    /// when known.
    Battery {
        /// Remaining battery capacity percentage (`0..=100`), if reported.
        percent: Option<u8>,
    },
}

/// Platform power and session signals read by [`PowerPolicy::evaluate`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PowerSignals {
    /// Current electrical power source and battery charge level.
    pub source: PowerSource,
    /// OS Low Power Mode / energy saver is active.
    pub low_power_mode: bool,
    /// Displays are asleep.
    pub display_asleep: bool,
    /// User session / screen is locked.
    pub session_locked: bool,
}

/// Rules governing frame pacing and pausing across power and session states
/// (SPEC §6.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PowerPolicy {
    /// Frame-rate cap on AC power (default `30`).
    pub ac_fps: u32,
    /// Frame-rate cap on battery power (default `15`).
    pub battery_fps: u32,
    /// Battery percentage threshold below which rendering and world time pause
    /// while on battery (default `20`).
    pub min_battery_percent: u8,
    /// Whether OS Low Power Mode / battery saver pauses the tank (default `true`).
    pub pause_in_low_power_mode: bool,
    /// Whether to keep running while the session is locked (`true` for a
    /// screensaver host animating over the lock screen, SPEC §6.2 & L1;
    /// default `false`).
    pub allow_over_lock: bool,
}

impl Default for PowerPolicy {
    fn default() -> Self {
        Self {
            ac_fps: 30,
            battery_fps: 15,
            min_battery_percent: 20,
            pause_in_low_power_mode: true,
            allow_over_lock: false,
        }
    }
}

/// Why rendering and world time are paused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseReason {
    /// The user paused the app.
    User,
    /// The surface is occluded, minimised, zero-sized, or covered by a
    /// full-screen app.
    Hidden,
    /// Displays are asleep.
    DisplayAsleep,
    /// The user session is locked.
    SessionLocked,
    /// OS Low Power Mode / energy saver is enabled.
    LowPowerMode,
    /// On battery below [`PowerPolicy::min_battery_percent`].
    LowBattery(u8),
}

impl fmt::Display for PauseReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::User => f.write_str("paused"),
            Self::Hidden => f.write_str("hidden"),
            Self::DisplayAsleep => f.write_str("display asleep"),
            Self::SessionLocked => f.write_str("session locked"),
            Self::LowPowerMode => f.write_str("low power mode"),
            Self::LowBattery(p) => write!(f, "battery at {p}%"),
        }
    }
}

/// Outcome of evaluating a [`PowerPolicy`] against current signals and surface
/// visibility.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerDecision {
    /// Run the simulation at 20 Hz and cap rendering to `max_fps`.
    Run {
        /// Maximum frames per second to render.
        max_fps: u32,
    },
    /// Pause both rendering and world time.
    Pause(PauseReason),
}

impl PowerPolicy {
    /// Evaluates whether a host should run (and at what frame cap) or pause.
    #[must_use]
    pub const fn evaluate(&self, signals: PowerSignals, surface_hidden: bool) -> PowerDecision {
        if surface_hidden {
            return PowerDecision::Pause(PauseReason::Hidden);
        }
        if signals.display_asleep {
            return PowerDecision::Pause(PauseReason::DisplayAsleep);
        }
        if signals.session_locked && !self.allow_over_lock {
            return PowerDecision::Pause(PauseReason::SessionLocked);
        }
        if signals.low_power_mode && self.pause_in_low_power_mode {
            return PowerDecision::Pause(PauseReason::LowPowerMode);
        }
        if let PowerSource::Battery { percent: Some(p) } = signals.source
            && p < self.min_battery_percent
        {
            return PowerDecision::Pause(PauseReason::LowBattery(p));
        }
        let max_fps = match signals.source {
            PowerSource::Ac => self.ac_fps,
            PowerSource::Battery { .. } => self.battery_fps,
        };
        PowerDecision::Run { max_fps }
    }
}

/// Watches OS power and session events and invokes a callback when
/// [`PowerSignals`] changes.
pub struct PowerMonitor {
    backend: Backend,
}

impl PowerMonitor {
    /// Starts watching OS power and session signals.
    ///
    /// `on_change` may be invoked from an OS notification thread or queue, so
    /// implementations should only signal/wake their own event loop.
    pub fn new(on_change: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            backend: Backend::new(on_change),
        }
    }

    /// Current snapshot of power and session signals.
    #[must_use]
    pub fn signals(&self) -> PowerSignals {
        self.backend.signals()
    }
}

impl fmt::Debug for PowerMonitor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PowerMonitor")
            .field("signals", &self.signals())
            .finish()
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod fallback {
    use super::PowerSignals;

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ac_runs_at_30_and_battery_drops_to_15() {
        let policy = PowerPolicy::default();
        assert_eq!(
            policy.evaluate(PowerSignals::default(), false),
            PowerDecision::Run { max_fps: 30 }
        );
        let battery = PowerSignals {
            source: PowerSource::Battery { percent: Some(80) },
            ..PowerSignals::default()
        };
        assert_eq!(
            policy.evaluate(battery, false),
            PowerDecision::Run { max_fps: 15 }
        );
        let unknown_battery = PowerSignals {
            source: PowerSource::Battery { percent: None },
            ..PowerSignals::default()
        };
        assert_eq!(
            policy.evaluate(unknown_battery, false),
            PowerDecision::Run { max_fps: 15 }
        );
    }

    #[test]
    fn battery_threshold_pauses_only_strictly_below_minimum_and_only_on_battery() {
        let policy = PowerPolicy::default();
        let at_20 = PowerSignals {
            source: PowerSource::Battery { percent: Some(20) },
            ..PowerSignals::default()
        };
        assert_eq!(
            policy.evaluate(at_20, false),
            PowerDecision::Run { max_fps: 15 }
        );
        let at_19 = PowerSignals {
            source: PowerSource::Battery { percent: Some(19) },
            ..PowerSignals::default()
        };
        assert_eq!(
            policy.evaluate(at_19, false),
            PowerDecision::Pause(PauseReason::LowBattery(19))
        );
        // On AC power there is no battery-percent pause.
        assert_eq!(
            policy.evaluate(PowerSignals::default(), false),
            PowerDecision::Run { max_fps: 30 }
        );
    }

    #[test]
    fn low_power_lock_display_sleep_and_hidden_pause_in_priority_order() {
        let policy = PowerPolicy::default();
        let mut s = PowerSignals {
            source: PowerSource::Battery { percent: Some(10) },
            low_power_mode: true,
            display_asleep: true,
            session_locked: true,
        };
        assert_eq!(
            policy.evaluate(s, true),
            PowerDecision::Pause(PauseReason::Hidden)
        );
        assert_eq!(
            policy.evaluate(s, false),
            PowerDecision::Pause(PauseReason::DisplayAsleep)
        );
        s.display_asleep = false;
        assert_eq!(
            policy.evaluate(s, false),
            PowerDecision::Pause(PauseReason::SessionLocked)
        );
        s.session_locked = false;
        assert_eq!(
            policy.evaluate(s, false),
            PowerDecision::Pause(PauseReason::LowPowerMode)
        );
        s.low_power_mode = false;
        assert_eq!(
            policy.evaluate(s, false),
            PowerDecision::Pause(PauseReason::LowBattery(10))
        );
    }

    #[test]
    fn low_power_mode_setting_can_opt_out() {
        let policy = PowerPolicy {
            pause_in_low_power_mode: false,
            ..PowerPolicy::default()
        };
        let s = PowerSignals {
            low_power_mode: true,
            ..PowerSignals::default()
        };
        assert_eq!(
            policy.evaluate(s, false),
            PowerDecision::Run { max_fps: 30 }
        );
    }

    #[test]
    fn saver_host_runs_over_lock_but_still_pauses_on_other_signals() {
        let policy = PowerPolicy {
            allow_over_lock: true,
            ..PowerPolicy::default()
        };
        let locked_ac = PowerSignals {
            session_locked: true,
            ..PowerSignals::default()
        };
        assert_eq!(
            policy.evaluate(locked_ac, false),
            PowerDecision::Run { max_fps: 30 }
        );

        let locked_battery = PowerSignals {
            source: PowerSource::Battery { percent: Some(50) },
            session_locked: true,
            ..PowerSignals::default()
        };
        assert_eq!(
            policy.evaluate(locked_battery, false),
            PowerDecision::Run { max_fps: 15 }
        );

        let locked_display_asleep = PowerSignals {
            display_asleep: true,
            ..locked_ac
        };
        assert_eq!(
            policy.evaluate(locked_display_asleep, false),
            PowerDecision::Pause(PauseReason::DisplayAsleep)
        );

        let locked_low_power = PowerSignals {
            low_power_mode: true,
            ..locked_ac
        };
        assert_eq!(
            policy.evaluate(locked_low_power, false),
            PowerDecision::Pause(PauseReason::LowPowerMode)
        );

        let locked_low_battery = PowerSignals {
            source: PowerSource::Battery { percent: Some(15) },
            session_locked: true,
            ..PowerSignals::default()
        };
        assert_eq!(
            policy.evaluate(locked_low_battery, false),
            PowerDecision::Pause(PauseReason::LowBattery(15))
        );

        assert_eq!(
            policy.evaluate(locked_ac, true),
            PowerDecision::Pause(PauseReason::Hidden)
        );
    }

    #[test]
    fn pause_reasons_format_cleanly() {
        assert_eq!(PauseReason::User.to_string(), "paused");
        assert_eq!(PauseReason::Hidden.to_string(), "hidden");
        assert_eq!(PauseReason::DisplayAsleep.to_string(), "display asleep");
        assert_eq!(PauseReason::SessionLocked.to_string(), "session locked");
        assert_eq!(PauseReason::LowPowerMode.to_string(), "low power mode");
        assert_eq!(PauseReason::LowBattery(12).to_string(), "battery at 12%");
    }
}
