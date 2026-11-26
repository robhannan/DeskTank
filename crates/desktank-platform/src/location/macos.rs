//! macOS location through `CoreLocation` (`CLLocationManager`).
//!
//! Nothing here blocks or polls: the host asks for the last known fix, which
//! the OS keeps up to date while updates run on the thread that created the
//! manager (the host's main thread and its run loop). Updates run only until
//! a recent fix arrives (or a few minutes pass), so the location hardware is
//! not kept busy, and the user is asked for access at most once.

use std::time::{Duration, Instant};

use objc2::encode::{Encode, Encoding};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::{msg_send, sel};

use super::Coordinates;

#[link(name = "CoreLocation", kind = "framework")]
unsafe extern "C" {}

/// `CLAuthorizationStatus` values.
const STATUS_NOT_DETERMINED: i32 = 0;
const STATUS_AUTHORIZED_ALWAYS: i32 = 3;
const STATUS_AUTHORIZED_WHEN_IN_USE: i32 = 4;

/// A fix this young is fresh enough to stop updating for.
const FRESH_FIX: Duration = Duration::from_mins(10);
/// How long one round of updates may run without a fresh fix.
const UPDATE_WINDOW: Duration = Duration::from_mins(5);
/// When to look for a new fix after a fresh one, in case the device moved.
const REFRESH_AFTER_FIX: Duration = Duration::from_hours(6);
/// When to try again after a round that found nothing fresh.
const RETRY_AFTER_MISS: Duration = Duration::from_mins(30);

/// `CLLocationCoordinate2D`.
#[repr(C)]
#[derive(Clone, Copy)]
struct CoordinateRaw {
    latitude: f64,
    longitude: f64,
}

// SAFETY: matches the layout of `CLLocationCoordinate2D`, two `CLLocationDegrees`
// (`double`).
unsafe impl Encode for CoordinateRaw {
    const ENCODING: Encoding = Encoding::Struct(
        "CLLocationCoordinate2D",
        &[Encoding::Double, Encoding::Double],
    );
}

/// Whether the user's answer lets us read the location.
fn authorized(status: i32) -> bool {
    matches!(
        status,
        STATUS_AUTHORIZED_ALWAYS | STATUS_AUTHORIZED_WHEN_IN_USE
    )
}

/// A fix is usable if `CoreLocation` marked it valid (a negative accuracy
/// means invalid) and its coordinates are a real place.
fn valid_fix(raw: CoordinateRaw, accuracy: f64) -> Option<Coordinates> {
    (accuracy >= 0.0
        && raw.latitude.is_finite()
        && raw.longitude.is_finite()
        && (-90.0..=90.0).contains(&raw.latitude)
        && (-180.0..=180.0).contains(&raw.longitude))
    .then_some(Coordinates {
        latitude: raw.latitude,
        longitude: raw.longitude,
    })
}

/// Whether the manager is delivering updates.
#[derive(Clone, Copy)]
enum Phase {
    /// Not updating; start again at `next`, if ever.
    Idle { next: Option<Instant> },
    /// Updating until a fresh fix or `deadline`.
    Updating { deadline: Instant },
}

pub(super) struct Backend {
    /// Created on first use, on the calling thread.
    manager: Option<Retained<AnyObject>>,
    /// Whether the access prompt was already requested.
    asked: bool,
    phase: Phase,
    last: Option<Coordinates>,
}

impl Default for Backend {
    fn default() -> Self {
        Self {
            manager: None,
            asked: false,
            phase: Phase::Idle { next: None },
            last: None,
        }
    }
}

impl Backend {
    pub(super) fn coordinates(&mut self) -> Option<Coordinates> {
        let class = AnyClass::get(c"CLLocationManager")?;
        // SAFETY: `+locationServicesEnabled` is a class method taking no
        // arguments and returning `BOOL`.
        let enabled: Bool = unsafe { msg_send![class, locationServicesEnabled] };
        if !enabled.as_bool() {
            self.stop();
            return self.last;
        }
        let manager = if let Some(manager) = &self.manager {
            manager.clone()
        } else {
            // SAFETY: `+new` on a Cocoa class returns a retained instance.
            let manager: Retained<AnyObject> = unsafe { msg_send![class, new] };
            self.manager = Some(manager.clone());
            manager
        };
        match status(class, &manager) {
            STATUS_NOT_DETERMINED => {
                if !self.asked {
                    self.asked = true;
                    // SAFETY: `-requestWhenInUseAuthorization` takes no
                    // arguments and returns nothing.
                    let _: () = unsafe { msg_send![&*manager, requestWhenInUseAuthorization] };
                }
                return None;
            }
            status if authorized(status) => {}
            // Restricted or denied: the manual fallback applies.
            _ => {
                self.stop();
                return None;
            }
        }
        let now = Instant::now();
        match self.phase {
            Phase::Idle { next: Some(next) } if now >= next => self.start(&manager, now),
            Phase::Idle { next: None } => self.start(&manager, now),
            Phase::Idle { next: Some(_) } | Phase::Updating { .. } => {}
        }
        if let Some((fix, age)) = read_fix(&manager) {
            self.last = Some(fix);
            if matches!(self.phase, Phase::Updating { .. }) && age < FRESH_FIX {
                Self::stop_updating(&manager);
                self.phase = Phase::Idle {
                    next: Some(now + REFRESH_AFTER_FIX),
                };
            }
        }
        if let Phase::Updating { deadline } = self.phase
            && now >= deadline
        {
            Self::stop_updating(&manager);
            self.phase = Phase::Idle {
                next: Some(now + RETRY_AFTER_MISS),
            };
        }
        self.last
    }

