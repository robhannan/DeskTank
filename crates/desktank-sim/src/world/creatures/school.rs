//! What keeps a school loose: each follower's own quirks.
//!
//! Followers once all aimed at one point just ahead of the lead, pushed
//! apart only by spacing, so a school packed into a round blob around that
//! point and swam in lockstep. Now each follower keeps a place of its own
//! in the lead's frame (along its heading and across it) and its own pace,
//! and both drift with smooth noise:
//!
//! - **Place.** A slot up to [`ALONG`] body lengths either way along the
//!   school's axis and [`ACROSS`] either side of it, so the school stretches
//!   along the way it swims. Slots are measured from the point the lead
//!   reaches in 8 ticks. The axis is level, tilted as the lead climbs or
//!   dives (less while it's slow) and never mirrored, so a lead turning
//!   round doesn't fling every slot across the school: the fish turn with
//!   it, and those that were behind now lead. The slot drifts over
//!   [`SLOT_TICKS`], so fish slide through the school, trail or swim ahead,
//!   and a [`WOBBLE_TICKS`] wobble keeps each one moving on its own.
//! - **Pace.** A speed cap between cruise and top over [`PACE_TICKS`];
//!   near the top of its swing it darts ahead, near the bottom it lags
//!   behind at under cruise (falling back a little), and one further than
//!   [`CATCH_UP`] lengths from its place swims at top speed.
//! - **Spacing.** Its own personal space, 1–1.5 body lengths.
//!
//! The noise is value noise from a hash of the world seed and creature id,
//! with a period and phase of each fish's own, so it costs a few integer
//! hashes a tick, needs no state (nothing to save) and never depends on
//! update order or on any other creature.

use super::{CreatureId, Kind, ONE, Schoolmate};
use crate::rng::splitmix64;

/// Slot range either way along the school's axis, in body lengths.
const ALONG: i32 = 7 / 2;
/// Slot range either side of the axis, in body lengths.
const ACROSS: i32 = 2;
/// Periods of the slot's drift, the wobble and the pace, in ticks (each
/// fish draws its own from the range).
const SLOT_TICKS: (u64, u64) = (200, 480);
const WOBBLE_TICKS: (u64, u64) = (24, 60);
const PACE_TICKS: (u64, u64) = (40, 120);
/// A follower further than this many body lengths from its place swims at
/// top speed.
pub(super) const CATCH_UP: i32 = 6;

/// One follower's quirks on one tick.
pub(super) struct Quirks {
    seed: u64,
    id: CreatureId,
    tick: u64,
}

impl Quirks {
    pub(super) fn of(seed: u64, id: CreatureId, tick: u64) -> Self {
        Self { seed, id, tick }
    }

    /// Smooth noise in `-ONE..ONE` on its own `salt`ed channel, with a
    /// period drawn from `periods` (ticks).
    fn noise(&self, salt: u64, periods: (u64, u64)) -> i32 {
        let key = splitmix64(self.seed ^ splitmix64(u64::from(self.id.0) | salt << 32));
        let period = periods.0 + key % (periods.1 - periods.0);
        // A phase of its own, so no two fish change at once.
        let t = self.tick + (key >> 32) % period;
        let (cell, into) = (t / period, t % period);
        let value = |i: u64| {
            // The top 9 bits: 0..512.
            let bits = i32::try_from(splitmix64(key ^ i) >> 55).unwrap_or(0);
            i64::from(bits - ONE)
        };
        // Smoothstep from one knot to the next, in 1/ONE.
        let s = i64::try_from(into * u64::from(ONE.unsigned_abs()) / period).unwrap_or(0);
        let one = i64::from(ONE);
        let s = s * s * (3 * one - 2 * s) / (one * one);
        let (a, b) = (value(cell), value(cell + 1));
        // Between two knots in -ONE..ONE.
        i32::try_from(a + (b - a) * s / one).unwrap_or(0)
    }

    /// Its place in the lead's frame, `(along, across)` in 1/256 cells for
    /// a body `len` long (1/256 cells).
    pub(super) fn slot(&self, len: i32) -> (i32, i32) {
        // ±3.5 lengths: a whole school about seven long.
        let along = self.noise(0, SLOT_TICKS) * (2 * ALONG + 1) * len / (2 * ONE);
        let across = self.noise(1, SLOT_TICKS) * ACROSS * len / ONE;
        let wobble = |salt, reach: i32| self.noise(salt, WOBBLE_TICKS) * reach / ONE;
        (along + wobble(2, len * 3 / 4), across + wobble(3, len / 2))
    }

    /// The personal space it keeps, 1–1.5 body lengths, in 1/256 cells.
    pub(super) fn spacing(&self, len: i32) -> i32 {
        let key = splitmix64(self.seed ^ splitmix64(u64::from(self.id.0) | 4 << 32));
        // 0..128 of 1/256.
        let extra = i32::try_from(key >> 57).unwrap_or(0);
        len * (ONE + extra) / ONE
    }

