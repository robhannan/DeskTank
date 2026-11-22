//! Floaters as buoyant bodies (T6c part 2, D22): a floater plant's cells
//! move together as one object, as a rigid body's do (D17, T4c), so it
//! rests at the surface and drifts with the current.
//!
//! The plant table stays the floater's owner: its species, health, part
//! counts, base and bounds are the plant's, and growth, budding, cutting
//! and death work on its cells as for any plant. What makes it a body is
//! its [`Raft`], kept by plant id beside the table: its velocity and where
//! it is relative to where its cells are drawn, in 1/256 cell. Because the
//! raft holds only that sub-cell offset (not a reference shape or an
//! absolute position), cells growing onto it or cut from it need no
//! re-anchoring: its next step works on whatever cells the plant has.
//!
//! Each tick, before the movement pass (with the rigid bodies), every
//! awake raft is stepped:
//!
//! - **Buoyancy.** Each row of its cells reads the fluid beside it (past
//!   other plants, so a clump of floaters still finds the water), blended
//!   with the next row by how far the raft sits between cells, as a rigid
//!   body's rows do. Floating leaves weigh their material's density less
//!   that fluid, so a raft whose leaves are in the water row under the air
//!   rests with them a quarter cell proud (floating leaves are 0.75).
//!   Hanging roots are about as dense as water and take no part in water,
//!   but weigh their full density in air, so a raft lifted out falls back.
//!   A ball's (marimo's) cells weigh their density less the fluid's
//!   everywhere, so it sinks to rest on what is below it.
//! - **Current.** Its sideways speed is pulled towards the current's mean
//!   speed over its cells (T5), so it drifts as the floating cells the
//!   current carries do; with no current it slows to rest. A ball grips
//!   the current more weakly ([`BALL_GRIP`]) and rolls at only a quarter
//!   of its speed ([`BALL_DRIFT`]): it is nudged along the bottom.
//! - **Motion.** Speeds are damped in water and capped as a body's are; it
//!   moves a cell at a time into fluid (water, air, bubbles) or floating
//!   cells, which go to the cells it leaves, so every cell is conserved
//!   (`World::carry`). Grains, solids, bodies, other plants and the glass
//!   stop it. A raft pressed against something stops pushing, and after a
//!   few ticks at rest it sleeps, until a chunk around it has work, it
//!   grows or is cut, or the current changes.

use super::super::World;
use super::super::bodies::{GRAVITY, MAX_SPEED, ONE, REST_SPEED, REST_TICKS, steps};
use crate::grid::{Cell, Rect};
use crate::hash::Fnv64;
use crate::material::MovementClass;
use crate::plants::Part;
use crate::save::{SaveError, v7};

/// Speed a raft loses per tick in water, in 1/256 of its speed: a flat
/// leaf at the surface damps its bobbing within a few ticks (a rigid
/// body's linear drag is 8).
const DAMP: i64 = 64;
/// Share of the gap to the current's speed a raft makes up per tick in
/// water, in 1/256 (about 2 s to reach the current's speed).
const GRIP: i64 = 24;
/// A ball's grip on the current: a heavy ball on the bottom lags it (about
/// 6 s to reach its speed).
const BALL_GRIP: i64 = 8;
/// A ball rolls at most at this share of the current's speed, in 1/256.
const BALL_DRIFT: i64 = 64;
/// Most cells a raft looks along a row, past other plants, for the fluid
/// around it.
const REACH: usize = super::super::LEVEL_REACH;
/// Water's density in milli, if the table has no water.
const WATER_MILLI: i64 = 1000;
/// A current in cells per second as a raft speed: 1/256 cell per tick.
#[allow(clippy::cast_precision_loss)] // small exact constants
const PER_SECOND: f32 = ONE as f32 / super::super::TICKS_PER_SECOND as f32;

