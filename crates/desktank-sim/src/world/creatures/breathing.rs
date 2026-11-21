//! Breathing air (SPEC §2.4): air-breathers (frogs, labyrinth fish) take
//! a breath at the surface every [`BETWEEN_BREATHS`].
//!
//! - **A breath** is taken whenever one is due and there is air right
//!   above the body's top row. It then skims the surface for a few seconds
//!   before diving again.
//! - **Finding air.** While a breath is due the breather rises straight
//!   up if the water above is clear to the surface in its own band (a
//!   floating plant in another band is no obstacle). Else it heads for the
//!   nearest *gap*: a run of surface columns whose first cell below the
//!   air is open water (not a floating leaf, rock or wood). A gap with a
//!   clear [`SHAFT`] below it comes first: it swims across below the
//!   shaft, clear of the roots a floating-plant mat hangs, then up. Else
//!   the nearest gap its body fits in at the top, else any. The gaps are found once a tick, and only while some breather is
//!   searching, by following each column's air–water interface from the
//!   surface row: a cell or two per column, not a scan of the water.
//! - **Giving up.** A search lasts [`SEEK`] of every [`RETRY`] after the
//!   breath fell due; in between, and while there is no gap at all, it
//!   wanders and feeds as usual, so it never freezes under a mat.
//! - **Suffocation.** Once a breath is [`GRACE`] overdue (time to rise
//!   through a tall tank), it builds up stress each second, towards
//!   [`STRESS_LIMIT`] over its species' `air_tolerance_seconds`, and dies
//!   of suffocation there. Frogs last minutes, labyrinth fish far longer.
//!   A breath lets the stress recover as it does in good water.
//!
//! No state is added: the phases follow from the saved `next_breath`.

use super::{Creature, Kind, ONE, TICKS_PER_SECOND, World, centre_of, to_i64};
use crate::STRESS_LIMIT;

/// Ticks between breaths.
const BETWEEN_BREATHS: u64 = 20 * TICKS_PER_SECOND as u64;
/// Ticks it skims the surface for floating food after a breath.
const SKIM: u64 = 3 * TICKS_PER_SECOND as u64;
/// Ticks of each [`RETRY`] cycle, from the breath falling due, that it
/// spends looking for air.
const SEEK: u64 = 60 * TICKS_PER_SECOND as u64;
/// Ticks between the starts of searches while a breath is overdue.
const RETRY: u64 = 90 * TICKS_PER_SECOND as u64;
/// Ticks a breath may be overdue before it suffocates: long enough to
/// rise through the tallest water column.
const GRACE: u64 = 45 * TICKS_PER_SECOND as u64;
/// Most gaps it weighs per tick, nearest first, when none near it has a
/// clear shaft: bounds the cost under a mat broken into many small gaps.
const MAX_GAPS: usize = 8;
/// Rows of open water a gap needs straight below it to be swum up into
/// from below whatever hangs from the surface around it (roots).
const SHAFT: i32 = 20;

/// Open water at the surface: columns `x0..x1` whose first cell below the
/// air is open water, in `row` or, where the surface ripples, higher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Gap {
    x0: usize,
    x1: usize,
    row: usize,
}

impl Gap {
    /// Its width in cells.
    pub(super) fn width(&self) -> usize {
        self.x1 - self.x0
    }
}

/// Stress per second for a species that lasts `seconds` without air.
pub(super) fn air_stress(seconds: u32) -> u32 {
    if seconds == 0 {
        0
    } else {
        STRESS_LIMIT.div_ceil(seconds)
    }
}

impl World {
    /// Whether `c` is searching for air this tick (see [`SEEK`]).
    pub(super) fn seeking_air(&self, k: &Kind, c: &Creature) -> bool {
        k.breathes_air
            && self
                .tick
                .checked_sub(c.next_breath)
                .is_some_and(|late| late % RETRY < SEEK)
    }

    /// Whether `c` has gone [`GRACE`] past a breath (it suffocates).
    pub(super) fn out_of_air(&self, k: &Kind, c: &Creature) -> bool {
        k.air_stress > 0 && self.tick >= c.next_breath.saturating_add(GRACE)
    }

    /// Refreshes the surface gaps if any of `list` is searching for air,
    /// else clears them.
    pub(super) fn refresh_air_gaps(&mut self, list: &[Creature]) {
        let mut gaps = core::mem::take(&mut self.creatures.gaps);
        gaps.clear();
        if list
            .iter()
            .any(|c| self.kind(c.species).is_some_and(|k| self.seeking_air(k, c)))
        {
            self.surface_gaps(&mut gaps);
        }
        self.creatures.gaps = gaps;
    }

    /// Every gap at the surface, left to right.
    pub(super) fn surface_gaps(&self, out: &mut Vec<Gap>) {
        for x in 0..self.width {
            let Some(row) = self.interface(x) else {
                continue;
            };
            let open =
                self.creatures.swim[usize::from(self.cells[row * self.width + x].material.0)];
            match out.last_mut() {
                // Ripples: the run's lowest interface, where a body's top
                // row is in water all along and has air above somewhere.
                Some(g) if open && g.x1 == x => {
                    g.x1 += 1;
                    g.row = g.row.max(row);
                }
                _ if open => out.push(Gap {
                    x0: x,
                    x1: x + 1,
                    row,
                }),
                _ => {}
            }
        }
    }

    /// The first row below the air in column `x`, followed from the
    /// surface row (the water level drifts a little from it), if it has
    /// air above.
    fn interface(&self, x: usize) -> Option<usize> {
        let air = |y: usize| self.cells[y * self.width + x].material == self.creatures.air;
        let mut y = self.surface_row.min(self.height.checked_sub(1)?);
        if air(y) {
            while y + 1 < self.height && air(y) {
                y += 1;
            }
            (!air(y)).then_some(y)
        } else {
            while y > 0 && !air(y - 1) {
                y -= 1;
            }
            (y > 0).then_some(y)
        }
    }

