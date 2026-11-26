//! The user's location, for sunrise and sunset.
//!
//! [`LocationProvider`] is a cached, non-blocking lookup: [`coordinates`]
//! never waits on the OS, and nothing touches the OS's location services
//! until the first call, so a host that doesn't ask (location sync off) does
//! no location work at all.
//!
//! [`coordinates`]: LocationProvider::coordinates

#[cfg(target_os = "macos")]
mod macos;

/// A place on Earth, in degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coordinates {
    /// Degrees north of the equator (`-90.0..=90.0`).
    pub latitude: f64,
    /// Degrees east of the prime meridian (`-180.0..=180.0`).
    pub longitude: f64,
}

/// Looks up the device's location from the OS, if it has location services
/// and the user allows them.
///
/// Dropping the provider stops any location updates it started.
///
/// macOS reads `CoreLocation`: the first call asks the user for access once
/// (when undecided), and a fix is read from the OS cache; updates run only
/// until a fix arrives. Other platforms have no backend yet, so
/// [`LocationProvider::coordinates`] is always `None` there and hosts fall
/// back to the coordinates in the config.
#[derive(Default)]
pub struct LocationProvider {
    #[cfg(target_os = "macos")]
    backend: macos::Backend,
}

impl LocationProvider {
    /// A provider that has not touched the OS yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The last known location, or `None` while there is none: location
    /// services are off or denied, no fix has arrived yet, or this platform
    /// has no backend. Cheap and never blocks, so a host can call it each
    /// minute and keep a manual fallback for `None`.
    #[must_use]
    #[cfg_attr(not(target_os = "macos"), allow(clippy::unused_self))]
    pub fn coordinates(&mut self) -> Option<Coordinates> {
        #[cfg(target_os = "macos")]
        {
            self.backend.coordinates()
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_provider_does_no_os_work() {
        // Construction must be free (and silent) so hosts can hold one cheaply.
        let _ = LocationProvider::new();
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn platforms_without_a_backend_report_no_location() {
        assert_eq!(LocationProvider::new().coordinates(), None);
    }
}
