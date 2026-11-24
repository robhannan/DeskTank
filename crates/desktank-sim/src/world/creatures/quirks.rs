//! Each creature's quirks: what keeps two of a kind from acting as one.
//!
//! Species data gives every individual the same numbers, so creatures
//! added together used to swim at one speed, surface on one beat and hop
//! in one rhythm. Quirks vary them per individual in two ways, both from a
//! hash of the world seed and creature id:
//!
//! - **Traits** ([`Quirks::percent`]) are fixed for life: its own speed,
//!   restlessness, favourite depth, breathing and hopping rhythm, boldness.
//! - **Moods** ([`Quirks::noise`]) drift smoothly over seconds: value noise
//!   with a period and phase of each creature's own, never white noise, so
//!   no two change at once and nothing jitters from tick to tick.
//!
//! Neither needs state, so nothing is saved (old saves load and carry on
//! with the same quirks), nothing is allocated, and a creature's quirks
//! never depend on update order or on any other creature. Each use has its
//! own salt in [`salt`], so traits are independent of one another.

use super::{Creature, CreatureId, Kind, ONE, World};
use crate::rng::splitmix64;

/// One salt per quirk, so no two are correlated.
pub(super) mod salt {
    pub const SLOT_ALONG: u64 = 0x5100;
    pub const SLOT_ACROSS: u64 = 0x5101;
    pub const WOBBLE_ALONG: u64 = 0x5102;
    pub const WOBBLE_ACROSS: u64 = 0x5103;
    pub const SPACING: u64 = 0x5104;
    pub const PACE: u64 = 0x5105;
    pub const SHAPE_ALONG: u64 = 0x5106;
    pub const SHAPE_ACROSS: u64 = 0x5107;
    pub const SPLIT: u64 = 0x5108;
    pub const SIDE: u64 = 0x5109;
    pub const LEAD_PACE: u64 = 0x510a;
    pub const REACT: u64 = 0x510b;
    pub const SPEED: u64 = 0x5200;
    pub const MOOD: u64 = 0x5201;
    pub const RESTLESS: u64 = 0x5202;
    pub const DEPTH: u64 = 0x5203;
    pub const BREATH: u64 = 0x5300;
    pub const SKIM: u64 = 0x5301;
    pub const HOP: u64 = 0x5400;
    pub const WASTE: u64 = 0x5401;
    pub const BOLD: u64 = 0x5500;
    pub const PERSIST: u64 = 0x5501;
    pub const PATIENCE: u64 = 0x5502;
}

/// Period of a creature's mood (how brisk it feels), in ticks.
const MOOD_TICKS: (u64, u64) = (80, 240);

/// One creature's quirks on one tick.
#[derive(Clone, Copy)]
pub(super) struct Quirks {
    seed: u64,
    id: CreatureId,
    tick: u64,
}

impl Quirks {
    pub(super) fn of(seed: u64, id: CreatureId, tick: u64) -> Self {
        Self { seed, id, tick }
    }

    /// The id in the high half and the salt in the low, so no two
    /// (creature, quirk) pairs share a key.
    fn key(self, salt: u64) -> u64 {
        splitmix64(self.seed ^ splitmix64(u64::from(self.id.0) << 32 | salt))
    }

    /// A trait fixed for life: a whole percentage in `lo..=hi`.
    pub(super) fn percent(self, salt: u64, lo: u64, hi: u64) -> u64 {
        lo + self.key(salt) % (hi - lo + 1)
    }

    /// `v` scaled by [`Quirks::percent`].
    pub(super) fn scale(self, salt: u64, lo: u64, hi: u64, v: u64) -> u64 {
        v * self.percent(salt, lo, hi) / 100
    }

    /// A tick count `v` scaled by [`Quirks::percent`], at most `100%` of
    /// `v` when `hi <= 100` (so saved counters stay in range).
    pub(super) fn ticks(self, salt: u64, lo: u64, hi: u64, v: u16) -> u16 {
        u16::try_from(self.scale(salt, lo, hi, u64::from(v))).unwrap_or(v)
    }

    /// Its species' movement numbers `k` with its own speed (85–115%) and
    /// a mood that drifts its cruise ±10% over 4–12 s; top speed stays
    /// under a cell per tick, as validated speeds are.
    pub(super) fn own_kind(self, mut k: Kind) -> Kind {
        let speed = i32::try_from(self.percent(salt::SPEED, 85, 115)).unwrap_or(100);
        k.top = (k.top * speed / 100).clamp(1, ONE - 1);
        let mood = self.noise(salt::MOOD, MOOD_TICKS);
        k.cruise = (k.cruise * speed / 100 * (10 * ONE + mood) / (10 * ONE)).clamp(1, k.top);
        k.accel = (k.top / 6).max(2);
        k
    }