/// What acts on a raft this tick.
struct Forces {
    /// Its mass, milli · cells.
    mass: i64,
    /// Its weight less the fluid it displaces, milli · cells.
    weight: i64,
    /// Mean density of the fluid around it (milli; 0 where unknown).
    mean: i64,
    /// The 2D current's mean speed over its cells, 1/256 cell per tick.
    target: (i64, i64),
    /// Water's density (milli).
    water: i64,
}

/// A floater's motion: what makes the plant a buoyant body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::world) struct Raft {
    /// Where it is relative to where its cells are drawn, in 1/256 cell
    /// (+ = right, down); within about half a cell.
    off: (i64, i64),
    /// Velocity in 1/256 cell per tick.
    vel: (i64, i64),
    /// Ticks it has been at rest.
    still: u16,
    /// Stepped every tick, not only when something near it changed.
    awake: bool,
}

impl Raft {
    /// A raft at rest where its cells are, awake.
    pub(super) fn new() -> Self {
        Self {
            off: (0, 0),
            vel: (0, 0),
            still: 0,
            awake: true,
        }
    }

    /// A bud's or fragment's raft: moving as this one does.
    pub(super) fn child(self) -> Self {
        Self {
            still: 0,
            awake: true,
            ..self
        }
    }
}

impl World {
    /// Per tick, with the rigid bodies: steps every awake raft, and every
    /// sleeping one with work around it, in plant id order.
    pub(in crate::world) fn update_rafts(&mut self) {
        if self.plants.rafts.is_empty() {
            return;
        }
        let due: Vec<u16> = self
            .plants
            .rafts
            .iter()
            .filter(|&(id, r)| {
                r.awake
                    || self
                        .plants
                        .table
                        .get(id)
                        .is_some_and(|p| self.work_touches(p.bounds))
            })
            .map(|(&id, _)| id)
            .collect();
        for id in due {
            let awake = self.step_raft(id);
            if let Some(r) = self.plants.rafts.get_mut(&id) {
                r.awake = awake;
            }
        }
    }

    /// Wakes plant `id`'s raft, if it is a floater.
    pub(super) fn wake_raft(&mut self, id: u16) {
        if let Some(r) = self.plants.rafts.get_mut(&id) {
            r.awake = true;
            r.still = 0;
        }
    }

    /// Wakes every raft (the current changed).
    pub(in crate::world) fn wake_rafts(&mut self) {
        for r in self.plants.rafts.values_mut() {
            r.awake = true;
            r.still = 0;
        }
    }

