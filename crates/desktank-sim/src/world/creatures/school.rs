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
//!   Slots are measured in the species' full-grown length: young fish
//!   once took slots scaled to their own small bodies, packed the middle,
//!   and their summed personal space pushed every adult out to one radius,
//!   a ring round a clump of young.
//! - **Shape.** The whole school stretches and squeezes over
//!   [`SHAPE_TICKS`] (from its lead's quirks): long and thin for a while,
//!   then bunched up, or loosened into a broad shoal.
//! - **Pace.** A speed cap between cruise and top over [`PACE_TICKS`];
//!   near the top of its swing it darts ahead, near the bottom it lags
//!   behind at under cruise (falling back a little), and one further than
//!   [`CATCH_UP`] lengths from its place swims at top speed.
//! - **Spacing.** Its own personal space, 1–1.5 body lengths, pushing it
//!   at most that far however many fish crowd it.
//!
//! The noise is the creature's [`Quirks`]: a few integer hashes a tick, no
//! state, independent of update order and of any other creature.

use super::quirks::{Quirks, salt};
use super::{Kind, ONE, Schoolmate};

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

/// Period of the whole school's shape, in ticks (each lead draws its own).
const SHAPE_TICKS: (u64, u64) = (900, 2_400);

/// The whole school's stretch this tick, `(along, across)` in 1/256 of
/// the slot ranges, from its lead's quirks `lead`: the school draws out
/// long and thin, bunches up short, or loosens into a broad shoal, and
/// every fish's place stretches with it.
pub(super) fn shape(lead: Quirks) -> (i32, i32) {
    // Noise in -256..256 to 60–140% along and 40–120% across.
    let along = ONE + lead.noise(salt::SHAPE_ALONG, SHAPE_TICKS) * 2 / 5;
    let across = ONE * 4 / 5 + lead.noise(salt::SHAPE_ACROSS, SHAPE_TICKS) * 2 / 5;
    (along, across)
}

/// Its place in the lead's frame, `(along, across)` in 1/256 cells, for
/// a school of fish `len` long (1/256 cells) stretched by `shape` (see
/// [`shape`]).
pub(super) fn slot(q: Quirks, len: i32, shape: (i32, i32)) -> (i32, i32) {
    // ±3.5 lengths: a whole school about seven long.
    let along = q.noise(salt::SLOT_ALONG, SLOT_TICKS) * (2 * ALONG + 1) * len / (2 * ONE);
    let across = q.noise(salt::SLOT_ACROSS, SLOT_TICKS) * ACROSS * len / ONE;
    let (along, across) = (along * shape.0 / ONE, across * shape.1 / ONE);
    let wobble = |salt, reach: i32| q.noise(salt, WOBBLE_TICKS) * reach / ONE;
    (
        along + wobble(salt::WOBBLE_ALONG, len * 3 / 4),
        across + wobble(salt::WOBBLE_ACROSS, len / 2),
    )
}

/// The personal space it keeps, 1–1.5 body lengths, in 1/256 cells.
pub(super) fn spacing(q: Quirks, len: i32) -> i32 {
    // Lengths are at most 32 cells: far inside i32 at 150%.
    let percent = i32::try_from(q.percent(salt::SPACING, 100, 150)).unwrap_or(100);
    len * percent / 100
}

/// Its speed cap this tick (see the module docs); `far` from its place
/// it swims at top speed.
pub(super) fn pace(q: Quirks, k: &Kind, far: bool) -> i32 {
    let p = q.noise(salt::PACE, PACE_TICKS);
    if far || p > ONE * 5 / 8 {
        k.top
    } else if p < -ONE * 5 / 8 {
        k.cruise * 3 / 4
    } else {
        k.cruise + (k.top - k.cruise) * (p + ONE) / (2 * ONE)
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
    use crate::world::creatures::CreatureId;

    fn q(id: u32, tick: u64) -> Quirks {
        Quirks::of(7, CreatureId(id), tick)
    }

    #[test]
    fn slots_are_longer_than_wide() {
        let len = 7 * ONE;
        let (mut along, mut across) = ((i32::MAX, i32::MIN), (i32::MAX, i32::MIN));
        for id in 1..40 {
            for t in (0..4_000).step_by(50) {
                let (a, c) = slot(q(id, t), len, (ONE, ONE));
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
    fn the_school_shape_drifts_within_its_range() {
        let (mut along, mut across) = ((i32::MAX, i32::MIN), (i32::MAX, i32::MIN));
        let mut last = shape(q(3, 0));
        for t in 1..20_000 {
            let now = shape(q(3, t));
            // Smooth: a school never jumps to a new shape.
            assert!((now.0 - last.0).abs() <= 2 && (now.1 - last.1).abs() <= 2);
            along = (along.0.min(now.0), along.1.max(now.0));
            across = (across.0.min(now.1), across.1.max(now.1));
            last = now;
        }
        // 60–140% along and 40–120% across, and it uses most of that.
        assert!(
            along.0 >= ONE * 3 / 5 && along.1 <= ONE * 7 / 5,
            "{along:?}"
        );
        assert!(
            across.0 >= ONE * 2 / 5 && across.1 <= ONE * 6 / 5,
            "{across:?}"
        );
        assert!(along.1 - along.0 > ONE / 2 && across.1 - across.0 > ONE / 2);
    }

    #[test]
    fn spacing_is_one_to_one_and_a_half_lengths() {
        let len = 7 * ONE;
        for id in 1..100 {
            let s = spacing(q(id, 0), len);
            assert!((len..=len * 3 / 2).contains(&s), "{s}");
        }
    }
}