    fn start(&mut self, manager: &AnyObject, now: Instant) {
        // SAFETY: `-startUpdatingLocation` takes no arguments and returns
        // nothing. Without a delegate the manager only refreshes `-location`.
        let _: () = unsafe { msg_send![manager, startUpdatingLocation] };
        self.phase = Phase::Updating {
            deadline: now + UPDATE_WINDOW,
        };
    }

    fn stop_updating(manager: &AnyObject) {
        // SAFETY: `-stopUpdatingLocation` takes no arguments and returns
        // nothing.
        let _: () = unsafe { msg_send![manager, stopUpdatingLocation] };
    }

    /// Stops updates if they are running, and forgets the schedule.
    fn stop(&mut self) {
        if let (Phase::Updating { .. }, Some(manager)) = (self.phase, &self.manager) {
            Self::stop_updating(manager);
        }
        self.phase = Phase::Idle { next: None };
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The authorization status of `manager` (`+authorizationStatus` before macOS
/// 11, the instance property after).
fn status(class: &AnyClass, manager: &AnyObject) -> i32 {
    // SAFETY: `-respondsToSelector:` takes a selector and returns `BOOL`.
    let instance: Bool =
        unsafe { msg_send![manager, respondsToSelector: sel!(authorizationStatus)] };
    if instance.as_bool() {
        // SAFETY: `-authorizationStatus` takes no arguments and returns a
        // `CLAuthorizationStatus` (`int`).
        unsafe { msg_send![manager, authorizationStatus] }
    } else {
        // SAFETY: `+authorizationStatus` takes no arguments and returns a
        // `CLAuthorizationStatus` (`int`).
        unsafe { msg_send![class, authorizationStatus] }
    }
}

/// The manager's cached fix and its age, if it has a valid one.
fn read_fix(manager: &AnyObject) -> Option<(Coordinates, Duration)> {
    // SAFETY: `-location` takes no arguments and returns an optional
    // `CLLocation` (nil until the first fix).
    let location: Option<Retained<AnyObject>> = unsafe { msg_send![manager, location] };
    let location = location?;
    // SAFETY: `-coordinate` returns a `CLLocationCoordinate2D` by value, and
    // `-horizontalAccuracy` a `CLLocationAccuracy` (`double`).
    let (raw, accuracy): (CoordinateRaw, f64) = unsafe {
        (
            msg_send![&*location, coordinate],
            msg_send![&*location, horizontalAccuracy],
        )
    };
    // SAFETY: `-timestamp` returns a non-nil `NSDate`, and
    // `-timeIntervalSinceNow` an `NSTimeInterval` (`double`), negative for
    // the past.
    let age: f64 = unsafe {
        let timestamp: Retained<AnyObject> = msg_send![&*location, timestamp];
        msg_send![&*timestamp, timeIntervalSinceNow]
    };
    let fix = valid_fix(raw, accuracy)?;
    Some((fix, Duration::try_from_secs_f64((-age).max(0.0)).ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_location_links_and_the_manager_class_resolves() {
        assert!(AnyClass::get(c"CLLocationManager").is_some());
    }

    #[test]
    fn only_always_and_when_in_use_are_authorized() {
        // notDetermined, restricted, denied
        for status in [0, 1, 2] {
            assert!(!authorized(status), "{status}");
        }
        assert!(authorized(3));
        assert!(authorized(4));
    }

    #[test]
    fn invalid_fixes_are_rejected() {
        let at = |latitude, longitude| CoordinateRaw {
            latitude,
            longitude,
        };
        assert_eq!(
            valid_fix(at(51.5, -0.12), 65.0),
            Some(Coordinates {
                latitude: 51.5,
                longitude: -0.12
            })
        );
        assert_eq!(valid_fix(at(51.5, -0.12), -1.0), None, "marked invalid");
        assert_eq!(valid_fix(at(91.0, 0.0), 10.0), None);
        assert_eq!(valid_fix(at(0.0, 181.0), 10.0), None);
        assert_eq!(valid_fix(at(f64::NAN, 0.0), 10.0), None);
        assert_eq!(valid_fix(at(0.0, f64::INFINITY), 10.0), None);
    }
}