    /// One tick of motion for floater `id` (see the module docs). Returns
    /// whether it stays awake.
    #[allow(clippy::many_single_char_names, clippy::too_many_lines)] // cell maths
    fn step_raft(&mut self, id: u16) -> bool {
        let Some(p) = self.plants.table.get(&id) else {
            return false;
        };
        let Some(&raft) = self.plants.rafts.get(&id) else {
            return false;
        };
        let ball = self
            .plants
            .species
            .get(p.species)
            .is_some_and(|s| s.form.ball().is_some());
        let mut members = self.plant_members(id, self.clip_to_grid(p.bounds));
        if members.is_empty() {
            return false;
        }
        let Forces {
            mass,
            weight,
            mean,
            target,
            water,
        } = self.raft_forces(&members, raft.off.1, ball);
        let mut raft = raft;

        // Forces: weight, buoyancy and the 2D current, each dropped while
        // the raft rests against something that way.
        let grip = if ball { BALL_GRIP } else { GRIP };
        // A ball on the bottom only rolls along at a share of it.
        let target = if ball {
            (target.0 * BALL_DRIFT / 256, target.1 * BALL_DRIFT / 256)
        } else {
            target
        };
        let g = GRAVITY * weight / mass.max(1);
        let dvy_cur = if target.1 != 0 {
            let pull_y = target.1 - raft.vel.1;
            pull_y - damp(pull_y, grip * mean, water * 256)
        } else {
            0
        };
        let fy = g + dvy_cur;
        if fy != 0 && raft.vel.1 == 0 && !self.raft_free(&members, id, 0, fy.signum()) {
            raft.off.1 = if fy > 0 {
                raft.off.1.min(0)
            } else {
                raft.off.1.max(0)
            };
        } else {
            raft.vel.1 += fy;
        }
        raft.vel.1 = damp(raft.vel.1, DAMP * mean, water * 256);
        let pull = target.0 - raft.vel.0;
        let dvx = pull - damp(pull, grip * mean, water * 256);
        if dvx != 0 && raft.vel.0 == 0 && !self.raft_free(&members, id, dvx.signum(), 0) {
            raft.off.0 = if dvx > 0 {
                raft.off.0.min(0)
            } else {
                raft.off.0.max(0)
            };
        } else {
            raft.vel.0 += dvx;
        }
        raft.vel.0 = raft.vel.0.clamp(-MAX_SPEED, MAX_SPEED);
        raft.vel.1 = raft.vel.1.clamp(-MAX_SPEED, MAX_SPEED);
        raft.off.0 += raft.vel.0;
        raft.off.1 += raft.vel.1;

        // Translation, a cell at a time: vertical, then horizontal.
        let mut moved = false;
        for axis in [1, 0] {
            let n = steps(if axis == 1 { raft.off.1 } else { raft.off.0 });
            for _ in 0..n.abs() {
                let d = n.signum();
                let (dx, dy) = if axis == 1 { (0, d) } else { (d, 0) };
                if self.shift_raft(id, &mut members, dx, dy) {
                    moved = true;
                    if axis == 1 {
                        raft.off.1 -= d * ONE;
                    } else {
                        raft.off.0 -= d * ONE;
                    }
                } else {
                    if axis == 1 {
                        raft.vel.1 = 0;
                        raft.off.1 = 0;
                    } else {
                        raft.vel.0 = 0;
                        raft.off.0 = 0;
                    }
                    break;
                }
            }
        }

        let drifting = (target.0 != 0 && self.raft_free(&members, id, target.0.signum(), 0))
            || (target.1 != 0 && self.raft_free(&members, id, 0, target.1.signum()));
        let slow = !drifting && raft.vel.0.abs() <= REST_SPEED && raft.vel.1.abs() <= REST_SPEED;
        raft.still = if !moved && slow {
            raft.still.saturating_add(1)
        } else {
            0
        };
        let awake = raft.still < REST_TICKS;
        if !awake {
            raft.vel = (0, 0);
        }
        if let Some(r) = self.plants.rafts.get_mut(&id) {
            *r = raft;
        }
        awake
    }

