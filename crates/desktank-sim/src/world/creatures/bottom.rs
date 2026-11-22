//! How bottom dwellers leave the bottom, or go into it.
//!
//! - **Can't swim.** A species without `swim_every` never leaves the
//!   faces it walks or crawls on (snails, crabs).
//! - **Would rather not swim.** A bottom walker or surface crawler with
//!   `swim_every` (shrimp, plecos, otocinclus, corydoras, loaches, the
//!   axolotl) has a chance at one window of [`SWIM_TICKS`] in each
//!   `swim_every`: one chance in [`SWIM_ODDS`], so its swims come
//!   irregularly. In a swim it lifts off (a crawler turns upright, if it
//!   fits), rises at a slant (no higher than its zone) for the first half
//!   and glides down for the second, at its top speed, then settles: a
//!   walker sinks back to the bed, a crawler takes hold of whatever it
//!   lands on. It doesn't swim while it is after food or afraid.
//! - **Burrowing.** A walker with `burrow_every` (the kuhli loach) gets a
//!   window of [`BURROW_TICKS`] in each `burrow_every`, again at one chance
//!   in [`BURROW_ODDS`]. In one, resting on loose substrate (a powder from
//!   the substrate palette: sand, gravel, aquasoil; never rock, wood or glass)
//!   under at least half its body and nothing else under it (a poured
//!   bed's top is ragged), it digs straight down at [`DIG_SPEED`]. The
//!   substrate really moves: each loose cell its body digs into is swapped
//!   with the water on a spoil heap beside the burrow (mirrored: a column's
//!   spoil lands as far out as it was in), and the powder physics settles
//!   the heaps at their angle of repose, leaving a dip between them. It
//!   digs until it is a body's height below the ground beside it (every
//!   other window, half that), or it reaches something it can't dig (rock,
//!   wood, a plant, the glass), then draws the bed back over itself: the
//!   top grains of the heaps are swapped into the open water of its body,
//!   filling it from the bottom up to the ground beside it, so it lies
//!   under the bed (or with its back showing). Afterwards (or early, when
//!   it is hungry and there is food) it wriggles up out of whatever covers
//!   it, without moving those cells, which close in under it. Every move is
//!   a swap of a loose cell with water, so substrate is never made or lost
//!   and rock, wood and plants are never touched. While in the bed it
//!   doesn't move or eat, a predator can't take it, and the renderer hides
//!   it wherever loose substrate covers it. Only cells it digs change, so
//!   only their chunks redraw.
//!
//! Windows are picked from the world seed, the creature's id and the
//! window's count, like a shrimp's hops, so they need no saved state: a
//! burrow is the body's position and the cells it moved, which are saved.

use super::{Creature, Kind, ONE, TICKS_PER_SECOND, World, Zone, extent};
use crate::grid::Rect;
use crate::material::MovementClass;
use crate::palette::PalettePath;
use crate::rng::splitmix64;

/// Ticks a swim lasts (4 s): rising for the first half, gliding down for
/// the second.
pub const SWIM_TICKS: u64 = 4 * TICKS_PER_SECOND as u64;
/// One swim window in this many is taken.
pub const SWIM_ODDS: u64 = 2;
/// Ticks a burrow window lasts (16 s): it digs and holds for all but the
/// last `dig_ticks`, which leave it time to wriggle back out.
pub const BURROW_TICKS: u64 = 16 * TICKS_PER_SECOND as u64;
/// One burrow window in this many is taken.
pub const BURROW_ODDS: u64 = 2;
/// How fast a burrower digs in or out, in 1/256 cell per tick (a cell in
/// a quarter second).
pub const DIG_SPEED: i32 = ONE / 10;
/// Cells a swim heads sideways for every cell it rises or sinks.
const SWIM_SLANT: i32 = 2;
/// Grains a dug-in burrower draws back over itself a tick.
const BACKFILL_CELLS: u64 = 2;
/// Rows above its body a burrower throws spoil up to.
const SPOIL_REACH: usize = 6;
/// Window salts, so a creature's swims and burrows fall apart.
const SWIM_SALT: u64 = 0x7377_696d;
const BURROW_SALT: u64 = 0x6275_7272;