    /// Takes a breath if one is due and there is air above `c`; otherwise,
    /// while searching, where to swim for one. Just after a breath, it
    /// skims the surface for floating food.
    pub(super) fn air_target(&self, k: &Kind, c: &mut Creature) -> Option<(i32, i32, i32)> {
        if self.tick >= c.next_breath {
            let b = self.creature_body(c);
            if b.y0 > 0
                && (b.x0..b.x1)
                    .any(|x| self.cells[(b.y0 - 1) * self.width + x].material == self.creatures.air)
            {
                c.next_breath = self.tick + BETWEEN_BREATHS;
            } else if self.seeking_air(k, c) {
                return self.gap_target(k, c);
            } else {
                return None;
            }
        }
        if c.next_breath.saturating_sub(self.tick) > BETWEEN_BREATHS - SKIM {
            let tx = c
                .food
                .map_or(c.tx, |i| centre_of(to_i64(i as usize % self.width)));
            return Some((tx, c.y - ONE, k.top));
        }
        None
    }

    /// Where to swim for air: the nearest spot in a gap where `c`'s body
    /// sits with its top row in the water's top row. A gap with a clear
    /// shaft [`SHAFT`] rows down below it comes first: it swims across
    /// below the shaft (clear of hanging roots), then straight up. Else the
    /// nearest gap its body fits in at the top, else the nearest at all,
    /// swum to directly. `None` with no gap: the surface is covered.
    fn gap_target(&self, k: &Kind, c: &Creature) -> Option<(i32, i32, i32)> {
        let (half_w, half_h) = (k.w * ONE / 2, k.h * ONE / 2);
        let spot = |g: &Gap| {
            // Half a cell in from each edge where the gap allows, so a
            // body easing to a stop on arrival still lies wholly inside.
            let (lo, hi) = (cell_fixed(g.x0) + half_w, cell_fixed(g.x1) - half_w);
            let margin = ((hi - lo) / 2).clamp(0, ONE / 2);
            let (lo, hi) = (lo + margin, hi - margin);
            // A gap narrower than the body: its middle.
            let x = if lo <= hi {
                c.x.clamp(lo, hi)
            } else {
                i32::midpoint(lo, hi)
            };
            (x, cell_fixed(g.row) + half_h)
        };
        let far = |&(x, y): &(i32, i32)| super::dist(x - c.x, y - c.y);
        let fits = |x: i32, y: i32| {
            self.fits(k, c.turn, x, y, c.band())
                || self.free_band(k, c.turn, x, y, c.band()).is_some()
        };
        let shaft = |(x, y): (i32, i32)| {
            let bottom = y + SHAFT * ONE;
            let mut at = y;
            while at < bottom {
                if !fits(x, at) {
                    return false;
                }
                at += k.h.max(1) * ONE;
            }
            fits(x, bottom)
        };
        // Clear water straight up to the surface in its own band (where a
        // floating plant in another band is no obstacle): rise.
        let cell = usize::try_from(c.x.div_euclid(ONE)).unwrap_or(0);
        if let Some(row) = self.interface(cell.min(self.width - 1)) {
            let top = cell_fixed(row) + half_h;
            let bottom = c.y.min(top + SHAFT * ONE);
            let mut at = top;
            while at < bottom && self.fits(k, c.turn, c.x, at, c.band()) {
                at += k.h.max(1) * ONE;
            }
            if at >= bottom {
                return Some(self.rise(k, c));
            }
        }
        // Else nearest first, walking out both ways from the gaps around it.
        let gaps = &self.creatures.gaps;
        let split = gaps.partition_point(|g| g.x1 <= cell);
        let mut left = gaps[..split].iter().rev().map(spot).peekable();
        let mut right = gaps[split..].iter().map(spot).peekable();
        let (mut fitting, mut nearest) = (None, None);
        let mut weighed = 0;
        let direct = loop {
            if weighed == MAX_GAPS {
                break fitting.or(nearest)?;
            }
            weighed += 1;
            let next = match (left.peek(), right.peek()) {
                (Some(l), Some(r)) if far(l) <= far(r) => left.next(),
                (Some(_) | None, Some(_)) => right.next(),
                (Some(_), None) => left.next(),
                (None, None) => break fitting.or(nearest)?,
            }?;
            if shaft(next) {
                let (x, y) = next;
                // Across to the shaft, below it while not yet under it
                // (clear of hanging roots), then up it to the spot.
                let depth = if (x - c.x).abs() > half_w {
                    c.y.max(y + SHAFT * ONE)
                } else {
                    y
                };
                return Some((x, depth, k.top));
            }
            nearest = nearest.or(Some(next));
            if fitting.is_none() && fits(next.0, next.1) {
                fitting = Some(next);
            }
        };
        let (tx, ty) = direct;
        Some(if (tx - c.x).abs() < ONE {
            self.rise(k, c)
        } else {
            (tx, ty, k.top)
        })
    }

    /// Straight up for air, sliding along a ledge overhead towards the
    /// wander target until the way up is clear.
    fn rise(&self, k: &Kind, c: &Creature) -> (i32, i32, i32) {
        let tx = if self.fits(k, c.turn, c.x, c.y - ONE, c.band()) {
            c.x
        } else {
            c.tx
        };
        (tx, c.y - 8 * ONE, k.top)
    }
}

/// The fixed-point left or top edge of cell `v`.
fn cell_fixed(v: usize) -> i32 {
    i32::try_from(v)
        .unwrap_or(i32::MAX / ONE)
        .saturating_mul(ONE)
}
