//! Rigid-body dynamics on the cell grid (SPEC §2.1 *Rigid bodies*, T4c):
//! gravity, buoyancy and drag, sub-stepped moves through fluids, impacts
//! that push grains aside, toppling about a contact, sliding, momentum
//! passed between bodies, and sleep. Integer maths only (see
//! [`super::shape`]).

use crate::grid::Cell;
use crate::material::{GLUED_VARIANT_BIT, MovementClass};

use super::super::World;
use super::super::hardscape::FULL_MASK;
use super::shape::{ONE, RASTER_STEP, TURN, isqrt, rotate, to_i64};

/// Gravity, in 1/256 cell per tick² (1/8 cell/tick²).
pub(in crate::world) const GRAVITY: i64 = 32;
/// Speed cap, so a sub-stepped move stays a few cells per tick.
pub(in crate::world) const MAX_SPEED: i64 = 4 * ONE;
/// Spin cap, in angle units per tick (1/64 turn).
const MAX_SPIN: i64 = TURN / 64;
/// Quadratic drag: the speed lost per tick is `v² · ρ_fluid / (DRAG_QUAD ·
/// ρ_body · ONE)`. With gravity this sets terminal speeds: rock (2.6) in
/// water about a cell per tick, driftwood (0.7) about 0.4.
const DRAG_QUAD: i64 = 5;
/// Linear drag, in 1/256 of the speed per tick at equal densities; damps
/// the bobbing of a floating body.
const DRAG_LIN: i64 = 8;
/// Spin lost per tick in a fluid, in 1/256 of the spin at equal densities.
const SPIN_DRAG: i64 = 24;
/// A body steps a cell once its position is this far from where it is
/// drawn (more than half a cell: hysteresis, so a body resting near a
/// cell boundary doesn't flicker between two rows).
const SNAP: i64 = ONE / 2 + ONE / 8;
/// Slowest impact that pushes grains aside (3/8 cell per tick): a body
/// set down gently rests on top.
const EMBED_SPEED: i64 = 96;
/// Energy to push one grain aside, per unit (milli) of the grain's
/// density, in the units of `n · ρ · v²`.
const EMBED_COST: i64 = 4 * ONE * ONE;
/// How far a pushed grain may go: rows up, and cells sideways.
const PUSH_UP: i64 = 12;
const PUSH_SIDE: i64 = 12;
/// Speed lost to friction per tick while resting on something, scaled by
/// the body's effective weight (1 at full gravity).
const FRICTION: i64 = 12;
/// Speed gained per tick sliding down a step that is free on one side.
const SLIDE: i64 = 20;
/// Below this speed (and with no spin) a body that didn't move counts as
/// resting.
pub(in crate::world) const REST_SPEED: i64 = 4;
/// Ticks at rest after which a body sleeps.
pub(in crate::world) const REST_TICKS: u16 = 10;
/// Angle units per radian (65536 / 2π).
const PER_RADIAN: i64 = 10_430;

/// What lies where a body would move.
#[derive(Default)]
struct Probe {
    /// A cell that can't be entered or pushed: the grid edge, a solid,
    /// another body.
    hard: bool,
    /// The first free (unglued) body in the way.
    hit: Option<u16>,
    /// Grains (powder or floating cells) in the way, by index.
    grains: Vec<usize>,
}

impl Probe {
    fn free(&self) -> bool {
        !self.hard && self.grains.is_empty()
    }
}

/// Where a resting body touches its support, if it would tip.
struct Tip {
    /// The contact it turns about, in position units.
    pivot: (i64, i64),
    /// Horizontal lever arm of its weight about the pivot.
    arm: i64,
}

