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
//!   [`CATCH_UP`] full-grown lengths from its place swims at top speed
//!   (so young, whose places are as far out as adults', don't race).
//! - **Spacing.** Its own personal space, 1–1.5 body lengths, pushing it
//!   at most that far however many fish crowd it.
//! - **Alignment.** Its place leans along the mean heading of the mates of
//!   its group within two and a half of its species' lengths (see
//!   [`align_reach`]), [`ALIGN_TICKS`] ahead, so neighbours turn together without the whole school marching in step.
//! - **Splitting.** The school isn't always one group. While its lead's
//!   slow [`SPLIT_TICKS`] noise is above [`SPLIT_ABOVE`] (a quarter of the
//!   time or so) and it has at least [`SPLIT_MIN`] fish, the mates that
//!   draw the other side (a hash of id and an epoch of [`SIDE_TICKS`]; the
//!   lead is always on the first) divide off, as long as each side has two:
//!   the first of them in id order is a **sub-lead**. Like the lead it
//!   wanders on a target of its own, and its side takes places round it,
//!   shaped by its quirks, so the two groups travel apart. When the noise
//!   drops they follow the lead again, and the usual catch-up at top
//!   speed brings them back: the school rejoins. See [`divide`].
//! - **Lead pace.** The lead and any sub-lead wander at a pace of their own,
//!   from three fifths of cruise to halfway to top speed over
//!   [`LEAD_PACE_TICKS`], not one flat cruise, so the whole school speeds
//!   up and slows down (see [`lead_pace`]).
//! - **Walls.** A place is clamped to the rows of the species' zone and
//!   the columns that hold the whole body inside the side walls, as a
//!   wander target is (see [`clamp_to_walls`]).
//!
//! The noise is the creature's [`Quirks`]: a few integer hashes a tick, no
//! state, independent of update order and of any other creature.

use super::quirks::{Quirks, salt};
use super::{Kind, ONE, Schoolmate};
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
/// A follower further than this many of its species' full-grown lengths
/// from its place swims at top speed.
pub(super) const CATCH_UP: i32 = 6;

/// How many ticks ahead of its place a follower leans along the mean
/// heading of the mates it aligns with.
pub(super) const ALIGN_TICKS: i32 = 4;
/// Periods of the school's split noise and of the lead's pace, in ticks.
const SPLIT_TICKS: (u64, u64) = (600, 1_800);
const LEAD_PACE_TICKS: (u64, u64) = (300, 900);
/// The school splits while its split noise (in `-ONE..ONE`) is above this.
const SPLIT_ABOVE: i32 = ONE * 2 / 5;
/// The fewest fish of a species that split.
const SPLIT_MIN: usize = 6;
/// How long a fish keeps its side, in ticks, however many splits come.
const SIDE_TICKS: u64 = 6_000;

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

/// How far a follower looks for neighbours to align with, in 1/256 cells,
/// for a species `len` long (1/256 cells): two and a half lengths.
pub(super) fn align_reach(len: i32) -> i64 {
    i64::from(len) * 5 / 2
}

/// Whether the school whose lead has quirks `lead` is split this tick.
fn splitting(lead: Quirks) -> bool {
    lead.noise(salt::SPLIT, SPLIT_TICKS) > SPLIT_ABOVE
}

/// Whether `id` is on the sub-lead's side in the epoch of `tick`: a coin
/// of its own that keeps its face for [`SIDE_TICKS`], so who goes with whom
/// varies from one stretch to the next but not within a split.
fn far_side(seed: u64, id: super::CreatureId, tick: u64) -> bool {
    let epoch = splitmix64(seed ^ (tick / SIDE_TICKS));
    Quirks::of(epoch, id, 0).percent(salt::SIDE, 0, 1) == 1
}

