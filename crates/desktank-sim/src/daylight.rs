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

/// How the sun behaves over a whole day at a place (see
/// [`DaylightSchedule::solar_schedule`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolarDaylight {
    /// The sun rises and sets.
    Normal,
    /// The sun stays up all day (midnight sun): always day tint, and
    /// creatures don't sleep for the night.
    PolarDay,
    /// The sun stays down all day: always night tint, while creatures keep
    /// the default rest window ([`crate::night::BEDTIME`] and
    /// [`crate::night::WAKE_TIME`]).
    PolarNight,
}

/// The day's lighting and rest times, in minutes after midnight. The fixed
/// schedule ([`DaylightSchedule::FIXED`]) is the tank's default; hosts can
/// instead follow the sun at the user's location
/// ([`DaylightSchedule::solar_schedule`]). A schedule is a host input like
/// the time of day: it is not saved and not hashed.
///
/// Around sunrise the tint ramps night → dawn → day over
/// `dawn_start..dawn_peak..day_start`, and around sunset day → dusk →
/// night over `dusk_start..dusk_peak..night_start`. Any of these may wrap
/// past midnight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DaylightSchedule {
    /// The tint starts to leave night.
    pub dawn_start: u16,
    /// The warm dawn tint, at sunrise.
    pub dawn_peak: u16,
    /// Full daylight starts.
    pub day_start: u16,
    /// The tint starts to leave day.
    pub dusk_start: u16,
    /// The warm dusk tint, at sunset.
    pub dusk_peak: u16,
    /// Full night starts.
    pub night_start: u16,
    /// The earliest bedtime of any creature (each adds its own stagger).
    pub bedtime: u16,
    /// The earliest wake time of any creature (each adds its own stagger).
    pub wake_time: u16,
    /// Whether the sun stays up or down all day.
    pub polar: PolarDaylight,
}

/// Minutes between the start and end of the lamp's ramps: an hour each way
/// around sunrise and sunset, less when the day or night is short.
const RAMP_HALF: u16 = 60;
/// How long before sunrise creatures start to wake, and after sunset to
/// settle (at most).
const REST_LEAD: u16 = 30;

/// Minutes forward from `from` to `to` on the 24-hour circle.
const fn gap(from: u16, to: u16) -> u16 {
    (to + MINUTES_PER_DAY - from) % MINUTES_PER_DAY
}

/// `minute` shifted back by `by` minutes, on the 24-hour circle.
const fn before(minute: u16, by: u16) -> u16 {
    (minute + MINUTES_PER_DAY - by) % MINUTES_PER_DAY
}

impl DaylightSchedule {
    /// The fixed schedule: sunrise 06:30, sunset 20:30, a dawn ramp from
    /// 05:30 to 07:30 and a dusk ramp from 19:30 to 21:30, creatures
    /// resting from 21:00 to 06:00.
    pub const FIXED: Self = Self {
        dawn_start: 330,
        dawn_peak: 390,
        day_start: 450,
        dusk_start: 1170,
        dusk_peak: 1230,
        night_start: 1290,
        bedtime: crate::night::BEDTIME,
        wake_time: crate::night::WAKE_TIME,
        polar: PolarDaylight::Normal,
    };

    /// A schedule with the sun rising at `sunrise` and setting at `sunset`
    /// (minutes after midnight, each taken modulo a day). The ramps are an
    /// hour each way, shortened so they never overlap or invert when the
    /// day or night is short; creatures wake half an hour before sunrise
    /// and settle half an hour after sunset (less in a short night). Equal
    /// times give a day of no length: the whole day is night.
    #[must_use]
    pub const fn from_sun_times(sunrise: u16, sunset: u16) -> Self {
        let sunrise = sunrise % MINUTES_PER_DAY;
        let sunset = sunset % MINUTES_PER_DAY;
        let day = gap(sunrise, sunset);
        let night = MINUTES_PER_DAY - day;
        let shorter = if day < night { day } else { night };
        let half = if shorter / 2 < RAMP_HALF {
            shorter / 2
        } else {
            RAMP_HALF
        };
        let lead = if night / 2 < REST_LEAD {
            night / 2
        } else {
            REST_LEAD
        };
        Self {
            dawn_start: before(sunrise, half),
            dawn_peak: sunrise,
            day_start: (sunrise + half) % MINUTES_PER_DAY,
            dusk_start: before(sunset, half),
            dusk_peak: sunset,
            night_start: (sunset + half) % MINUTES_PER_DAY,
            bedtime: (sunset + lead) % MINUTES_PER_DAY,
            wake_time: before(sunrise, lead),
            polar: PolarDaylight::Normal,
        }
    }