impl World {
    /// One tick of motion for a free body. Returns whether it stays awake.
    #[allow(clippy::too_many_lines)] // Keep the force, motion and rest phases in tick order.
    pub(super) fn step_body(&mut self, id: u16) -> bool {
        let Some(b) = self.bodies.get(&id) else {
            return false;
        };
        let n = b.shape.n.max(1);
        let (mass, rho) = self.body_mass_and_density(id);
        let (fluid, mean_fluid, straddling) = self.displaced(id);
        // Net weight after buoyancy, and the gravity it gives.
        let weight = mass - fluid;
        let g = GRAVITY * weight / mass;
        let mut moved = false;

        // Forces.
        let Some(b) = self.bodies.get_mut(&id) else {
            return false;
        };
        // Damp surface bobbing when straddling a fluid boundary (e.g. air
        // over water) and displaced fluid nearly balances the load.
        if straddling && g.abs() < GRAVITY / 3 && mean_fluid > 0 {
            b.vel.1 /= 2;
            if g.abs() <= 3 {
                b.vel.1 = 0;
            } else {
                b.vel.1 += g;
            }
        } else {
            b.vel.1 += g;
        }
        for v in [&mut b.vel.0, &mut b.vel.1] {
            let quad = *v * v.abs() * mean_fluid / (DRAG_QUAD * rho * ONE);
            let lin = *v * mean_fluid * DRAG_LIN / (rho * 256);
            let d = quad + lin;
            *v -= if d.abs() > v.abs() { *v } else { d };
            *v = (*v).clamp(-MAX_SPEED, MAX_SPEED);
        }
        b.pos.0 += b.vel.0;
        b.pos.1 += b.vel.1;

        // Translation, a cell at a time: vertical, then horizontal.
        let mut grounded = false;
        let off = (b.pos.0 - b.drawn_com().0, b.pos.1 - b.drawn_com().1);
        let steps_y = steps(off.1);
        let steps_x = steps(off.0);
        for _ in 0..steps_y.abs() {
            let dy = steps_y.signum();
            if self.try_shift(id, 0, dy, mass) {
                moved = true;
            } else {
                grounded = dy > 0;
                break;
            }
        }
        for _ in 0..steps_x.abs() {
            if self.try_shift(id, steps_x.signum(), 0, mass) {
                moved = true;
            } else {
                break;
            }
        }
        // Resting on something even if it didn't try to move down.
        if weight > 0 && !grounded && steps_y == 0 {
            grounded = !self.probe_shift(id, 0, 1).free();
            if grounded && let Some(b) = self.bodies.get_mut(&id) {
                b.vel.1 = b.vel.1.min(0);
                b.pos.1 = b.pos.1.min(b.drawn_com().1);
            }
        }

        // Support: friction, sliding, or tipping about the contact.
        let mut pivot = None;
        if grounded && weight > 0 {
            let friction = FRICTION * g / GRAVITY;
            if let Some(b) = self.bodies.get_mut(&id) {
                b.vel.0 -= b.vel.0.signum() * friction.min(b.vel.0.abs());
            }
            if let Some(tip) = self.tip(id) {
                let Some(b) = self.bodies.get_mut(&id) else {
                    return false;
                };
                let (rx, ry) = (b.pos.0 - tip.pivot.0, b.pos.1 - tip.pivot.1);
                let inertia = b.shape.inertia + (n * (rx * rx + ry * ry));
                let alpha = g * tip.arm * n * PER_RADIAN / inertia.max(1);
                // Never too slow to get going.
                let alpha = if alpha == 0 { tip.arm.signum() } else { alpha };
                b.spin = (b.spin + alpha).clamp(-MAX_SPIN, MAX_SPIN);
                pivot = Some(tip.pivot);
            } else {
                let slide = SLIDE * g / GRAVITY;
                let (spin, vx) = self.bodies.get(&id).map_or((0, 0), |b| (b.spin, b.vel.0));
                if spin != 0 {
                    if let Some(b) = self.bodies.get_mut(&id) {
                        b.spin = 0;
                    }
                } else if vx == 0 {
                    let first = if (self.tick + u64::from(id)).is_multiple_of(2) {
                        1
                    } else {
                        -1
                    };
                    for dir in [first, -first] {
                        if self.probe_shift(id, dir, 0).free()
                            && self.probe_shift(id, dir, 1).free()
                        {
                            if let Some(b) = self.bodies.get_mut(&id) {
                                b.vel.0 += dir * slide;
                            }
                            break;
                        }
                    }
                }
            }
        } else if let Some(b) = self.bodies.get_mut(&id) {
            let d = b.spin * mean_fluid * SPIN_DRAG / (rho * 256);
            b.spin -= if d.abs() > b.spin.abs() { b.spin } else { d };
        }

        // Rotation, about the contact when tipping, else the centre of mass.
        if self.bodies.get(&id).is_some_and(|b| b.spin != 0) && self.turn(id, pivot) {
            moved = true;
        }

        let Some(b) = self.bodies.get_mut(&id) else {
            return false;
        };
        let slow = b.vel.0.abs() <= REST_SPEED && b.vel.1.abs() <= REST_SPEED && b.spin == 0;
        if !moved && slow {
            b.still = b.still.saturating_add(1);
        } else {
            b.still = 0;
        }
        if b.still >= REST_TICKS {
            b.vel = (0, 0);
            b.pos = b.drawn_com();
            return false;
        }
        true
    }