/// A swim under way: whether it is still rising, and which way it heads
/// (-1 left, 1 right).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Swim {
    pub rising: bool,
    pub dir: i32,
}

impl World {
    /// The swim `c` is in this tick, if any.
    pub(super) fn swim(&self, k: &Kind, c: &Creature) -> Option<Swim> {
        let (phase, h) = window(self.seed, self.tick, c, k.swim_every, SWIM_TICKS, SWIM_SALT)?;
        (h % SWIM_ODDS == 0).then_some(Swim {
            rising: phase < SWIM_TICKS / 2,
            dir: if (h >> 8) & 1 == 0 { 1 } else { -1 },
        })
    }

    /// How deep `c` aims to dig this tick, in 1/256 cell, if it is in a
    /// burrow window: its body's height, or half of it every other window.
    fn burrow_target(&self, k: &Kind, c: &Creature) -> Option<i32> {
        let (_, h) = window(
            self.seed,
            self.tick,
            c,
            k.burrow_every,
            BURROW_TICKS - dig_ticks(k),
            BURROW_SALT,
        )?;
        (h % BURROW_ODDS == 0).then_some(if (h >> 8) & 1 == 0 {
            k.h * ONE
        } else {
            k.h * ONE / 2
        })
    }

    /// One tick of a swim: a crawler turns upright to lift off (or, if it
    /// can't, keeps crawling: returns false), then it heads up or down at
    /// a slant at its top speed. A rise stops at its zone's top.
    pub(super) fn swim_step(&self, k: &Kind, c: &mut Creature, swim: Swim) -> bool {
        let mut dir = swim.dir;
        if k.zone == Zone::Surfaces && c.turn != 0 {
            // Off a wall: upright, shifted away from it as far as the
            // turned box is narrower, and heading away.
            let foot = super::FEET[usize::from(c.turn)].0;
            let slide = (k.w - k.h).abs() / 2 + 1;
            let Some(x) = (0..=slide)
                .map(|s| c.x - foot * s * ONE)
                .find(|&x| self.fits(k, 0, x, c.y, c.band()))
            else {
                return false;
            };
            if foot != 0 {
                dir = -foot;
            }
            (c.x, c.turn) = (x, 0);
        }
        let top = c.y - extent(k, c.turn).1 * ONE / 2;
        // It rises no higher than its zone's top, nor within two rows of
        // the surface.
        let (zone_top, _) = super::zone(k, self.surface_row, self.height);
        let floor = i32::try_from(zone_top.max(self.surface_row + 2)).unwrap_or(i32::MAX) * ONE;
        let dy = if !swim.rising {
            ONE
        } else if top - ONE >= floor {
            -ONE
        } else {
            0
        };
        let (tx, ty) = (c.x + dir * SWIM_SLANT * 4 * ONE, c.y + dy * 4);
        // When a loach meets hardscape, spend the rising part of the swim
        // climbing its face. The usual shallow arc sends it straight back
        // into the same pocket, especially at the top of its depth zone.
        let ahead = c.x + dir * ONE;
        let climb = k.burrow_every > 0
            && swim.rising
            && top - ONE >= i32::try_from(self.surface_row + 2).unwrap_or(i32::MAX) * ONE
            && super::body(k, c.turn, ahead, c.y, self.width, self.height).is_some()
            && !self.fits(k, c.turn, ahead, c.y, c.band());
        if climb {
            super::steer(k, c, c.x, c.y - 4 * ONE, k.top, true);
        } else {
            super::steer(k, c, tx, ty, k.top, false);
        }
        if self.try_move(k, c) != super::Move::Moved {
            (c.vx, c.vy) = (0, 0);
        }
        c.face_along(c.vx, c.vy);
        true
    }

    /// A crawler back from a swim: off a diagonal heading (a crawl steps
    /// along one axis) and, where it fits, back on whole cells (its odd
    /// sides centre it on a cell).
    pub(super) fn settle(&self, k: &Kind, c: &mut Creature) {
        if c.vx != 0 && c.vy != 0 {
            (c.vx, c.vy) = (0, 0);
        }
        // The centre of the cell it is in is the nearest cell centre.
        let snap = |v: i32| v.div_euclid(ONE) * ONE + ONE / 2;
        let (x, y) = (snap(c.x), snap(c.y));
        if (x, y) != (c.x, c.y) && self.fits(k, c.turn, x, y, c.band()) {
            (c.x, c.y) = (x, y);
        }
    }