    /// The schedule for sunrise and sunset at a place on a day, from the
    /// NOAA solar position approximation (good to a few minutes outside the
    /// polar circles). `day_of_year` is 1 to 366 (clamped), `latitude` and
    /// `longitude` are degrees north and east (clamped), and
    /// `utc_offset_minutes` is the local clock's offset from UTC on that
    /// day, so daylight saving is the host's to add. Non-finite input gives
    /// [`DaylightSchedule::FIXED`]. Where the sun doesn't rise or set the
    /// schedule is [`PolarDaylight::PolarDay`] or
    /// [`PolarDaylight::PolarNight`] with the fixed rest times.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::similar_names
    )]
    pub fn solar_schedule(
        day_of_year: u16,
        latitude: f64,
        longitude: f64,
        utc_offset_minutes: i16,
    ) -> Self {
        if !latitude.is_finite() || !longitude.is_finite() {
            return Self::FIXED;
        }
        let latitude = latitude.clamp(-90.0, 90.0);
        let longitude = longitude.clamp(-180.0, 180.0);
        let day = day_of_year.clamp(1, 366);
        // Fractional year, in radians (at local noon).
        let g = core::f64::consts::TAU / 365.0 * f64::from(day - 1);
        let eqtime = 229.18
            * (0.000_075 + 0.001_868 * g.cos()
                - 0.032_077 * g.sin()
                - 0.014_615 * (2.0 * g).cos()
                - 0.040_849 * (2.0 * g).sin());
        let decl = 0.006_918 - 0.399_912 * g.cos() + 0.070_257 * g.sin()
            - 0.006_758 * (2.0 * g).cos()
            + 0.000_907 * (2.0 * g).sin()
            - 0.002_697 * (3.0 * g).cos()
            + 0.001_48 * (3.0 * g).sin();
        let lat = latitude.to_radians();
        // The sun's centre is at the horizon (less refraction and its
        // radius) at 90.833 degrees from the zenith.
        let zenith = 90.833_f64.to_radians();
        let polar = |up: bool| Self {
            polar: if up {
                PolarDaylight::PolarDay
            } else {
                PolarDaylight::PolarNight
            },
            ..Self::FIXED
        };
        if lat.cos().abs() < 1e-9 {
            // At a pole the sun's height is the declination, all day.
            let height = lat.signum() * decl;
            return polar(height > -(core::f64::consts::FRAC_PI_2 - zenith));
        }
        let cos_ha = zenith.cos() / (lat.cos() * decl.cos()) - lat.tan() * decl.tan();
        if cos_ha <= -1.0 {
            return polar(true);
        }
        if cos_ha >= 1.0 {
            return polar(false);
        }
        let ha = cos_ha.acos().to_degrees();
        let offset = f64::from(utc_offset_minutes);
        let minute = |angle: f64| {
            let local = (720.0 - 4.0 * (longitude + angle) - eqtime + offset).rem_euclid(1440.0);
            // In [0, 1440], so the cast is exact.
            (local.round() as u16) % MINUTES_PER_DAY
        };
        Self::from_sun_times(minute(ha), minute(-ha))
    }
}

impl Default for DaylightSchedule {
    fn default() -> Self {
        Self::FIXED
    }
}

/// The colour multiplier (per sRGB-encoded channel, 0–1) for `minute`
/// minutes after midnight (taken modulo a day) under the fixed schedule.
/// Exactly 1 by day.
#[must_use]
pub fn tint(minute: u16) -> [f32; 3] {
    tint_with_schedule(minute, DaylightSchedule::FIXED)
}