    /// What acts on a raft with cells `members` sitting `off_y` (1/256
    /// cell) from where it is drawn (see the module docs); a `ball`'s cells
    /// all sink.
    fn raft_forces(&self, members: &[usize], off_y: i64, ball: bool) -> Forces {
        let water = self
            .plants
            .water
            .map_or(WATER_MILLI, |m| self.props[usize::from(m.0)].milli)
            .max(1);
        let float = self.plants.material(Part::Float);
        let width = self.width;
        let frac = off_y.clamp(-ONE, ONE);
        let towards = frac.signum();
        // Mass and net weight (milli · cells), and the fluid around the
        // cells whose rows found one.
        let (mut mass, mut weight, mut fluid, mut known) = (0i64, 0i64, 0i64, 0i64);
        let (mut cur_x, mut cur_y) = (0.0f32, 0.0f32);
        let mut start = 0;
        while start < members.len() {
            let y = members[start] / width;
            let mut end = start;
            while end + 1 < members.len() && members[end + 1] / width == y {
                end += 1;
            }
            let row = &members[start..=end];
            let (x0, x1) = (row[0] % width, row[row.len() - 1] % width);
            let (mut leaves, mut rest, mut n_leaves, mut n_rest) = (0, 0, 0, 0);
            for &i in row {
                let m = self.cells[i].material;
                let milli = self.props[usize::from(m.0)].milli;
                if Some(m) == float {
                    leaves += milli;
                    n_leaves += 1;
                } else {
                    rest += milli;
                    n_rest += 1;
                }
                let mut cur = self.current_at_cell(i % width, y);
                if ball && cur == [0.0, 0.0] && y >= crate::field::FIELD_SCALE {
                    // A marimo resting on a bed whose 8x8 field block is mostly
                    // substrate still sits in open water just above it.
                    cur = self.current_at_cell(i % width, y - crate::field::FIELD_SCALE);
                }
                cur_x += cur[0];
                cur_y += cur[1];
            }
            mass += leaves + rest;
            let yi = i64::try_from(y).unwrap_or(i64::MAX);
            let here = self.open_fluid(x0, x1, yi);
            let next = if towards == 0 {
                here
            } else {
                self.open_fluid(x0, x1, yi + towards)
            };
            let rho = match (here, next) {
                (Some(h), Some(n)) => Some(h + (n - h) * frac.abs() / ONE),
                (h, _) => h,
            };
            // A row with no fluid in reach either way (wedged in a mat of
            // floaters) neither lifts nor sinks it: reading only the row it
            // leans towards would push it back and forth.
            if let Some(rho) = rho {
                weight += leaves - n_leaves * rho;
                weight += if ball {
                    rest - n_rest * rho
                } else {
                    (rest - n_rest * rho).max(0) * (water - rho).clamp(0, water) / water
                };
                fluid += rho * (n_leaves + n_rest);
                known += n_leaves + n_rest;
            }
            start = end + 1;
        }
        let mean = if known > 0 { fluid / known } else { 0 };
        #[allow(clippy::cast_precision_loss)] // members < MAX_SIDE²
        let n = members.len() as f32;
        // Cells per second to 1/256 cell per tick.
        #[allow(clippy::cast_possible_truncation)] // a few cells per tick
        let target = (
            (cur_x / n * PER_SECOND).round() as i64,
            (cur_y / n * PER_SECOND).round() as i64,
        );
        Forces {
            mass,
            weight,
            mean,
            target,
            water,
        }
    }

    /// Mean density (milli) of the first non-plant cells left of `x0` and
    /// right of `x1` on row `y`, within [`REACH`], where they are fluid.
    fn open_fluid(&self, x0: usize, x1: usize, y: i64) -> Option<i64> {
        let y = usize::try_from(y).ok().filter(|&y| y < self.height)?;
        let look = |from: Option<usize>, dir: isize| -> Option<i64> {
            let mut x = from?;
            for _ in 0..REACH {
                if x >= self.width {
                    return None;
                }
                let c = self.cells[y * self.width + x];
                if self.plants.part(c.material).is_none() {
                    let p = self.props[usize::from(c.material.0)];
                    return p.class.is_fluid().then_some(p.milli);
                }
                x = x.checked_add_signed(dir)?;
            }
            None
        };
        match (look(x0.checked_sub(1), -1), look(Some(x1 + 1), 1)) {
            (Some(a), Some(b)) => Some(i64::midpoint(a, b)),
            (a, b) => a.or(b),
        }
    }

    /// Whether floater `id`'s cells `members` could all move by `(dx, dy)`:
    /// into its own cells, fluid or floating cells, inside the grid.
    fn raft_free(&self, members: &[usize], id: u16, dx: i64, dy: i64) -> bool {
        let (dx, dy) = (unit(dx), unit(dy));
        members.iter().all(|&i| {
            let Some(j) = self.at(i % self.width, i / self.width, dx, dy) else {
                return false;
            };
            let c = self.cells[j];
            (c.state == id && self.plants.part(c.material).is_some())
                || matches!(
                    self.props[usize::from(c.material.0)].class,
                    MovementClass::Liquid | MovementClass::Buoyant | MovementClass::Floating
                )
        })
    }