    /// Σ over the body's cells of the density (milli) of the fluid each
    /// displaces, the mean, and whether the body straddles fluids of
    /// different densities (e.g. air over water). Each row reads the fluid
    /// beside its two ends, blended with the row above or below by how far
    /// the body sits between cells, so buoyancy changes smoothly as a
    /// floating body crosses the surface. A row with no fluid beside it
    /// (buried) gets none.
    fn displaced(&self, id: u16) -> (i64, i64, bool) {
        let Some(body) = self.bodies.get(&id) else {
            return (0, 0, false);
        };
        let width = self.width;
        let frac = (body.pos.1 - body.drawn_com().1).clamp(-ONE, ONE);
        let towards = frac.signum();
        let mut total = 0;
        let mut min_fluid: Option<i64> = None;
        let mut max_fluid: Option<i64> = None;
        let mut row_start = 0;
        while row_start < body.cells.len() {
            let y = body.cells[row_start] / width;
            let mut end = row_start;
            while end + 1 < body.cells.len() && body.cells[end + 1] / width == y {
                end += 1;
            }
            let (x0, x1) = (body.cells[row_start] % width, body.cells[end] % width);
            let count = to_i64(end + 1 - row_start);
            let here = self.side_fluid(x0, x1, to_i64(y));
            let next = self.side_fluid(x0, x1, to_i64(y) + towards);
            for sample in [here, next].into_iter().flatten() {
                min_fluid = Some(min_fluid.map_or(sample, |m| m.min(sample)));
                max_fluid = Some(max_fluid.map_or(sample, |m| m.max(sample)));
            }
            let rho = match (here, next) {
                (Some(h), Some(nx)) => h + (nx - h) * frac.abs() / ONE,
                (Some(h), None) => h,
                (None, Some(nx)) => nx * frac.abs() / ONE,
                (None, None) => 0,
            };
            total += rho * count;
            row_start = end + 1;
        }
        let straddling = matches!((min_fluid, max_fluid), (Some(lo), Some(hi)) if lo != hi);
        (total, total / body.shape.n.max(1), straddling)
    }

    /// Mean density (milli) of the fluid cells just left of `x0` and right
    /// of `x1` on row `y`, if either is a fluid.
    fn side_fluid(&self, x0: usize, x1: usize, y: i64) -> Option<i64> {
        let y = usize::try_from(y).ok().filter(|&y| y < self.height)?;
        let fluid = |x: Option<usize>| {
            let x = x.filter(|&x| x < self.width)?;
            let p = self.props[usize::from(self.cells[y * self.width + x].material.0)];
            p.class.is_fluid().then_some(p.milli)
        };
        match (fluid(x0.checked_sub(1)), fluid(Some(x1 + 1))) {
            (Some(a), Some(b)) => Some(i64::midpoint(a, b)),
            (a, b) => a.or(b),
        }
    }