/// [`tint`] under `schedule`: linear between the schedule's key times, and
/// continuous across midnight. A polar day is always [`DAY`] and a polar
/// night always [`NIGHT`].
#[must_use]
pub fn tint_with_schedule(minute: u16, schedule: DaylightSchedule) -> [f32; 3] {
    match schedule.polar {
        PolarDaylight::PolarDay => return DAY,
        PolarDaylight::PolarNight => return NIGHT,
        PolarDaylight::Normal => {}
    }
    let keys = [
        (schedule.dawn_start, NIGHT),
        (schedule.dawn_peak, DAWN),
        (schedule.day_start, DAY),
        (schedule.dusk_start, DAY),
        (schedule.dusk_peak, DUSK),
        (schedule.night_start, NIGHT),
    ];
    // Each key's minutes after the first; the schedule keeps them in
    // order around the day.
    let mut at = [0_u16; 6];
    for i in 1..keys.len() {
        at[i] = at[i - 1] + gap(keys[i - 1].0, keys[i].0);
    }
    let now = gap(schedule.dawn_start, minute % MINUTES_PER_DAY);
    // The last key at or before `now`; past the final one it is night.
    let key = at.iter().rposition(|&a| a <= now).unwrap_or(0);
    let Some(&(_, to)) = keys.get(key + 1) else {
        return NIGHT;
    };
    let from = keys[key].1;
    // `now` is before the next key, so the span is not empty.
    let along = f32::from(now - at[key]) / f32::from(at[key + 1] - at[key]);
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

    /// The tint before schedules: linear between fixed key times.
    fn reference_tint(minute: u16) -> [f32; 3] {
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
        let now = minute % MINUTES_PER_DAY;
        let key = KEYS.windows(2).position(|w| now < w[1].0).unwrap_or(0);
        let ((start, from), (end, to)) = (KEYS[key], KEYS[key + 1]);
        let along = f32::from(now - start) / f32::from(end - start);
        [0, 1, 2].map(|c| from[c] + (to[c] - from[c]) * along)
    }

    fn hm(h: u16, m: u16) -> u16 {
        h * 60 + m
    }

    /// Circular distance between two minutes of the day.
    fn apart(a: u16, b: u16) -> u16 {
        let d = a.abs_diff(b);
        d.min(MINUTES_PER_DAY - d)
    }

    #[track_caller]
    fn assert_sun(s: DaylightSchedule, sunrise: u16, sunset: u16) {
        assert_eq!(s.polar, PolarDaylight::Normal);
        assert!(
            apart(s.dawn_peak, sunrise) <= 8 && apart(s.dusk_peak, sunset) <= 8,
            "sunrise {} sunset {}, expected {sunrise} {sunset}",
            s.dawn_peak,
            s.dusk_peak,
        );
    }

    #[test]
    fn fixed_schedule_matches_the_old_tint_for_every_minute() {
        assert_eq!(DaylightSchedule::default(), DaylightSchedule::FIXED);
        for m in 0..MINUTES_PER_DAY {
            let fixed = tint_with_schedule(m, DaylightSchedule::FIXED);
            assert_eq!(fixed, reference_tint(m), "minute {m}");
            assert_eq!(tint(m), fixed, "minute {m}");
        }
        // The fixed times are the sun times 06:30 and 20:30.
        assert_eq!(
            DaylightSchedule::from_sun_times(hm(6, 30), hm(20, 30)),
            DaylightSchedule::FIXED
        );
    }

    #[test]
    fn sun_times_make_ramps_that_never_overlap() {
        for sunrise in (0..MINUTES_PER_DAY).step_by(7) {
            for sunset in (0..MINUTES_PER_DAY).step_by(11) {
                let s = DaylightSchedule::from_sun_times(sunrise, sunset);
                let ramps = gap(s.dawn_start, s.day_start) + gap(s.dusk_start, s.night_start);
                let day = gap(sunrise, sunset);
                assert!(
                    gap(s.dawn_start, s.dawn_peak) == gap(s.dawn_peak, s.day_start)
                        && gap(s.dusk_start, s.dusk_peak) == gap(s.dusk_peak, s.night_start)
                        && ramps <= MINUTES_PER_DAY,
                    "{sunrise} {sunset}: {s:?}"
                );
                // Dawn, then day, then dusk, then night, in order.
                let order = gap(s.dawn_start, s.dawn_peak)
                    + gap(s.dawn_peak, s.day_start)
                    + gap(s.day_start, s.dusk_start)
                    + gap(s.dusk_start, s.dusk_peak)
                    + gap(s.dusk_peak, s.night_start);
                assert!(order <= MINUTES_PER_DAY, "{sunrise} {sunset}: {s:?}");
                // The ramps never reach into the other's.
                assert!(
                    gap(s.dawn_start, s.dusk_start) >= gap(s.dawn_start, s.day_start) || day == 0,
                    "{sunrise} {sunset}: {s:?}"
                );
            }
        }
    }

    #[test]
    fn sun_times_normalise_and_degenerate_days_are_safe() {
        assert_eq!(
            DaylightSchedule::from_sun_times(
                hm(6, 30) + MINUTES_PER_DAY,
                hm(20, 30) + 2 * MINUTES_PER_DAY
            ),
            DaylightSchedule::FIXED
        );
        for m in [0, 600, 1439] {
            let s = DaylightSchedule::from_sun_times(m, m);
            // No daylight: always night.
            for now in (0..MINUTES_PER_DAY).step_by(13) {
                assert_eq!(tint_with_schedule(now, s), NIGHT, "{m} at {now}");
            }
        }
        // Rest starts after sunset and ends before sunrise.
        let s = DaylightSchedule::from_sun_times(hm(8, 0), hm(16, 0));
        assert_eq!((s.bedtime, s.wake_time), (hm(16, 30), hm(7, 30)));
        // A very short night leaves a rest window of no length, not a
        // negative one.
        let s = DaylightSchedule::from_sun_times(hm(0, 10), hm(23, 50));
        assert_eq!(gap(s.bedtime, s.wake_time), 0);
    }

    #[test]
    fn solar_times_follow_the_sun_in_london() {
        // Spring equinox: 20 March.
        assert_sun(
            DaylightSchedule::solar_schedule(79, 51.5, 0.0, 0),
            hm(6, 0),
            hm(18, 10),
        );
        // Summer solstice, in British Summer Time.
        assert_sun(
            DaylightSchedule::solar_schedule(172, 51.5, 0.0, 60),
            hm(4, 43),
            hm(21, 21),
        );
        // Winter solstice, in GMT.
        assert_sun(
            DaylightSchedule::solar_schedule(355, 51.5, 0.0, 0),
            hm(8, 4),
            hm(15, 53),
        );
    }

    #[test]
    fn solar_times_follow_the_seasons_in_sydney() {
        // Southern summer solstice, in AEDT.
        assert_sun(
            DaylightSchedule::solar_schedule(355, -33.87, 151.21, 660),
            hm(5, 41),
            hm(20, 5),
        );
        // Southern winter solstice, in AEST.
        assert_sun(
            DaylightSchedule::solar_schedule(172, -33.87, 151.21, 600),
            hm(7, 0),
            hm(16, 54),
        );
    }

    #[test]
    fn polar_day_and_night_are_flat() {
        // Tromsø: the midnight sun in June, the polar night in December.
        let june = DaylightSchedule::solar_schedule(172, 69.65, 18.96, 120);
        assert_eq!(june.polar, PolarDaylight::PolarDay);
        let december = DaylightSchedule::solar_schedule(355, 69.65, 18.96, 60);
        assert_eq!(december.polar, PolarDaylight::PolarNight);
        // Creatures keep the default rest window in the polar night.
        assert_eq!(
            (december.bedtime, december.wake_time),
            (crate::night::BEDTIME, crate::night::WAKE_TIME)
        );
        for m in 0..MINUTES_PER_DAY {
            assert_eq!(tint_with_schedule(m, june), DAY, "{m}");
            assert_eq!(tint_with_schedule(m, december), NIGHT, "{m}");
        }
        // The poles themselves, where the latitude's cosine is zero.
        assert_eq!(
            DaylightSchedule::solar_schedule(172, 90.0, 0.0, 0).polar,
            PolarDaylight::PolarDay
        );
        assert_eq!(
            DaylightSchedule::solar_schedule(355, 90.0, 0.0, 0).polar,
            PolarDaylight::PolarNight
        );
        assert_eq!(
            DaylightSchedule::solar_schedule(355, -90.0, 0.0, 0).polar,
            PolarDaylight::PolarDay
        );
        assert_eq!(
            DaylightSchedule::solar_schedule(172, -90.0, 0.0, 0).polar,
            PolarDaylight::PolarNight
        );
    }

    #[test]
    fn solar_schedule_clamps_and_rejects_bad_input() {
        let fixed = DaylightSchedule::FIXED;
        assert_eq!(
            DaylightSchedule::solar_schedule(100, f64::NAN, 0.0, 0),
            fixed
        );
        assert_eq!(
            DaylightSchedule::solar_schedule(100, 0.0, f64::INFINITY, 0),
            fixed
        );
        assert_eq!(
            DaylightSchedule::solar_schedule(0, 51.5, 0.0, 0),
            DaylightSchedule::solar_schedule(1, 51.5, 0.0, 0)
        );
        assert_eq!(
            DaylightSchedule::solar_schedule(9999, 51.5, 0.0, 0),
            DaylightSchedule::solar_schedule(366, 51.5, 0.0, 0)
        );
        assert_eq!(
            DaylightSchedule::solar_schedule(100, 123.0, 0.0, 0),
            DaylightSchedule::solar_schedule(100, 90.0, 0.0, 0)
        );
    }

    #[test]
    fn every_place_and_day_gives_a_continuous_tint() {
        let mut worst = 0.0_f32;
        for day in (1..=366).step_by(25) {
            for lat in [
                -80.0, -66.0, -45.0, -10.0, 0.0, 30.0, 51.5, 64.0, 66.0, 70.0,
            ] {
                for lon in [-170.0, -75.0, 0.0, 90.0, 179.0] {
                    for offset in [-60, 840] {
                        let s = DaylightSchedule::solar_schedule(day, lat, lon, offset);
                        for m in 0..MINUTES_PER_DAY {
                            let (a, b) = (tint_with_schedule(m, s), tint_with_schedule(m + 1, s));
                            for i in 0..3 {
                                worst = worst.max((a[i] - b[i]).abs());
                            }
                        }
                    }
                }
            }
        }
        // Ramps may be a few minutes at extreme latitudes, so allow a
        // larger step there; none is a jump from night to day.
        assert!(worst < 0.3, "{worst}");
    }

    #[test]
    fn tint_is_continuous_when_ramps_wrap_midnight() {
        // Dawn straddling midnight, then dusk straddling midnight.
        for (sunrise, sunset) in [(0, hm(12, 0)), (hm(23, 40), hm(11, 0)), (hm(12, 0), 0)] {
            let s = DaylightSchedule::from_sun_times(sunrise, sunset);
            assert_eq!(tint_with_schedule(sunrise, s), DAWN, "{s:?}");
            assert_eq!(tint_with_schedule(sunset, s), DUSK, "{s:?}");
            for m in 0..MINUTES_PER_DAY {
                let (a, b) = (tint_with_schedule(m, s), tint_with_schedule(m + 1, s));
                for i in 0..3 {
                    assert!((a[i] - b[i]).abs() < 0.02, "jump at {m} in {s:?}");
                }
            }
        }
        // The same schedule shifted round the clock gives the same tints.
        let base = DaylightSchedule::from_sun_times(hm(6, 30), hm(20, 30));
        let shifted = DaylightSchedule::from_sun_times(hm(23, 0), hm(13, 0));
        for m in 0..MINUTES_PER_DAY {
            let (a, b) = (
                tint_with_schedule(m, base),
                tint_with_schedule(m + hm(16, 30), shifted),
            );
            for i in 0..3 {
                assert!((a[i] - b[i]).abs() < 1e-5, "minute {m}");
            }
        }
    }
}
