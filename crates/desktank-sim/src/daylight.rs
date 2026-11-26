//! Day and night (SPEC §2.1, §2.6, T6): the tank's lighting from the local
//! time of day, which the host injects (the simulation never reads the
//! wall clock).
//!
//! Lighting happens only at render time: the colour multiplier here tints
//! the drawn tank, and the simulation's light field doesn't change with the
//! hour, so day and night never wake a chunk. The schedule is a tank light:
//! full from 07:30 to 19:30, a warm ramp at dawn (05:30–07:30) and dusk
//! (19:30–21:30), and dim blue moonlight overnight.

use crate::TICKS_PER_SECOND;

/// Minutes in a day.
pub const MINUTES_PER_DAY: u16 = 24 * 60;
/// Midday, in minutes after midnight: the default time, drawn untinted.
pub const NOON: u16 = 12 * 60;

/// World ticks in one minute of the day at real time.
const TICKS_PER_MINUTE: u32 = 60 * TICKS_PER_SECOND;

const NIGHT: [f32; 3] = [0.30, 0.36, 0.58];
const DAWN: [f32; 3] = [0.98, 0.78, 0.62];
const DAY: [f32; 3] = [1.0, 1.0, 1.0];
const DUSK: [f32; 3] = [0.96, 0.70, 0.58];

/// Key times (minutes after midnight) and the tint there; linear between.
const KEYS: [(u16, [f32; 3]); 8] = [
    (0, NIGHT),
    (330, NIGHT),
    (390, DAWN),
    (450, DAY),
    (1170, DAY),
    (1230, DUSK),
    (1290, NIGHT),
    (MINUTES_PER_DAY, NIGHT),
];

/// The tank's clock of the day, advanced in life ticks so it keeps in step
/// with the creatures' (and plants') life at the world's time ratio
/// ([`crate::World::set_time_ratio`]). A pure, deterministic counter: hosts
/// inject the starting minute and never read the wall clock here.
///
/// At ratio 1 the clock runs in real time (a minute of the day per 60
/// seconds); at ratio 60 a day passes in 24 minutes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DaylightClock {
    minute: u16,
    /// Life ticks into `minute`, below [`TICKS_PER_MINUTE`].
    sub_ticks: u32,
}

impl DaylightClock {
    /// A clock at `minute` after midnight (taken modulo a day).
    #[must_use]
    pub const fn new(minute: u16) -> Self {
        Self {
            minute: minute % MINUTES_PER_DAY,
            sub_ticks: 0,
        }
    }

    /// The minute of the day, from 0 to [`MINUTES_PER_DAY`] - 1.
    #[must_use]
    pub const fn minute(&self) -> u16 {
        self.minute
    }

    /// Sets the minute (taken modulo a day), dropping the part of a minute
    /// already counted: a host syncing to a real clock.
    pub const fn set_minute(&mut self, minute: u16) {
        self.minute = minute % MINUTES_PER_DAY;
        self.sub_ticks = 0;
    }

    /// Advances by `life_ticks` life ticks (see
    /// [`crate::World::last_life_ticks`]), wrapping at the end of the day.
    /// Returns the new minute.
    pub fn advance_life_ticks(&mut self, life_ticks: u32) -> u16 {
        let total = u64::from(self.sub_ticks) + u64::from(life_ticks);
        let per_minute = u64::from(TICKS_PER_MINUTE);
        let minutes = (u64::from(self.minute) + total / per_minute) % u64::from(MINUTES_PER_DAY);
        // Both are below their moduli, so the casts are exact.
        #[allow(clippy::cast_possible_truncation)]
        {
            self.minute = minutes as u16;
            self.sub_ticks = (total % per_minute) as u32;
        }
        self.minute
    }

    /// Advances by `seconds` of real time at `time_ratio` (clamped to the
    /// world's range, non-finite taken as 1), wrapping at the end of the
    /// day. Ratios are taken to the thousandth. Returns the new minute.
    pub fn advance_seconds(&mut self, seconds: u32, time_ratio: f32) -> u16 {
        let ratio = if time_ratio.is_finite() {
            time_ratio.clamp(crate::MIN_TIME_RATIO, crate::MAX_TIME_RATIO)
        } else {
            crate::DEFAULT_TIME_RATIO
        };
        // At most 100_000 thousandths.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let step = u64::from((ratio * 1000.0).round() as u32);
        let ticks = u64::from(seconds) * u64::from(TICKS_PER_SECOND) * step / 1000;
        // Wrap whole days first so a long gap can't overflow a u32.
        let day = u64::from(MINUTES_PER_DAY) * u64::from(TICKS_PER_MINUTE);
        #[allow(clippy::cast_possible_truncation)] // below one day's ticks
        self.advance_life_ticks((ticks % day) as u32);
        self.minute
    }
}

/// The colour multiplier (per sRGB-encoded channel, 0–1) for `minute`
/// minutes after midnight (taken modulo a day). Exactly 1 by day.
#[must_use]
pub fn tint(minute: u16) -> [f32; 3] {
    let now = minute % MINUTES_PER_DAY;
    let key = KEYS.windows(2).position(|w| now < w[1].0).unwrap_or(0);
    let ((start, from), (end, to)) = (KEYS[key], KEYS[key + 1]);
    let along = f32::from(now - start) / f32::from(end - start);
    // Where `from` and `to` agree this is exactly `from` (day stays 1).
    [0, 1, 2].map(|c| from[c] + (to[c] - from[c]) * along)
}