    /// What lies under the body's cells shifted by `(dx, dy)`.
    fn probe_shift(&self, id: u16, dx: i64, dy: i64) -> Probe {
        let Some(b) = self.bodies.get(&id) else {
            return Probe {
                hard: true,
                ..Probe::default()
            };
        };
        let w = self.width;
        self.probe(
            id,
            b.cells
                .iter()
                .map(|&i| (to_i64(i % w) + dx, to_i64(i / w) + dy)),
        )
    }

    /// What lies under world cells `cells`, ignoring the body's own.
    fn probe(&self, id: u16, cells: impl Iterator<Item = (i64, i64)>) -> Probe {
        let mut p = Probe::default();
        let Some(material) = self.bodies.get(&id).map(|b| b.material) else {
            p.hard = true;
            return p;
        };
        for (x, y) in cells {
            let Some(j) = self.index(x, y) else {
                p.hard = true;
                return p;
            };
            let c = self.cells[j];
            if c.material == material && c.state == id {
                continue;
            }
            match self.props[usize::from(c.material.0)].class {
                MovementClass::Liquid | MovementClass::Buoyant => {}
                MovementClass::Powder | MovementClass::Floating => p.grains.push(j),
                MovementClass::Rigid => {
                    p.hard = true;
                    if p.hit.is_none()
                        && self
                            .bodies
                            .get(&c.state)
                            .is_some_and(|o| !o.anchored && o.material == c.material)
                    {
                        p.hit = Some(c.state);
                    }
                }
                MovementClass::Solid => {
                    p.hard = true;
                }
            }
        }
        p
    }

    /// The grid index of `(x, y)`, if inside.
    fn index(&self, x: i64, y: i64) -> Option<usize> {
        let x = usize::try_from(x).ok().filter(|&x| x < self.width)?;
        let y = usize::try_from(y).ok().filter(|&y| y < self.height)?;
        Some(y * self.width + x)
    }

    /// Moves the body one cell along `(dx, dy)` if it can: into fluid, or
    /// (moving down fast enough) pushing grains aside, which costs it
    /// energy. Otherwise stops it on that axis, passing momentum to a body
    /// it hit, and returns false.
    fn try_shift(&mut self, id: u16, dx: i64, dy: i64, mass: i64) -> bool {
        let probe = self.probe_shift(id, dx, dy);
        if probe.free() || (dy > 0 && !probe.hard && self.embed(id, &probe.grains, mass)) {
            self.shift_cells(id, dx, dy);
            return true;
        }
        // Blocked: an inelastic hit on another free body shares the speed.
        let Some(b) = self.bodies.get(&id) else {
            return false;
        };
        let v = if dx == 0 { b.vel.1 } else { b.vel.0 };
        if let Some(other) = probe.hit
            && let Some(o) = self.bodies.get(&other)
        {
            let ov = if dx == 0 { o.vel.1 } else { o.vel.0 };
            let (om, _) = self.body_mass_and_density(other);
            let dir = if dx == 0 { dy } else { dx };
            if (v - ov) * dir > 0 {
                let dv = mass * (v - ov) / (mass + om);
                if let Some(o) = self.bodies.get_mut(&other) {
                    if dx == 0 {
                        o.vel.1 += dv;
                    } else {
                        o.vel.0 += dv;
                    }
                    o.awake = true;
                    o.still = 0;
                }
            }
        }
        if let Some(b) = self.bodies.get_mut(&id) {
            let com = b.drawn_com();
            if dx == 0 {
                b.vel.1 = 0;
                b.pos.1 = com.1;
            } else {
                b.vel.0 = 0;
                b.pos.0 = com.0;
            }
        }
        false
    }