    /// A mood: smooth noise in `-ONE..ONE` with a period of its own drawn
    /// from `periods` (ticks, `lo < hi`) and a phase of its own.
    pub(super) fn noise(self, salt: u64, periods: (u64, u64)) -> i32 {
        let key = self.key(salt);
        let period = periods.0 + key % (periods.1 - periods.0);
        let t = self.tick + (key >> 32) % period;
        let (cell, into) = (t / period, t % period);
        let value = |i: u64| {
            // The top 9 bits: 0..512.
            let bits = i32::try_from(splitmix64(key ^ i) >> 55).unwrap_or(0);
            i64::from(bits - ONE)
        };
        // Smoothstep from one knot to the next, in 1/ONE.
        let one = i64::from(ONE);
        let s = i64::try_from(into * u64::from(ONE.unsigned_abs()) / period).unwrap_or(0);
        let s = s * s * (3 * one - 2 * s) / (one * one);
        let (a, b) = (value(cell), value(cell + 1));
        i32::try_from(a + (b - a) * s / one).unwrap_or(0)
    }
}

impl World {
    /// `c`'s quirks this tick.
    pub(super) fn quirks(&self, c: &Creature) -> Quirks {
        Quirks::of(self.seed, c.id, self.tick)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(id: u32, tick: u64) -> Quirks {
        Quirks::of(7, CreatureId(id), tick)
    }

    #[test]
    fn noise_is_smooth_bounded_and_differs_by_creature() {
        let periods = (200, 480);
        let mut spread = 0;
        for id in 1..20 {
            let mut last = q(id, 0).noise(salt::SLOT_ALONG, periods);
            for t in 1..2_000 {
                let n = q(id, t).noise(salt::SLOT_ALONG, periods);
                assert!((-ONE..ONE).contains(&n));
                // At most 512 over the shortest period, times 1.5 at the
                // steepest point of the smoothstep (3.8), plus rounding.
                assert!((n - last).abs() <= 6, "{id} jumps at {t}");
                last = n;
            }
            spread += (q(id, 500).noise(salt::SLOT_ALONG, periods)
                - q(1, 500).noise(salt::SLOT_ALONG, periods))
            .abs();
        }
        assert!(spread > 19 * ONE / 4, "creatures move alike: {spread}");
    }

    #[test]
    fn traits_stay_in_range_vary_and_are_independent() {
        let speeds: Vec<_> = (1..200)
            .map(|id| q(id, 0).percent(salt::SPEED, 85, 115))
            .collect();
        assert!(speeds.iter().all(|s| (85..=115).contains(s)));
        assert_eq!(*speeds.iter().min().unwrap(), 85);
        assert_eq!(*speeds.iter().max().unwrap(), 115);
        // Fixed for life, and not the same as another trait.
        assert_eq!(
            q(9, 0).percent(salt::SPEED, 85, 115),
            q(9, 9_999).percent(salt::SPEED, 85, 115)
        );
        let same = (1..200)
            .filter(|&id| {
                q(id, 0).percent(salt::SPEED, 0, 99) == q(id, 0).percent(salt::BOLD, 0, 99)
            })
            .count();
        assert!(same < 10, "{same}");
    }

    #[test]
    fn no_two_creatures_share_a_quirk() {
        // Neighbouring ids (added together, schooling together) never
        // share a waveform across quirks, as `id ^ salt` keys once did.
        let salts = [
            salt::SLOT_ALONG,
            salt::SLOT_ACROSS,
            salt::WOBBLE_ALONG,
            salt::WOBBLE_ACROSS,
            salt::SPACING,
            salt::PACE,
            salt::SHAPE_ALONG,
            salt::SHAPE_ACROSS,
            salt::SPLIT,
            salt::SIDE,
            salt::LEAD_PACE,
            salt::REACT,
            salt::SPEED,
            salt::MOOD,
            salt::RESTLESS,
            salt::DEPTH,
            salt::BREATH,
            salt::SKIM,
            salt::HOP,
            salt::WASTE,
            salt::BOLD,
            salt::PERSIST,
            salt::PATIENCE,
        ];
        let mut keys = std::collections::BTreeSet::new();
        for id in 1..300 {
            for &s in &salts {
                assert!(keys.insert(q(id, 0).key(s)), "id {id} salt {s:#x}");
            }
        }
    }

    #[test]
    fn noise_and_traits_are_pinned() {
        // Pinned, so every platform (and wasm32) quirks alike, and a change
        // to the hash shows up here first.
        let got: Vec<i64> = [(1, 0), (2, 500), (3, 12_345)]
            .into_iter()
            .flat_map(|(id, t)| {
                let q = q(id, t);
                [
                    i64::from(q.noise(salt::SLOT_ALONG, (200, 480))),
                    i64::from(q.noise(salt::MOOD, MOOD_TICKS)),
                    i64::try_from(q.percent(salt::SPEED, 85, 115)).unwrap(),
                ]
            })
            .collect();
        assert_eq!(got, PINNED);
    }

    const PINNED: [i64; 9] = [-6, -28, 108, -43, 192, 114, -212, 110, 110];

    #[test]
    fn own_speeds_stay_under_a_cell_a_tick_and_ticks_under_their_cap() {
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
        (k.cruise, k.top) = (100, 204); // 16 cells a second, the most allowed
        for id in 1..300 {
            for t in [0, 777, 5_000] {
                let own = q(id, t).own_kind(k);
                assert!(own.top < ONE && own.cruise <= own.top && own.cruise >= 1);
                assert!(q(id, t).ticks(salt::BOLD, 60, 100, 60) <= 60);
            }
        }
    }
}