/// Brightness of a tint: Rec. 601 luma weights, rounded as the glow
/// mock-ups used.
const fn luma(tint: [f32; 3]) -> f32 {
    0.3 * tint[0] + 0.59 * tint[1] + 0.11 * tint[2]
}

/// How dark the tank is under `tint` (a [`tint`] value), from 0 by day to
/// 1 at night: what a glowing creature's emissive colour is scaled by, so
/// it never shows by day and shows partly at dawn and dusk.
#[must_use]
pub fn darkness(tint: [f32; 3]) -> f32 {
    ((luma(DAY) - luma(tint)) / (luma(DAY) - luma(NIGHT))).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticks(minutes: u32) -> u32 {
        minutes * 60 * TICKS_PER_SECOND
    }

    #[test]
    fn clock_runs_in_real_time_at_ratio_one() {
        let mut clock = DaylightClock::new(NOON);
        assert_eq!(clock.advance_seconds(59, 1.0), NOON);
        assert_eq!(clock.advance_seconds(1, 1.0), NOON + 1);
        assert_eq!(clock.advance_seconds(3600, 1.0), NOON + 61);
        let mut live = DaylightClock::new(0);
        for _ in 0..ticks(90) {
            live.advance_life_ticks(1);
        }
        assert_eq!(live.minute(), 90);
        assert_eq!(live.advance_life_ticks(TICKS_PER_MINUTE - 1), 90);
        assert_eq!(live.advance_life_ticks(1), 91);
    }

    #[test]
    fn clock_scales_with_the_time_ratio() {
        let mut fast = DaylightClock::new(0);
        assert_eq!(fast.advance_seconds(60, 2.0), 2);
        assert_eq!(fast.advance_seconds(30, 2.0), 3);
        let mut hour = DaylightClock::new(0);
        assert_eq!(hour.advance_seconds(60, 60.0), 60);
        // A day passes in 24 minutes at 60x.
        assert_eq!(hour.advance_seconds(24 * 60 - 60, 60.0), 0);
        let mut slow = DaylightClock::new(100);
        assert_eq!(slow.advance_seconds(60, 0.5), 100);
        assert_eq!(slow.advance_seconds(60, 0.5), 101);
        assert_eq!(slow.advance_seconds(120, 0.1), 101);
        assert_eq!(slow.advance_seconds(3480, 0.1), 107);
        // Out-of-range and bad ratios clamp to the world's range.
        let mut c = DaylightClock::new(0);
        assert_eq!(c.advance_seconds(60, f32::NAN), 1);
        assert_eq!(c.advance_seconds(60, 1e9), 101);
        assert_eq!(c.advance_seconds(600, -3.0), 102);
    }

    #[test]
    fn clock_wraps_at_the_end_of_the_day() {
        let mut clock = DaylightClock::new(MINUTES_PER_DAY - 1);
        assert_eq!(clock.advance_seconds(60, 1.0), 0);
        assert_eq!(clock.advance_seconds(24 * 3600, 1.0), 0);
        assert_eq!(clock.advance_seconds(u32::MAX, 100.0), clock.minute());
        assert!(clock.minute() < MINUTES_PER_DAY);
        assert_eq!(DaylightClock::new(MINUTES_PER_DAY + 5).minute(), 5);
        // Whole days and the same split in two steps agree.
        let mut a = DaylightClock::new(300);
        let mut b = DaylightClock::new(300);
        a.advance_life_ticks(ticks(1000) + 77);
        b.advance_life_ticks(ticks(1000));
        b.advance_life_ticks(77);
        assert_eq!(a, b);
        assert_eq!(a.minute(), (300 + 1000) % MINUTES_PER_DAY);
        let mut set = a;
        set.set_minute(10);
        assert_eq!(set, DaylightClock::new(10));
    }

    #[test]
    fn day_is_untinted_and_night_is_dim_and_blue() {
        assert_eq!(tint(NOON), [1.0; 3]);
        assert_eq!(tint(450), [1.0; 3]);
        let night = tint(0);
        assert_eq!(night, NIGHT);
        assert!(night[2] > night[0] && night.iter().all(|&c| c < 0.6));
        assert_eq!(tint(MINUTES_PER_DAY + 1), tint(1));
        // Dawn and dusk are warm and between night and day.
        for m in [390, 1230] {
            let [r, g, b] = tint(m);
            assert!(r > g && g > b && r < 1.0, "{m}: {r} {g} {b}");
        }
        // Continuous: no jumps between neighbouring minutes.
        for m in 0..MINUTES_PER_DAY {
            let (a, b) = (tint(m), tint(m + 1));
            for i in 0..3 {
                assert!((a[i] - b[i]).abs() < 0.02, "jump at {m}");
            }
        }
    }

    #[test]
    fn darkness_is_zero_by_day_one_at_night_and_partial_at_dusk() {
        assert!(darkness(tint(NOON)).abs() < 1e-6);
        assert!(darkness(tint(450)).abs() < 1e-6);
        assert!((darkness(tint(0)) - 1.0).abs() < 1e-6);
        // Dusk's warm key: about a third dark.
        let dusk = darkness(DUSK);
        assert!((0.3..0.45).contains(&dusk), "{dusk}");
        // Brighter than day or darker than night stays in range.
        assert!(darkness([1.2; 3]).abs() < f32::EPSILON);
        assert!((darkness([0.0; 3]) - 1.0).abs() < f32::EPSILON);
        // It rises steadily through the evening.
        let evening: Vec<f32> = (1170..=1290)
            .step_by(10)
            .map(|m| darkness(tint(m)))
            .collect();
        assert!(evening.windows(2).all(|w| w[0] <= w[1]), "{evening:?}");
    }
}