    /// An impact into grains: if the body is falling fast enough and has
    /// the energy, moves each grain to the nearest fluid cell beside or
    /// above it (out of the body's way), slows the body by the energy that
    /// took, and returns true. Grains are moved, never deleted.
    fn embed(&mut self, id: u16, grains: &[usize], mass: i64) -> bool {
        let Some(b) = self.bodies.get(&id) else {
            return false;
        };
        let vy = b.vel.1;
        if vy < EMBED_SPEED {
            return false;
        }
        let cost: i64 = grains
            .iter()
            .map(|&g| self.props[usize::from(self.cells[g].material.0)].milli * EMBED_COST)
            .sum();
        let energy = mass * vy * vy;
        if energy < 2 * cost {
            return false;
        }
        // Where the body will be: its cells one row down.
        let w = self.width;
        let mut target: Vec<usize> = b.cells.iter().map(|&i| i + w).collect();
        target.sort_unstable();
        let cx = b.pos.0;
        let tick = self.tick;
        let mut moves = Vec::with_capacity(grains.len());
        for &g in grains {
            let (gx, gy) = (to_i64(g % w), to_i64(g / w));
            let away = match (gx * ONE + ONE / 2).cmp(&cx) {
                core::cmp::Ordering::Less => -1,
                core::cmp::Ordering::Greater => 1,
                core::cmp::Ordering::Equal => {
                    if tick.is_multiple_of(2) {
                        1
                    } else {
                        -1
                    }
                }
            };
            let Some(f) = self.push_target(id, gx, gy, away, &target, &moves) else {
                return false;
            };
            moves.push(f);
        }
        for (&g, &f) in grains.iter().zip(&moves) {
            self.swap(g, f);
        }
        let left = energy - 2 * cost;
        if let Some(b) = self.bodies.get_mut(&id) {
            b.vel.1 = isqrt(left / mass);
        }
        true
    }

    /// The nearest fluid cell for a grain at `(gx, gy)` pushed `away`: the
    /// same row or up to [`PUSH_UP`] rows above, at most [`PUSH_SIDE`]
    /// cells sideways (the away side first), outside the body's next
    /// cells `target` and not already taken.
    fn push_target(
        &self,
        id: u16,
        gx: i64,
        gy: i64,
        away: i64,
        target: &[usize],
        taken: &[usize],
    ) -> Option<usize> {
        let material = self.bodies.get(&id)?.material;
        for up in 0..=PUSH_UP {
            for k in 1..=PUSH_SIDE {
                for side in [away, -away] {
                    let Some(j) = self.index(gx + side * k, gy - up) else {
                        continue;
                    };
                    let c = self.cells[j];
                    if (c.material == material && c.state == id)
                        || !self.props[usize::from(c.material.0)].class.is_fluid()
                        || target.binary_search(&j).is_ok()
                        || taken.contains(&j)
                    {
                        continue;
                    }
                    return Some(j);
                }
            }
        }
        None
    }

    /// Moves the body's cells by `(dx, dy)` (already checked free).
    fn shift_cells(&mut self, id: u16, dx: i64, dy: i64) {
        let Some(b) = self.bodies.get(&id) else {
            return;
        };
        let w = self.width;
        let new: Vec<_> = b
            .cells
            .iter()
            .filter_map(|&i| {
                let j = self.index(to_i64(i % w) + dx, to_i64(i / w) + dy)?;
                let mask = self.masks.get(&i).copied().unwrap_or(FULL_MASK);
                Some((j, self.cells[i].variant, mask, self.algae.coat[i]))
            })
            .collect();
        if new.len() != b.cells.len() {
            return;
        }
        self.relocate(id, new);
        if let Some(b) = self.bodies.get_mut(&id) {
            b.anchor.0 += dx;
            b.anchor.1 += dy;
        }
    }