    /// Its speed cap this tick (see the module docs); `far` from its place
    /// it swims at top speed.
    pub(super) fn pace(&self, k: &Kind, far: bool) -> i32 {
        let p = self.noise(5, PACE_TICKS);
        if far || p > ONE * 5 / 8 {
            k.top
        } else if p < -ONE * 5 / 8 {
            k.cruise * 3 / 4
        } else {
            k.cruise + (k.top - k.cruise) * (p + ONE) / (2 * ONE)
        }
    }
}

/// The school's axis as a unit vector in 1/256: level, tilted by the
/// lead's climb or dive (by less while it swims slower than cruise), and
/// never mirrored, so it turns smoothly however the lead does.
pub(super) fn axis(k: &Kind, lead: &Schoolmate) -> (i32, i32) {
    let speed = i32::try_from(super::dist(lead.vx, lead.vy)).unwrap_or(0);
    if speed == 0 {
        return (ONE, 0);
    }
    // The heading's sine times its cosine: the slope of a level axis tilted
    // by the heading, the same for a heading and its reverse, in 1/256.
    let (hx, hy) = (lead.vx * ONE / speed, lead.vy * ONE / speed);
    let tilt = hx * hy / ONE * speed.min(k.cruise) / k.cruise.max(1);
    let n = i32::try_from(super::dist(ONE, tilt)).unwrap_or(ONE).max(1);
    (ONE * ONE / n, tilt * ONE / n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(id: u32, tick: u64) -> Quirks {
        Quirks::of(7, CreatureId(id), tick)
    }

    #[test]
    fn noise_is_smooth_bounded_and_differs_by_fish() {
        let mut spread = 0;
        for id in 1..20 {
            let mut last = q(id, 0).noise(0, SLOT_TICKS);
            for t in 1..2_000 {
                let n = q(id, t).noise(0, SLOT_TICKS);
                assert!((-ONE..ONE).contains(&n));
                // At most 512 over the shortest period, times 1.5 at the
                // steepest point of the smoothstep.
                assert!((n - last).abs() <= 4, "{id} jumps at {t}");
                last = n;
            }
            spread += (q(id, 500).noise(0, SLOT_TICKS) - q(1, 500).noise(0, SLOT_TICKS)).abs();
        }
        assert!(spread > 19 * ONE / 4, "fish move alike: {spread}");
    }

    #[test]
    fn slots_are_longer_than_wide() {
        let len = 7 * ONE;
        let (mut along, mut across) = ((i32::MAX, i32::MIN), (i32::MAX, i32::MIN));
        for id in 1..40 {
            for t in (0..4_000).step_by(50) {
                let (a, c) = q(id, t).slot(len);
                along = (along.0.min(a), along.1.max(a));
                across = (across.0.min(c), across.1.max(c));
            }
        }
        // Inside the span and wobble, and longer than it is wide.
        let reach = (2 * ALONG + 1) * len / 2 + len;
        assert!(along.0 >= -reach && along.1 <= reach, "{along:?}");
        assert!(
            across.0.abs().max(across.1.abs()) <= ACROSS * len + len,
            "{across:?}"
        );
        assert!(along.1 - along.0 > (across.1 - across.0) * 5 / 4);
    }

    fn lead(vx: i32, vy: i32) -> Schoolmate {
        Schoolmate {
            id: CreatureId(1),
            species: crate::species::SpeciesId(0),
            x: 0,
            y: 0,
            vx,
            vy,
        }
    }

    #[test]
    fn the_axis_turns_smoothly_and_never_mirrors() {
        let mut k = *crate::World::new(
            crate::WorldConfig::default(),
            crate::MaterialTable::builtin().unwrap(),
        )
        .unwrap()
        .creatures
        .kinds
        .iter()
        .flatten()
        .next()
        .unwrap();
        k.cruise = 64;
        // Level when level, still, or swimming either way.
        assert_eq!(axis(&k, &lead(0, 0)), (ONE, 0));
        assert_eq!(axis(&k, &lead(64, 0)), (ONE, 0));
        assert_eq!(axis(&k, &lead(-64, 0)), (ONE, 0));
        // The same line for a heading and its reverse.
        assert_eq!(axis(&k, &lead(60, 30)), axis(&k, &lead(-60, -30)));
        assert!(axis(&k, &lead(60, 30)).1 > 0 && axis(&k, &lead(60, -30)).1 < 0);
        // Turning round through every heading, the axis moves at most a
        // little per step.
        let mut last = axis(&k, &lead(64, 0));
        for deg in 1..=360 {
            let r = f64::from(deg).to_radians();
            #[allow(clippy::cast_possible_truncation)]
            let now = axis(&k, &lead((64.0 * r.cos()) as i32, (64.0 * r.sin()) as i32));
            assert!(
                (now.0 - last.0).abs() + (now.1 - last.1).abs() <= 16,
                "{deg}°"
            );
            last = now;
        }
    }

    #[test]
    fn spacing_is_one_to_one_and_a_half_lengths() {
        let len = 7 * ONE;
        for id in 1..100 {
            let s = q(id, 0).spacing(len);
            assert!((len..=len * 3 / 2).contains(&s), "{s}");
        }
    }
}