    /// One tick of burrowing for a burrower: digs in, holds or digs out.
    /// Returns whether it is in the bed (the rest of its tick is skipped);
    /// false leaves it to move as usual or, if something other than loose
    /// substrate buried it, to get free as any creature does.
    pub(super) fn burrow(&mut self, k: &Kind, c: &mut Creature, hungry: bool) -> bool {
        let Some(covered) = self.covered(k, c) else {
            return false;
        };
        let depth = self.bed_depth(k, c);
        let target = self
            .burrow_target(k, c)
            .filter(|_| !(hungry && self.anything_to_eat(k)))
            .filter(|_| covered || depth > 0 || (c.vy >= 0 && self.on_loose_bed(k, c)));
        if let Some(target) = target {
            if depth >= target || !self.dig_down(k, c) {
                self.backfill(k, c);
            }
        } else if covered {
            // Out through whatever settled over it: it wriggles up through
            // the loose grains, which close in under it.
            let y = c.y - DIG_SPEED;
            if !self.fits_burrowed(k, c.x, y) {
                return false;
            }
            c.y = y;
        } else {
            return false;
        }
        (c.vx, c.vy) = (0, 0);
        true
    }

    /// Digs `c` a step down. Each loose cell its body moves into is
    /// swapped with the water on top of a spoil heap beside it (see
    /// [`World::spoil_spot`]), so substrate is moved, never made or lost;
    /// anything else under it (rock, wood, a plant, the floor glass) stops
    /// it. Where no heap has room it waits, keeping the cells moved so far.
    /// Returns whether it moved down.
    #[allow(clippy::many_single_char_names)] // cell maths
    fn dig_down(&mut self, k: &Kind, c: &mut Creature) -> bool {
        let y = c.y + DIG_SPEED;
        let (Some(old), Some(new)) = (
            super::body(k, 0, c.x, c.y, self.width, self.height),
            super::body(k, 0, c.x, y, self.width, self.height),
        ) else {
            return false;
        };
        let cells = |row: usize| (new.x0..new.x1).map(move |x| (x, row));
        let fresh = || (old.y1..new.y1).flat_map(cells);
        if !fresh().all(|(x, row)| {
            let i = row * self.width + x;
            self.loose(i) || self.creatures.swim[usize::from(self.cells[i].material.0)]
        }) {
            return false;
        }
        for (x, row) in fresh() {
            let i = row * self.width + x;
            if self.loose(i) {
                let Some(j) = self.spoil_spot(&new, x) else {
                    return false;
                };
                self.swap(i, j);
            }
        }
        c.y = y;
        true
    }

    /// Dug in, a burrower draws the bed back over itself: up to
    /// [`BACKFILL_CELLS`] a tick, the top grain of the tallest spoil heap
    /// (or rim) beside it is swapped with the lowest open water in its
    /// body, filling it from the bottom up to the ground beside it. So a
    /// burrower a body's height down ends up under the bed, and one half
    /// that deep with its back showing; substrate is moved, never made or
    /// lost, and the heaps shrink by what it takes back.
    fn backfill(&mut self, k: &Kind, c: &Creature) {
        let Some(b) = super::body(k, 0, c.x, c.y, self.width, self.height) else {
            return;
        };
        // The ground beside it: the lower side's top row.
        let level = self.ground_beside(&b).max(b.y0);
        let reach = b.x1 - b.x0 + 2;
        let swim = |w: &World, i: usize| w.creatures.swim[usize::from(w.cells[i].material.0)];
        for n in 0..BACKFILL_CELLS {
            // The lowest open water in the body at or below that level,
            // resting on something, columns taken in turn.
            let width = b.x1 - b.x0;
            let start = usize::try_from((self.tick + n) % width as u64).unwrap_or(0);
            let to = (0..width)
                .map(|d| b.x0 + (start + d) % width)
                .find_map(|x| {
                    (level..b.y1)
                        .rev()
                        .map(|row| row * self.width + x)
                        .find(|&i| {
                            // Water only: food or detritus in it stays put.
                            self.cells[i].material == self.creatures.water
                                && (i + self.width >= self.cells.len()
                                    || !swim(self, i + self.width))
                        })
                });
            // The top grain of the tallest heap beside it, nearest first.
            let from = (1..=reach)
                .flat_map(|d| {
                    [
                        b.x0.checked_sub(d),
                        Some(b.x1 + d - 1).filter(|&x| x < self.width),
                    ]
                })
                .flatten()
                .filter_map(|x| self.heap_grain(x, &b))
                .min_by_key(|&i| i / self.width);
            let (Some(to), Some(from)) = (to, from) else {
                return;
            };
            if from / self.width >= to / self.width {
                // No heap stands above where the grain would go.
                return;
            }
            self.swap(from, to);
        }
    }

