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

use super::{CreatureId, ONE};
use crate::rng::splitmix64;

/// One salt per quirk, so no two are correlated.
pub(super) mod salt {
    /// Lifespan within ±10% (the value T9 used before quirks, kept so
    /// lifespans don't change).
    pub const LIFESPAN: u64 = 0x6c69_6665;
    pub const SLOT_ALONG: u64 = 0x5100;
    pub const SLOT_ACROSS: u64 = 0x5101;
    pub const WOBBLE_ALONG: u64 = 0x5102;
    pub const WOBBLE_ACROSS: u64 = 0x5103;
    pub const SPACING: u64 = 0x5104;
    pub const PACE: u64 = 0x5105;
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

    fn key(self, salt: u64) -> u64 {
        splitmix64(self.seed ^ splitmix64(u64::from(self.id.0) ^ salt))
    }

    /// A trait fixed for life: a whole percentage in `lo..=hi`.
    pub(super) fn percent(self, salt: u64, lo: u64, hi: u64) -> u64 {
        lo + self.key(salt) % (hi - lo + 1)
    }

    /// `v` scaled by [`Quirks::percent`].
    pub(super) fn scale(self, salt: u64, lo: u64, hi: u64, v: u64) -> u64 {
        v * self.percent(salt, lo, hi) / 100
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
    fn lifespan_salt_keeps_the_t9_spread() {
        // T9's formula: splitmix64(seed ^ splitmix64(id ^ salt)) % 21.
        for id in 1..50 {
            let old = splitmix64(7 ^ splitmix64(u64::from(id) ^ 0x6c69_6665)) % 21;
            assert_eq!(q(id, 0).percent(salt::LIFESPAN, 90, 110), 90 + old);
        }
    }
}