/// Sets each schoolmate's `head`, the fish its group follows, in the
/// start-of-tick snapshot `mates` (in id order, every schooling species
/// mixed): its species' first fish is the lead of all, unless the species
/// is splitting (see the module docs), when those on the far side follow
/// their first, the sub-lead. A species costs one pass, or three while it
/// splits.
pub(super) fn divide(mates: &mut [Schoolmate], seed: u64, tick: u64) {
    // The species whose lead is found: one bit per species id.
    let mut found = [0_u64; 4];
    for i in 0..mates.len() {
        let (species, lead) = (mates[i].species, mates[i].id);
        let (word, bit) = (usize::from(species.0 >> 6), 1_u64 << (species.0 & 63));
        if found[word] & bit != 0 {
            continue;
        }
        found[word] |= bit;
        let rest = &mut mates[i..];
        let count = rest.iter().filter(|m| m.species == species).count();
        let far = |m: &Schoolmate| m.id != lead && far_side(seed, m.id, tick);
        let sub = if count >= SPLIT_MIN && splitting(Quirks::of(seed, lead, tick)) {
            let mut far_mates = rest.iter().filter(|m| m.species == species && far(m));
            let first = far_mates.next().map(|m| m.id);
            let n = first.map_or(0, |_| 1 + far_mates.count());
            first.filter(|_| n >= 2 && count - n >= 2)
        } else {
            None
        };
        for m in rest.iter_mut().filter(|m| m.species == species) {
            m.head = match sub {
                Some(sub) if far(m) => sub,
                _ => lead,
            };
        }
    }
}

/// A lead's wander speed this tick: from three fifths of cruise to halfway
/// to top speed, drifting with its own noise (so a school speeds up and
/// slows down as a whole, not at one flat pace).
pub(super) fn lead_pace(q: Quirks, k: &Kind) -> i32 {
    let (slow, fast) = (k.cruise * 3 / 5, i32::midpoint(k.cruise, k.top));
    let n = q.noise(salt::LEAD_PACE, LEAD_PACE_TICKS) + ONE;
    (slow + (fast - slow) * n / (2 * ONE)).max(1)
}