    /// The top loose grain in column `x` beside `body`, with open water
    /// over it, from [`SPOIL_REACH`] rows above the body down to its
    /// bottom.
    fn heap_grain(&self, x: usize, body: &Rect) -> Option<usize> {
        let top = body
            .y0
            .saturating_sub(SPOIL_REACH)
            .max(self.surface_row + 1);
        let at = |row: usize| row * self.width + x;
        let swim = |i: usize| self.creatures.swim[usize::from(self.cells[i].material.0)];
        if !swim(at(top)) {
            return None;
        }
        let row = (top + 1..body.y1).find(|&row| !swim(at(row)))?;
        self.loose(at(row)).then(|| at(row))
    }

    /// Where a loose cell dug from column `x` under `body` goes: the water
    /// cell on top of the ground in a column beyond the body (a cell gap
    /// from it, so the burrow's rim stays put), on the side nearer `x` and
    /// as far out as `x` is in, else on the other side. The
    /// powder then settles as any poured grain does.
    fn spoil_spot(&self, body: &Rect, x: usize) -> Option<usize> {
        let left = x < usize::midpoint(body.x0, body.x1);
        // Mirrored: a column's spoil lands as far out as it is in.
        let off = if left { x - body.x0 } else { body.x1 - 1 - x };
        [left, !left].into_iter().find_map(|left| {
            let col = if left {
                body.x0.checked_sub(2 + off)?
            } else {
                Some(body.x1 + 1 + off).filter(|&col| col < self.width)?
            };
            self.heap_top(col, body)
        })
    }

    /// The water cell on the ground in column `col`, scanning down from
    /// [`SPOIL_REACH`] rows above `body` through open water (`None` where
    /// the column there is not open water, under the surface).
    fn heap_top(&self, col: usize, body: &Rect) -> Option<usize> {
        let top = body
            .y0
            .saturating_sub(SPOIL_REACH)
            .max(self.surface_row + 1);
        let water = self.creatures.water;
        let at = |row: usize| row * self.width + col;
        if top >= body.y1 || self.cells[at(top)].material != water {
            return None;
        }
        let ground = (top + 1..=body.y1)
            .find(|&row| row == self.height || self.cells[at(row)].material != water)?;
        let below = ground < self.height
            && self.creatures.swim[usize::from(self.cells[at(ground)].material.0)];
        (!below).then(|| at(ground - 1))
    }

    /// Whether loose substrate overlaps `c`'s body (`Some(false)`: nothing
    /// does). `None` if anything else it can't swim through does, or it
    /// sticks out of the grid.
    fn covered(&self, k: &Kind, c: &Creature) -> Option<bool> {
        let b = super::body(k, c.turn, c.x, c.y, self.width, self.height)?;
        let mut covered = false;
        for row in b.y0..b.y1 {
            for i in row * self.width + b.x0..row * self.width + b.x1 {
                if self.loose(i) {
                    covered = true;
                } else if self.blocks(i, Some(c.band())) {
                    return None;
                }
            }
        }
        Some(covered)
    }

    /// How far, in 1/256 cell, `c`'s body's bottom edge is below the ground
    /// just beside it (the lower side's; 0 when level with or above it).
    fn bed_depth(&self, k: &Kind, c: &Creature) -> i32 {
        let Some(b) = super::body(k, c.turn, c.x, c.y, self.width, self.height) else {
            return 0;
        };
        let bottom = c.y + extent(k, c.turn).1 * ONE / 2;
        let ground = i32::try_from(self.ground_beside(&b)).unwrap_or(i32::MAX);
        (bottom - ground * ONE).max(0)
    }