    /// Moves floater `id`'s cells `members` one cell along `(dx, dy)` if it
    /// can, with its base and bounds; returns whether it did.
    fn shift_raft(&mut self, id: u16, members: &mut Vec<usize>, dx: i64, dy: i64) -> bool {
        if !self.raft_free(members, id, dx, dy) {
            return false;
        }
        let (sx, sy) = (unit(dx), unit(dy));
        let w = self.width;
        let shifted = |i: usize| {
            (i % w)
                .checked_add_signed(sx)
                .zip((i / w).checked_add_signed(sy))
                .map_or(i, |(x, y)| y * w + x)
        };
        let new: Vec<(usize, Cell)> = members
            .iter()
            .map(|&i| (shifted(i), self.cells[i]))
            .collect();
        *members = self.carry(members, new);
        if let Some(p) = self.plants.table.get_mut(&id) {
            p.base = shifted(p.base);
            let b = p.bounds;
            p.bounds = Rect {
                x0: b.x0.wrapping_add_signed(sx),
                y0: b.y0.wrapping_add_signed(sy),
                x1: b.x1.wrapping_add_signed(sx),
                y1: b.y1.wrapping_add_signed(sy),
            };
        }
        true
    }

    /// The rafts as saved (T8, format version 7).
    pub(in crate::world) fn save_rafts(&self) -> Vec<v7::Raft> {
        self.plants
            .rafts
            .iter()
            .map(|(&plant, r)| v7::Raft {
                plant,
                off: r.off,
                vel: r.vel,
                still: r.still,
                awake: r.awake,
            })
            .collect()
    }

    /// Restores the rafts from a save, after the plant table: each must
    /// belong to a floater or ball plant, and one the save has no raft for
    /// gets one at rest.
    pub(in crate::world) fn load_rafts(&mut self, saved: Vec<v7::Raft>) -> Result<(), SaveError> {
        let species = std::sync::Arc::clone(&self.plants.species);
        let plants = &mut self.plants;
        plants.rafts.clear();
        let floater = |table: &std::collections::BTreeMap<u16, super::Plant>, id: u16| {
            table
                .get(&id)
                .and_then(|p| species.get(p.species))
                .is_some_and(|s| s.form.drifts())
        };
        for r in saved {
            let invalid = |what: &str| SaveError::Invalid(format!("raft {}: {what}", r.plant));
            if !floater(&plants.table, r.plant) || plants.rafts.contains_key(&r.plant) {
                return Err(invalid("not a floater or ball plant, or repeated"));
            }
            let near = |(x, y): (i64, i64), limit: i64| x.abs() <= limit && y.abs() <= limit;
            if !(near(r.off, 4 * ONE) && near(r.vel, MAX_SPEED)) {
                return Err(invalid("pose out of range"));
            }
            plants.rafts.insert(
                r.plant,
                Raft {
                    off: r.off,
                    vel: r.vel,
                    still: r.still,
                    awake: r.awake,
                },
            );
        }
        let missing: Vec<u16> = plants
            .table
            .keys()
            .copied()
            .filter(|&id| floater(&plants.table, id) && !plants.rafts.contains_key(&id))
            .collect();
        for id in missing {
            plants.rafts.insert(id, Raft::new());
        }
        Ok(())
    }

    pub(in crate::world) fn hash_rafts(&self, h: &mut Fnv64) {
        for (&id, r) in &self.plants.rafts {
            h.write(&id.to_le_bytes());
            for v in [r.off.0, r.off.1, r.vel.0, r.vel.1] {
                h.write(&v.to_le_bytes());
            }
            h.write(&r.still.to_le_bytes());
            h.write(&[u8::from(r.awake)]);
        }
    }
}

/// `v` less `v · k / den`, at least one unit towards 0 while `k` > 0, so a
/// damped speed comes to rest.
fn damp(v: i64, k: i64, den: i64) -> i64 {
    if v == 0 || k <= 0 {
        return v;
    }
    let d = v * k / den.max(1);
    v - if d == 0 { v.signum() } else { d }
}

/// The sign of `d` as a cell step.
fn unit(d: i64) -> isize {
    match d.signum() {
        1 => 1,
        -1 => -1,
        _ => 0,
    }
}