/// `tx` limited to the columns (1/256 cells) that hold a body `w` cells
/// wide inside the walls of a tank `width` cells wide, as a wander target
/// is picked: centred at least `w / 2 + 1` cells from each side.
pub(super) fn clamp_to_walls(tx: i32, w: i32, width: usize) -> i32 {
    let hw = i64::from(w / 2 + 1);
    let (lo, hi) = (hw, (super::to_i64(width) - hw - 1).max(hw));
    tx.clamp(super::centre_of(lo), super::centre_of(hi))
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
            fear: 0,
            head: CreatureId(1),
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
    fn a_school_splits_a_fifth_to_a_third_of_the_time_in_spells() {
        let (mut on, mut total, mut spells, mut last) = (0, 0, 0, false);
        for id in 1..40 {
            for t in 0..30_000 {
                let now = splitting(q(id, t));
                on += u32::from(now);
                spells += u32::from(now && !last);
                last = now;
                total += 1;
            }
        }
        assert!(on * 5 >= total && on * 3 <= total, "{on} of {total}");
        // A spell lasts a minute or so (1,200–3,600 ticks a cycle).
        assert!(total / spells >= 1_000, "{spells} spells");
    }

    fn kind() -> Kind {
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
        (k.cruise, k.top) = (60, 120);
        k
    }

    #[test]
    fn a_lead_paces_between_three_fifths_of_cruise_and_halfway_to_top() {
        let k = kind();
        let (mut lo, mut hi, mut last) = (i32::MAX, i32::MIN, lead_pace(q(3, 0), &k));
        for t in 1..20_000 {
            let p = lead_pace(q(3, t), &k);
            // Smooth: never a jump from one tick to the next.
            assert!((p - last).abs() <= 1, "{last} to {p} at {t}");
            (lo, hi, last) = (lo.min(p), hi.max(p), p);
        }
        assert!(lo >= k.cruise * 3 / 5 && hi <= i32::midpoint(k.cruise, k.top));
        // And it uses most of that range: slow spells and brisk ones.
        assert!(lo < k.cruise * 3 / 4 && hi > k.cruise * 5 / 4, "{lo}..{hi}");
    }

    fn mate(id: u32, species: u8) -> Schoolmate {
        Schoolmate {
            id: CreatureId(id),
            species: crate::species::SpeciesId(species),
            x: 0,
            y: 0,
            vx: 0,
            vy: 0,
            fear: 0,
            head: CreatureId(id),
        }
    }

    /// Two species interleaved in id order: `a` fish of species 1 and `b`
    /// of species 2.
    fn mixed(a: u32, b: u32) -> Vec<Schoolmate> {
        (1..=a + b)
            .map(|id| mate(id, if id % 2 == 1 && id / 2 < a { 1 } else { 2 }))
            .collect()
    }

    #[test]
    fn a_school_divides_only_when_both_sides_keep_two_and_never_across_species() {
        let (mut splits, mut calm) = (0, 0);
        for seed in 0..20 {
            for tick in (0..30_000).step_by(100) {
                for (a, b) in [(4, 3), (6, 3), (10, 12)] {
                    let mut m = mixed(a, b);
                    divide(&mut m, seed, tick);
                    for species in [1, 2] {
                        let of: Vec<_> = m.iter().filter(|x| x.species.0 == species).collect();
                        let lead = of[0].id;
                        let heads: std::collections::BTreeSet<_> =
                            of.iter().map(|x| x.head).collect();
                        // Only its own species' fish lead it, and the lead
                        // is always its own head.
                        assert!(of.iter().all(|x| of.iter().any(|y| y.id == x.head)));
                        assert_eq!(of[0].head, lead);
                        assert!(heads.len() <= 2);
                        if heads.len() == 2 {
                            let sub = heads.iter().find(|&&h| h != lead).unwrap();
                            let side = of.iter().filter(|x| x.head == *sub).count();
                            assert!(of.len() >= SPLIT_MIN, "{} fish split", of.len());
                            assert!(side >= 2 && of.len() - side >= 2);
                            // The sub-lead is the first of its side.
                            assert_eq!(of.iter().find(|x| x.head == *sub).unwrap().id, *sub);
                            splits += 1;
                        } else {
                            calm += 1;
                        }
                    }
                }
            }
        }
        assert!(
            splits > 1_000 && calm > splits,
            "{splits} splits, {calm} calm"
        );
    }

    #[test]
    fn sides_keep_their_fish_through_an_epoch_and_change_between_them() {
        let id = |n| CreatureId(n);
        let same = |epoch: u64| {
            (1..40)
                .filter(|&n| far_side(3, id(n), epoch * SIDE_TICKS) == far_side(3, id(n), 0))
                .count()
        };
        for n in 1..40 {
            assert_eq!(
                far_side(3, id(n), 0),
                far_side(3, id(n), SIDE_TICKS - 1),
                "fish {n} changed sides within an epoch"
            );
        }
        // About half on each side, and a new draw each epoch.
        let far = (1..40).filter(|&n| far_side(3, id(n), 0)).count();
        assert!((12..28).contains(&far), "{far}");
        assert!((8..32).contains(&same(1)), "{}", same(1));
    }

    #[test]
    fn a_follower_never_aims_outside_the_side_walls() {
        // A 7-cell fish is centred at least 4 cells from each wall.
        let (lo, hi) = (4 * ONE + ONE / 2, (100 - 4 - 1) * ONE + ONE / 2);
        for tx in [
            i32::MIN / 2,
            -100 * ONE,
            0,
            3 * ONE,
            4 * ONE,
            50 * ONE,
            99 * ONE,
        ] {
            let t = clamp_to_walls(tx, 7, 100);
            assert!((lo..=hi).contains(&t), "{tx} -> {t}");
            assert_eq!(t == tx, (lo..=hi).contains(&tx));
        }
        assert_eq!(clamp_to_walls(i32::MAX / 2, 7, 100), hi);
        // A tank narrower than the fish can't panic.
        let _ = clamp_to_walls(0, 7, 3);
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