    /// If the resting body would tip, where and how: its weight acts at
    /// the centre of mass, and the contacts are its cells resting on
    /// something. Outside the span of contact centres (or over a single
    /// contact: a point can't hold a body up) it turns about the nearest
    /// contact.
    fn tip(&self, id: u16) -> Option<Tip> {
        let body = self.bodies.get(&id)?;
        let width = self.width;
        // (x, bottom y) of the leftmost and rightmost contacts.
        let mut lo: Option<(i64, i64)> = None;
        let mut hi: Option<(i64, i64)> = None;
        for &index in &body.cells {
            let (x, y) = (to_i64(index % width), to_i64(index / width));
            let below = self.index(x, y + 1);
            let rests = match below {
                None => true,
                Some(j) => {
                    let contact = self.cells[j];
                    let own = contact.material == body.material && contact.state == id;
                    !own && !self.props[usize::from(contact.material.0)].class.is_fluid()
                }
            };
            if rests {
                if lo.is_none_or(|(lx, ly)| x < lx || (x == lx && y > ly)) {
                    lo = Some((x, y));
                }
                if hi.is_none_or(|(hx, hy)| x > hx || (x == hx && y > hy)) {
                    hi = Some((x, y));
                }
            }
        }
        let (lo, hi) = (lo?, hi?);
        let cx = body.drawn_com().0;
        let (l, h) = (lo.0 * ONE + ONE / 2, hi.0 * ONE + ONE / 2);
        if l < cx && cx < h {
            return None;
        }
        // Past one end, or balanced on a point: the nearer end; a dead
        // centre tips by id parity.
        let right = if cx >= h && cx <= l {
            id.is_multiple_of(2)
        } else {
            cx >= h
        };
        let contact = if right { hi } else { lo };
        let pivot = (contact.0 * ONE + ONE / 2, (contact.1 + 1) * ONE);
        let mut arm = cx - pivot.0;
        if arm == 0 {
            if body.shape.n == 1 {
                return None;
            }
            arm = if right { ONE / 2 } else { -ONE / 2 };
        }
        Some(Tip { pivot, arm })
    }