    /// The top row of the ground in the columns just beside `body`, from
    /// its top down: the lower side's (the grid's height where there is
    /// none, or no column beside it).
    fn ground_beside(&self, body: &Rect) -> usize {
        [
            body.x0.checked_sub(1),
            Some(body.x1).filter(|&x| x < self.width),
        ]
        .into_iter()
        .flatten()
        .map(|x| {
            (body.y0..self.height)
                .find(|&row| {
                    !self.creatures.swim[usize::from(self.cells[row * self.width + x].material.0)]
                })
                .unwrap_or(self.height)
        })
        .max()
        .unwrap_or(self.height)
    }

    /// How deep a burrower is in the bed, in 1/256 cell (at most its
    /// height; 0 for one that isn't, and for every other species).
    pub(super) fn sunk_depth(&self, c: &Creature) -> i32 {
        self.grown_kind(c)
            .filter(|k| k.burrow_every > 0)
            .map_or(0, |k| self.bed_depth(&k, c).min(k.h * ONE))
    }

    /// Whether `c`'s body may move to `y`: it lies in the grid on water and
    /// loose substrate only.
    fn fits_burrowed(&self, k: &Kind, x: i32, y: i32) -> bool {
        let Some(b) = super::body(k, 0, x, y, self.width, self.height) else {
            return false;
        };
        (b.y0..b.y1).all(|row| {
            (row * self.width + b.x0..row * self.width + b.x1).all(|i| {
                self.loose(i) || self.creatures.swim[usize::from(self.cells[i].material.0)]
            })
        })
    }

    /// Whether `c` rests on loose substrate: the row under its body is loose
    /// under at least half of it and open water under the rest (a poured
    /// bed's top is ragged), with nothing else (rock, wood, a plant) there.
    fn on_loose_bed(&self, k: &Kind, c: &Creature) -> bool {
        super::body(k, c.turn, c.x, c.y, self.width, self.height).is_some_and(|b| {
            if b.y1 >= self.height {
                return false;
            }
            let mut loose = 0;
            for i in b.y1 * self.width + b.x0..b.y1 * self.width + b.x1 {
                if self.loose(i) {
                    loose += 1;
                } else if !self.creatures.swim[usize::from(self.cells[i].material.0)] {
                    return false;
                }
            }
            loose * 2 >= b.x1 - b.x0
        })
    }

    /// Whether the cell at `i` is loose substrate a burrower can dig into.
    fn loose(&self, i: usize) -> bool {
        self.creatures.loose[usize::from(self.cells[i].material.0)]
    }
}

/// Whether material `m` is loose substrate: a powder from the substrate
/// palette.
pub(super) fn is_loose(class: MovementClass, palette: Option<PalettePath>) -> bool {
    class == MovementClass::Powder && palette == Some(PalettePath::SUBSTRATE)
}

/// Ticks a burrower takes to dig its whole height in, or out.
fn dig_ticks(k: &Kind) -> u64 {
    u64::try_from((k.h * ONE + DIG_SPEED - 1) / DIG_SPEED).unwrap_or(0)
}

