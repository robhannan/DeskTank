//! What keeps a school loose: each follower's own quirks.
//!
//! Followers once all aimed at one point just ahead of the lead, pushed
//! apart only by spacing, so a school packed into a round blob around that
//! point and swam in lockstep. Now each follower keeps a place of its own
//! in the lead's frame (along its heading and across it) and its own pace,
//! and both drift with smooth noise:
//!
//! - **Place.** A slot from [`AHEAD`] to [`BEHIND`] body lengths along the
//!   heading and [`ACROSS`] either side of it, so the school stretches
//!   along the way it swims. The slot drifts over [`SLOT_TICKS`], so fish
//!   slide through the school, trail behind or swim up level with the lead,
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

/// Slot range along the lead's heading, in body lengths (ahead positive).
const AHEAD: i32 = 1;
const BEHIND: i32 = 6;
/// Slot range either side of the heading, in body lengths.
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
        let span = AHEAD + BEHIND;
        // Centred on the middle of the span, ±span / 2.
        let along = (AHEAD - BEHIND) * len / 2 + self.noise(0, SLOT_TICKS) * span * len / (2 * ONE);
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

/// The lead's heading as a unit vector in 1/256: the way it swims, or the
/// way it faces while nearly still, so slots don't spin as it turns on the
/// spot.
pub(super) fn heading(k: &Kind, lead: &Schoolmate) -> (i32, i32) {
    let speed = i32::try_from(super::dist(lead.vx, lead.vy)).unwrap_or(0);
    if speed <= k.cruise / 4 {
        return (if lead.facing_left { -ONE } else { ONE }, 0);
    }
    (lead.vx * ONE / speed, lead.vy * ONE / speed)
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
    fn slots_stretch_along_the_heading() {
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
        assert!(
            along.0 >= -(BEHIND * len + len) && along.1 <= AHEAD * len + len,
            "{along:?}"
        );
        assert!(
            across.0.abs().max(across.1.abs()) <= ACROSS * len + len,
            "{across:?}"
        );
        assert!(along.1 - along.0 > (across.1 - across.0) * 5 / 4);
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