    /// Turns the body by its spin about `pivot` (its centre of mass if
    /// none), redrawing it when the drawn angle changes. A redraw that
    /// would overlap something tries one and two cells higher (a corner
    /// rolling over its contact) before the turn is refused, which stops
    /// the spin. Returns whether cells moved.
    fn turn(&mut self, id: u16, pivot: Option<(i64, i64)>) -> bool {
        let Some(b) = self.bodies.get(&id) else {
            return false;
        };
        let spin = b.spin;
        let pivot = pivot.unwrap_or(b.pos);
        let (rx, ry) = rotate(b.pos.0 - pivot.0, b.pos.1 - pivot.1, spin);
        let pos = (pivot.0 + rx, pivot.1 + ry);
        let angle = (b.angle + spin).rem_euclid(TURN);
        let drawn = raster_angle(angle);
        if drawn == b.drawn {
            if let Some(b) = self.bodies.get_mut(&id) {
                b.angle = angle;
                b.pos = pos;
            }
            return false;
        }
        let anchor = (
            (pos.0 - b.shape.com.0 + ONE / 2).div_euclid(ONE),
            (pos.1 - b.shape.com.1 + ONE / 2).div_euclid(ONE),
        );
        // Driftwood cells update their wetness in place on the grid; sync
        // those variants into the reference shape before rasterising so a
        // turn keeps per-cell wetness.
        if Some(b.material) == self.driftwood {
            let (old_anchor, old_drawn, mat) = (b.anchor, b.drawn, b.material);
            let width = self.width;
            let height = self.height;
            let grid_cells = &self.cells;
            if let Some(b) = self.bodies.get_mut(&id) {
                b.shape.sync_variants(old_anchor, old_drawn, |wx, wy| {
                    let x = usize::try_from(wx).ok().filter(|&x| x < width)?;
                    let y = usize::try_from(wy).ok().filter(|&y| y < height)?;
                    let c = grid_cells[y * width + x];
                    (c.material == mat && c.state == id).then_some(c.variant & !GLUED_VARIANT_BIT)
                });
            }
        }
        let Some(b) = self.bodies.get(&id) else {
            return false;
        };
        let Some(cells) = b.shape.raster(anchor, drawn) else {
            return false;
        };
        for lift in 0..=2 {
            let probe = self.probe(id, cells.iter().map(|&(x, y, _, _)| (x, y - lift)));
            if !probe.free() {
                continue;
            }
            let coat = if self.algae.coated > 0 && b.cells.iter().any(|&i| self.algae.coat[i] != 0)
            {
                let before = b.shape.raster_cell_ids(
                    b.anchor,
                    b.drawn,
                    b.cells
                        .iter()
                        .map(|&i| (to_i64(i % self.width), to_i64(i / self.width))),
                );
                let after = b.shape.raster_cell_ids(
                    anchor,
                    drawn,
                    cells.iter().map(|&(x, y, _, _)| (x, y)),
                );
                let coat: std::collections::BTreeMap<_, _> = before
                    .into_iter()
                    .zip(&b.cells)
                    .filter_map(|(id, &i)| {
                        (self.algae.coat[i] != 0).then_some((id, self.algae.coat[i]))
                    })
                    .collect();
                after
                    .into_iter()
                    .map(|id| coat.get(&id).copied().unwrap_or(0))
                    .collect()
            } else {
                Vec::new()
            };
            let new = cells
                .iter()
                .enumerate()
                .filter_map(|(i, &(x, y, v, m))| {
                    Some((
                        self.index(x, y - lift)?,
                        v,
                        m,
                        coat.get(i).copied().unwrap_or(0),
                    ))
                })
                .collect();
            self.relocate(id, new);
            if let Some(b) = self.bodies.get_mut(&id) {
                b.anchor = (anchor.0, anchor.1 - lift);
                b.drawn = drawn;
                b.angle = angle;
                b.pos = (pos.0, pos.1 - lift * ONE);
            }
            return true;
        }
        if let Some(b) = self.bodies.get_mut(&id) {
            b.spin = 0;
        }
        false
    }

    /// Moves body `id` onto the cells `new` (index, variant, outline
    /// mask, coat), as many as it has. The fluid in cells it enters goes to the
    /// cells it leaves, paired top row first, so air stays above water:
    /// every cell is conserved.
    pub(super) fn relocate(&mut self, id: u16, new: Vec<(usize, u8, u16, u8)>) {
        let Some(b) = self.bodies.get_mut(&id) else {
            return;
        };
        let material = b.material;
        let old = core::mem::take(&mut b.cells);
        // The masks go with the cells: clear where the body was, then set
        // where it lands (marking cells whose outline alone changed).
        for &k in &old {
            self.masks.remove(&k);
        }
        for &(k, _, m, _) in &new {
            if m != self.masks.get(&k).copied().unwrap_or(FULL_MASK) {
                self.mark_changed(k % self.width, k / self.width);
            }
            self.set_mask(k, m);
        }
        let new = new
            .into_iter()
            .map(|(k, v, _, coat)| {
                let cell = Cell {
                    material,
                    variant: v & !GLUED_VARIANT_BIT,
                    state: id,
                };
                (k, cell, coat)
            })
            .collect();
        let cells = self.carry(&old, new);
        let bounds = super::bounds(&cells, self.width);
        if let Some(b) = self.bodies.get_mut(&id) {
            b.cells = cells;
            b.bounds = bounds;
        }
    }