/// Where `c` is in its current window of `len` ticks in every `every`
/// (`None` outside one, or with `every` 0), and the window's hash.
fn window(
    seed: u64,
    tick: u64,
    c: &Creature,
    every: u64,
    len: u64,
    salt: u64,
) -> Option<(u64, u64)> {
    if every == 0 {
        return None;
    }
    let id = u64::from(c.id.0);
    let t = tick + id.wrapping_mul(salt) % every;
    let phase = t % every;
    (phase < len).then(|| {
        (
            phase,
            splitmix64(seed ^ splitmix64((t / every) ^ splitmix64(id ^ salt))),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Rect;
    use crate::material::MaterialTable;
    use crate::species::SpeciesId;
    use crate::{Brush, CreatureId, WorldConfig};

    const WIDTH: usize = 160;
    const HEIGHT: usize = 90;
    const BED: usize = 78;

    /// A tank with a bed of `bed` (over `under` from `split` rows down),
    /// in easy mode so nobody dies.
    fn tank(seed: u64, bed: &str, under: Option<(&str, usize)>) -> World {
        let mut w = World::new(
            WorldConfig {
                width: WIDTH,
                height: HEIGHT,
                seed,
                water_level: 0.95,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let fill = |w: &mut World, name: &str, y0: usize| {
            let m = w.materials().id(name).unwrap();
            let r = Rect {
                x0: 0,
                y0,
                x1: WIDTH,
                y1: HEIGHT,
            };
            w.fill_rect(r, m, Brush::Replace).unwrap();
        };
        fill(&mut w, bed, BED);
        if let Some((name, y0)) = under {
            fill(&mut w, name, y0);
        }
        w.set_easy_mode(true);
        w
    }

    fn species(w: &World, name: &str) -> SpeciesId {
        w.species().id(name).unwrap()
    }

    fn add(w: &mut World, name: &str, x: i64) -> CreatureId {
        let s = species(w, name);
        w.add_creature(s, x, i64::try_from(BED).unwrap() - 4)
            .unwrap()
    }

    fn on_surface(w: &World, id: CreatureId) -> bool {
        let c = w.creature(id).unwrap();
        let k = w.grown_kind(c).unwrap();
        w.on_surface(&k, c.turn, c.x, c.y, c.band())
    }

    /// Rows between the body's bottom edge and the bed's top.
    fn lift(w: &World, id: CreatureId) -> usize {
        BED.saturating_sub(w.creature_body(w.creature(id).unwrap()).y1)
    }

    /// Whether the `rows` rows under the body are all water.
    fn clear_below(w: &World, id: CreatureId, rows: usize) -> bool {
        let b = w.creature_body(w.creature(id).unwrap());
        (b.y1..(b.y1 + rows).min(HEIGHT)).all(|y| {
            (b.x0..b.x1).all(|x| w.creatures.swim[usize::from(w.cells[y * WIDTH + x].material.0)])
        })
    }

    #[test]
    fn snails_and_crabs_never_leave_the_faces_they_crawl() {
        let mut w = tank(1, "sand", None);
        let snails: Vec<_> = [
            "mystery_snail",
            "ramshorn_snail",
            "nerite",
            "thai_micro_crab",
        ]
        .into_iter()
        .zip([25, 60, 95, 130])
        .map(|(name, x)| {
            let k = w.kind(species(&w, name)).unwrap();
            assert_eq!((k.zone, k.swim_every), (Zone::Surfaces, 0), "{name}");
            add(&mut w, name, x)
        })
        .collect();
        // Let them settle onto the bed first.
        for _ in 0..TICKS_PER_SECOND * 10 {
            w.tick();
        }
        for _ in 0..TICKS_PER_SECOND * 300 {
            w.tick();
            for &id in &snails {
                assert!(
                    on_surface(&w, id),
                    "a snail or crab swam off at tick {}",
                    w.tick
                );
            }
        }
    }

    #[test]
    fn bottom_swimmers_swim_now_and_then_and_settle_again() {
        let mut w = tank(2, "sand", None);
        let swimmers: Vec<_> = [
            "bristlenose_pleco",
            "otocinclus",
            "cherry_shrimp",
            "corydoras",
            "kuhli_loach",
        ]
        .into_iter()
        .zip([20, 50, 80, 110, 140])
        .map(|(name, x)| (name, add(&mut w, name, x)))
        .collect();
        for _ in 0..TICKS_PER_SECOND * 10 {
            w.tick();
        }
        let ticks = TICKS_PER_SECOND * 600;
        let mut off = vec![0; swimmers.len()];
        let mut highest = vec![0; swimmers.len()];
        for _ in 0..ticks {
            w.tick();
            for (i, &(_, id)) in swimmers.iter().enumerate() {
                let crawler =
                    w.kind(w.creature(id).unwrap().species).unwrap().zone == Zone::Surfaces;
                // A crawler can climb the glass: count it off when it holds
                // nothing; a walker when nothing solid is under it (more
                // than a hop's height above the bed or a heap).
                let up = lift(&w, id);
                if (crawler && !on_surface(&w, id)) || (!crawler && clear_below(&w, id, 3)) {
                    off[i] += 1;
                }
                highest[i] = highest[i].max(up);
            }
        }
        for (i, &(name, _)) in swimmers.iter().enumerate() {
            assert!(
                off[i] > 0 && highest[i] >= 3,
                "{name} never swam: rose {}",
                highest[i]
            );
            // Mostly on the bottom.
            assert!(
                off[i] * 5 < ticks,
                "{name} swam {} ticks of {ticks}",
                off[i]
            );
        }
    }

    #[test]
    fn loach_swims_out_of_a_rock_pocket() {
        let mut w = tank(3, "rock", None);
        let rock = w.materials().id("rock").unwrap();
        for x0 in [65, 92] {
            w.fill_rect(
                Rect {
                    x0,
                    x1: x0 + 2,
                    y0: BED - 14,
                    y1: BED,
                },
                rock,
                Brush::Replace,
            )
            .unwrap();
        }
        let id = add(&mut w, "kuhli_loach", 80);
        for _ in 0..400 * TICKS_PER_SECOND {
            w.tick();
            let c = w.creature(id).unwrap();
            let b = w.creature_body(c);
            assert!(w.fits(&w.grown_kind(c).unwrap(), c.turn, c.x, c.y, c.band()));
            if b.x1 <= 65 || b.x0 >= 94 {
                return;
            }
        }
        panic!("loach stayed between the rocks");
    }

    /// Runs `w` for `seconds`, returning the deepest and the last sunk depth
    /// of `id`, in 1/256 cell.
    fn watch_burrow(w: &mut World, id: CreatureId, seconds: u32) -> (i32, i32) {
        let mut deepest = 0;
        for _ in 0..seconds * TICKS_PER_SECOND {
            w.tick();
            deepest = deepest.max(w.sunk_depth(w.creature(id).unwrap()));
        }
        (deepest, w.sunk_depth(w.creature(id).unwrap()))
    }

    #[test]
    fn loaches_burrow_into_loose_substrate_and_come_back_out() {
        for bed in ["sand", "gravel", "aquasoil"] {
            let mut w = tank(3, bed, None);
            let loach = add(&mut w, "kuhli_loach", 80);
            let k = *w.kind(species(&w, "kuhli_loach")).unwrap();
            let counts = w.material_counts();
            let (deepest, _) = watch_burrow(&mut w, loach, 400);
            assert_eq!(w.material_counts(), counts, "{bed}: substrate made or lost");
            assert!(deepest >= k.h * ONE / 2, "{bed}: dug {deepest}");
            assert!(deepest <= k.h * ONE, "{bed}: dug {deepest}");
            // Out again: wait for the end of a window, then until it is clear.
            let mut out = false;
            for _ in 0..BURROW_TICKS * 2 {
                w.tick();
                let c = w.creature(loach).unwrap();
                if w.sunk_depth(c) == 0 && w.covered(&w.grown_kind(c).unwrap(), c) == Some(false) {
                    out = true;
                    break;
                }
            }
            assert!(out, "{bed}: it stays in");
        }
    }

    #[test]
    fn loaches_burrow_into_a_ragged_bed() {
        // A poured bed's top is ragged: grains on every other column.
        let mut w = tank(3, "sand", None);
        let sand = w.materials().id("sand").unwrap();
        for x in (0..WIDTH).step_by(2) {
            w.set_cell(x, BED - 1, sand).unwrap();
        }
        let loach = add(&mut w, "kuhli_loach", 80);
        let k = *w.kind(species(&w, "kuhli_loach")).unwrap();
        assert!(
            watch_burrow(&mut w, loach, 400).0 >= k.h * ONE / 2,
            "never dug"
        );
    }

    /// Every cell's material, row-major.
    fn cells(w: &World) -> Vec<crate::MaterialId> {
        w.cells.iter().map(|c| c.material).collect()
    }

    #[test]
    fn loaches_never_dig_into_rock() {
        let mut w = tank(3, "rock", None);
        let loach = add(&mut w, "kuhli_loach", 80);
        let before = cells(&w);
        assert_eq!(watch_burrow(&mut w, loach, 400).0, 0, "dug into rock");
        assert!(cells(&w) == before, "the rock bed changed");
        // Two rows of sand over rock: it stops at the rock, which stays.
        let mut w = tank(3, "sand", Some(("rock", BED + 2)));
        let rock = w.materials().id("rock").unwrap();
        let loach = add(&mut w, "kuhli_loach", 80);
        let rocks = |w: &World| -> Vec<usize> {
            (0..w.cells.len())
                .filter(|&i| w.cells[i].material == rock)
                .collect()
        };
        let before = rocks(&w);
        let mut deepest = 0;
        for _ in 0..400 * TICKS_PER_SECOND {
            w.tick();
            let b = w.creature_body(w.creature(loach).unwrap());
            assert!(b.y1 <= BED + 2, "into the rock: {b:?}");
            deepest = deepest.max(w.sunk_depth(w.creature(loach).unwrap()));
        }
        assert!(deepest > 0, "never dug");
        assert_eq!(rocks(&w), before, "rock moved");
    }

    #[test]
    fn burrowing_moves_the_substrate_and_leaves_a_settled_bed() {
        let mut w = tank(5, "sand", None);
        let loach = add(&mut w, "kuhli_loach", 80);
        let sand = w.materials().id("sand").unwrap();
        // Settle the loach on the bed first.
        for _ in 0..TICKS_PER_SECOND * 5 {
            w.tick();
        }
        let counts = w.material_counts();
        let before = cells(&w);
        // Sand above the old bed's top: spoil heaped beside the burrow.
        let heaped = |w: &World| (0..BED * WIDTH).any(|i| w.cells[i].material == sand);
        let (mut dug, mut changed, mut heap, mut buried) = (false, false, false, false);
        for _ in 0..400 * TICKS_PER_SECOND {
            w.tick();
            // Moved, never made or lost.
            assert_eq!(w.material_counts(), counts, "at tick {}", w.tick);
            let c = w.creature(loach).unwrap();
            dug |= w.sunk_depth(c) >= ONE;
            changed |= cells(&w) != before;
            heap |= heaped(&w);
            // Drawn back over it: sand fills most of its body.
            if w.sunk_depth(c) > 0 {
                let b = w.creature_body(c);
                let sanded = (b.y0..b.y1)
                    .flat_map(|y| (b.x0..b.x1).map(move |x| y * WIDTH + x))
                    .filter(|&i| w.cells[i].material == sand)
                    .count();
                buried |= sanded * 2 > b.area();
            }
            if dug && w.sunk_depth(c) == 0 && w.covered(&w.grown_kind(c).unwrap(), c) == Some(false)
            {
                break;
            }
        }
        assert!(
            dug && changed && heap && buried,
            "it never dug in (dug {dug}, moved cells {changed}, heaped {heap}, buried {buried})"
        );
        // Out again, the bed settles: no sand left floating in the water,
        // and the loach is clear of it.
        for _ in 0..TICKS_PER_SECOND * 5 {
            w.tick();
        }
        for y in BED - SPOIL_REACH - 8..HEIGHT - 1 {
            for x in 0..WIDTH {
                if w.cells[y * WIDTH + x].material == sand {
                    let below = w.cells[(y + 1) * WIDTH + x].material;
                    assert!(
                        !w.creatures.swim[usize::from(below.0)],
                        "sand at ({x}, {y}) over water"
                    );
                }
            }
        }
    }

    #[test]
    fn swims_and_burrows_are_deterministic_and_survive_a_save() {
        let run = |save_at: Option<u32>| {
            let mut w = tank(4, "sand", None);
            add(&mut w, "kuhli_loach", 40);
            add(&mut w, "bristlenose_pleco", 100);
            for t in 0..TICKS_PER_SECOND * 120 {
                if save_at == Some(t) {
                    let bytes = crate::save::encode(&w);
                    w = crate::save::decode(&bytes, MaterialTable::builtin().unwrap()).unwrap();
                }
                w.tick();
            }
            w.state_hash()
        };
        let plain = run(None);
        assert_eq!(plain, run(None));
        // Mid-burrow and mid-swim saves carry on the same.
        for at in [TICKS_PER_SECOND * 30, TICKS_PER_SECOND * 75] {
            assert_eq!(plain, run(Some(at)), "saved at {at}");
        }
    }
}