    /// Moves an object whose cells are `old` (ascending indices) onto
    /// `new` (index, cell, coat), as many as it has: a rigid body, or a floater
    /// (T6c part 2). The fluid in cells it enters goes to the cells it
    /// leaves, paired top row first, so air stays above water: every cell
    /// is conserved. Returns its new cell indices, ascending.
    pub(in crate::world) fn carry(
        &mut self,
        old: &[usize],
        mut new: Vec<(usize, Cell, u8)>,
    ) -> Vec<usize> {
        new.sort_unstable_by_key(|c| c.0);
        let (mut entered, mut left) = (Vec::new(), Vec::new());
        let (mut i, mut j) = (0, 0);
        while i < old.len() || j < new.len() {
            match (old.get(i), new.get(j)) {
                (Some(&o), Some(&(nw, _, _))) if o == nw => {
                    i += 1;
                    j += 1;
                }
                (Some(&o), Some(&(nw, _, _))) if o < nw => {
                    left.push(o);
                    i += 1;
                }
                (Some(&o), None) => {
                    left.push(o);
                    i += 1;
                }
                (_, Some(&(nw, _, _))) => {
                    entered.push(nw);
                    j += 1;
                }
                (None, None) => break,
            }
        }
        let displaced: Vec<_> = entered.iter().map(|&e| self.cells[e]).collect();
        for &(k, cell, coat) in &new {
            self.write_coat(k, coat);
            if self.cells[k] != cell {
                self.cells[k] = cell;
                self.moved[k] = self.stamp;
                self.mark_changed(k % self.width, k / self.width);
            }
        }
        for (&k, &c) in left.iter().zip(&displaced) {
            self.write_coat(k, 0);
            self.cells[k] = c;
            self.moved[k] = self.stamp;
            self.mark_changed(k % self.width, k / self.width);
        }
        new.into_iter().map(|c| c.0).collect()
    }
}

/// Whole cells to step for an offset `off` between where a body is and
/// where it is drawn (0 inside the [`SNAP`] dead band).
pub(in crate::world) fn steps(off: i64) -> i64 {
    if off.abs() <= SNAP {
        0
    } else {
        (off + off.signum() * ONE / 2) / ONE
    }
}

/// The drawn angle for `angle`: the nearest raster step.
fn raster_angle(angle: i64) -> i64 {
    ((angle + RASTER_STEP / 2).div_euclid(RASTER_STEP) * RASTER_STEP).rem_euclid(TURN)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Brush, MaterialTable, Rect, WorldConfig};

    #[test]
    fn coat_follows_a_falling_rock_through_oblique_and_right_angle_rotations() {
        let mut w = World::new(
            WorldConfig {
                width: 64,
                height: 64,
                water_level: 1.0,
                ..WorldConfig::default()
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let rock = w.materials().id("rock").unwrap();
        w.fill_rect(
            Rect {
                x0: 28,
                y0: 16,
                x1: 35,
                y1: 19,
            },
            rock,
            Brush::Replace,
        )
        .unwrap();
        let id = w.body_at(28, 16).unwrap().0;
        assert!(w.set_algae_coat(28, 16, 15));
        assert!(w.set_algae_coat(34, 18, 9));
        for _ in 0..8 {
            w.tick();
        }
        let b = &w.bodies[&id];
        let (x, y) = b.anchor;
        assert!(y > 16, "free rock fell");
        let at = |w: &World, x, y| w.coat()[w.index(x, y).unwrap()];
        assert_eq!(at(&w, x, y) & 0xf, 15);
        assert_eq!(at(&w, x + 6, y + 2) & 0xf, 9);
        for _ in 0..2 {
            w.bodies.get_mut(&id).unwrap().spin = TURN / 8;
            assert!(w.turn(id, None));
            assert_eq!(w.ecosystem().algae_cells, 2);
        }
        let (x, y) = w.bodies[&id].anchor;
        assert_eq!(at(&w, x + 4, y - 2) & 0xf, 15);
        assert_eq!(at(&w, x + 2, y + 4) & 0xf, 9);
    }
}
