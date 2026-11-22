//! Rigid bodies (SPEC §2.1 *Rigid bodies*, D17): the cells of one rigid pour
//! move together as one object.
//!
//! A body is the set of rigid cells whose `state` holds its id. The world
//! keeps a table of bodies (id → material, cells, bounds, glued flag) in a
//! `BTreeMap`, so every pass over it runs in id order. The table can be
//! rebuilt from the grid alone: the id is in each cell's `state` and the
//! glued flag in the low bit of its variant ([`GLUED_VARIANT_BIT`]).
//!
//! Once per tick, before the movement pass, each body that isn't glued or
//! held moves as a rigid object (T4c, [`motion`]): gravity, buoyancy and
//! drag set its velocity; it moves a cell at a time through fluids, pushes
//! grains aside when it lands hard, tips over about a contact when its
//! centre of mass isn't over its support, and slides off steps. It keeps a
//! reference shape ([`shape`]) and is redrawn from it whenever its drawn
//! angle changes, so it turns without losing or duplicating cells. The
//! fluid it displaces moves into the cells it vacates, so every cell is
//! conserved.
//!
//! A body is only stepped while awake: from when it is created, split,
//! unglued or hit, or a chunk it overlaps has work this tick, until it has
//! rested for a few ticks. So a resting body costs a bounds check against
//! its chunks per tick, never wakes them, and its chunks can sleep.

mod motion;
mod shape;

pub(in crate::world) use motion::{GRAVITY, MAX_SPEED, REST_SPEED, REST_TICKS, steps};
pub(in crate::world) use shape::ONE;

use crate::grid::Rect;
use crate::material::{
    GLUED_VARIANT_BIT, MAX_DRIFTWOOD_WETNESS, MaterialId, MovementClass, driftwood_variant,
    driftwood_wetness,
};

use shape::{Shape, TURN};

use super::World;
use super::persist::{rect_from_v1, rect_v1};
use crate::save::{SaveError, v1, v10};

/// Ticks a body stays still after cells were last poured into it, so a
/// stroke can draw a shape before it drops (2, so a host that runs two
/// ticks before its next stamp still holds it).
const HOLD_TICKS: u64 = 2;
/// Ticks between driftwood waterlogging steps (14 s of world time): a
/// typical floating piece crosses neutral buoyancy in ~19–21 minutes.
pub const DRIFTWOOD_SOAK_EVERY: u64 = 280;
/// Drying runs once every this many soak steps, so soaking beats drying
/// and a floating piece at the waterline reliably waterlogs and sinks.
pub const DRIFTWOOD_DRY_DIV: u64 = 2;
/// Wetness lost per cell above water when a driftwood cell touches air,
/// giving a natural capillary-wicking gradient above the waterline while
/// letting pieces completely in air dry back to 0.
pub const DRIFTWOOD_AIR_DROP: u8 = 12;
/// Effective density (milli) of fully waterlogged driftwood (1.20), dense
/// enough to sink gently without burying itself in substrate like rock (2.6).
const DRIFTWOOD_WET_MILLI: i64 = 1_200;
/// Liquids at least this dense count as water for soaking driftwood (same
/// threshold as `world::life::WATERY_DENSITY`).
const WATERY_DENSITY: f32 = 0.5;

/// A rigid body's id, as stored in its cells' `state` (never 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BodyId(pub u16);

/// A rigid body as the world reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyInfo {
    /// Its id.
    pub id: BodyId,
    /// Its material.
    pub material: MaterialId,
    /// The rectangle covering its cells.
    pub bounds: Rect,
    /// Number of cells.
    pub cells: usize,
    /// Glued: it never moves.
    pub anchored: bool,
    /// How far it has turned since its shape last changed, in 1/65536 turn
    /// clockwise (0..65536).
    pub angle: u16,
}

/// Tidying a body needs after edits; later variants include earlier ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Pending {
    None,
    /// Cells were added: sort the list.
    Sort,
    /// Cells were overwritten: drop them and split the rest into pieces.
    Split,
}

#[derive(Clone, Debug)]
pub(super) struct Body {
    material: MaterialId,
    /// Cell indices, ascending. Between ticks edits may append to it or
    /// leave entries for cells since overwritten; the next tick tidies it.
    cells: Vec<usize>,
    bounds: Rect,
    anchored: bool,
    /// What the next tick must do to the cell list.
    pending: Pending,
    /// Check it for motion this tick even if no chunk around it has work.
    awake: bool,
    /// Held still (being poured into) through this tick.
    held_until: u64,
    /// Its shape at angle 0 (rebuilt whenever its cells change by edits).
    shape: Shape,
    /// World cell where the shape's bitmap corner is drawn.
    anchor: (i64, i64),
    /// Angle the cells are drawn at, a multiple of the raster step.
    drawn: i64,
    /// Centre of mass in 1/256 cells; it may run ahead of the drawn cells
    /// by up to about half a cell.
    pos: (i64, i64),
    /// Velocity in 1/256 cell per tick.
    vel: (i64, i64),
    /// Angle (1/65536 turn) and spin (per tick), clockwise.
    angle: i64,
    spin: i64,
    /// Ticks it has been at rest.
    still: u16,
}

impl Body {
    /// Where its centre of mass is as drawn.
    fn drawn_com(&self) -> (i64, i64) {
        (
            self.anchor.0 * ONE + self.shape.com.0,
            self.anchor.1 * ONE + self.shape.com.1,
        )
    }
}

impl World {
    /// All rigid bodies, by id, as of the last tick (edits made since apply
    /// at the next tick).
    pub fn bodies(&self) -> impl Iterator<Item = BodyInfo> + '_ {
        self.bodies.iter().map(|(&id, b)| info(id, b))
    }

    /// The body with this id, if it exists.
    #[must_use]
    pub fn body(&self, id: BodyId) -> Option<BodyInfo> {
        self.bodies.get(&id.0).map(|b| info(id.0, b))
    }

    /// The body the cell at `(x, y)` belongs to, if any.
    #[must_use]
    pub fn body_at(&self, x: usize, y: usize) -> Option<BodyId> {
        let c = self.cell(x, y)?;
        let rigid = self.props[usize::from(c.material.0)].class == MovementClass::Rigid;
        (rigid && self.bodies.contains_key(&c.state)).then_some(BodyId(c.state))
    }

    /// The body's current effective density relative to water (1.0). For
    /// driftwood this is the average across its cells as they waterlog
    /// (0.7 dry to 1.2 fully waterlogged); for other materials it is the
    /// material's base density.
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // density in milli is small (<= 1_000_000)
    pub fn body_density(&self, id: BodyId) -> Option<f32> {
        self.bodies.get(&id.0)?;
        let (_, rho) = self.body_mass_and_density(id.0);
        Some(rho as f32 / 1000.0)
    }

    /// Total mass (in cell-milli) and mean density (in milli, >= 1) of body
    /// `id`. For driftwood, averages per-cell density across its cells.
    pub(super) fn body_mass_and_density(&self, id: u16) -> (i64, i64) {
        let Some(b) = self.bodies.get(&id) else {
            return (1, 1);
        };
        let base = self.props[usize::from(b.material.0)].milli.max(1);
        let n = b.shape.n.max(1);
        if Some(b.material) != self.driftwood || b.cells.is_empty() {
            return (n * base, base);
        }
        let span = DRIFTWOOD_WET_MILLI - base;
        let max_w = i64::from(MAX_DRIFTWOOD_WETNESS);
        let (mut sum_w, mut valid_count) = (0_i64, 0_i64);
        for &i in &b.cells {
            if self.is_member(i, id, b.material) {
                sum_w += i64::from(driftwood_wetness(self.cells[i].variant));
                valid_count += 1;
            }
        }
        if valid_count == 0 {
            return (n * base, base);
        }
        // Scale to `n` (equal to `valid_count` after `tidy`) and round to nearest milli.
        let denom = max_w * valid_count;
        let mass = (n * base + (sum_w * span * n + denom / 2) / denom).max(n);
        let rho = ((mass + n / 2) / n).max(1);
        (mass, rho)
    }

    /// Glues (`true`) or unglues a body. A glued body never moves. Returns
    /// whether the flag changed.
    pub fn set_anchored(&mut self, id: BodyId, anchored: bool) -> bool {
        let Some(b) = self.bodies.get_mut(&id.0) else {
            return false;
        };
        if b.anchored == anchored {
            return false;
        }
        b.anchored = anchored;
        b.awake = true;
        b.still = 0;
        b.vel = (0, 0);
        b.spin = 0;
        let (material, cells) = (b.material, b.cells.clone());
        for i in cells {
            if self.is_member(i, id.0, material) {
                let v = &mut self.cells[i].variant;
                *v = (*v & !GLUED_VARIANT_BIT) | if anchored { GLUED_VARIANT_BIT } else { 0 };
                self.mark_changed(i % self.width, i / self.width);
            }
        }
        true
    }

    /// The body a new rigid cell of `material` joins: `body` if it exists
    /// and is of that material, else a new one (recorded in `body`). Holds
    /// the body still for [`HOLD_TICKS`]. Returns 0 (no body: the cell is
    /// static) only if all 65 535 ids are in use.
    pub(super) fn join_body(&mut self, body: &mut Option<u16>, material: MaterialId) -> u16 {
        let id = match *body {
            Some(id) if self.bodies.get(&id).is_some_and(|b| b.material == material) => id,
            _ => {
                let id = self.new_body(material, false);
                *body = (id != 0).then_some(id);
                id
            }
        };
        self.hold(id);
        id
    }

    /// Keeps a body still through the next [`HOLD_TICKS`] ticks.
    pub(super) fn hold(&mut self, id: u16) {
        let until = self.tick + HOLD_TICKS;
        if let Some(b) = self.bodies.get_mut(&id) {
            b.held_until = until;
            b.awake = true;
            b.still = 0;
            b.vel = (0, 0);
            b.spin = 0;
        }
    }

    /// Records that cell `i` joined body `id`; returns whether the body is
    /// glued.
    pub(super) fn body_added(&mut self, id: u16, i: usize) -> bool {
        let (x, y) = (i % self.width, i / self.width);
        let Some(b) = self.bodies.get_mut(&id) else {
            return false;
        };
        b.cells.push(i);
        b.pending = b.pending.max(Pending::Sort);
        b.bounds = b.bounds.union(Rect {
            x0: x,
            y0: y,
            x1: x + 1,
            y1: y + 1,
        });
        b.anchored
    }

    /// Records that rigid cell `i`'s outline mask changed in place, so its
    /// body takes its cells as its shape again (a turn redraws from the
    /// shape and would otherwise restore the old outline).
    pub(super) fn outline_edited(&mut self, i: usize) {
        if let Some(b) = self.bodies.get_mut(&self.cells[i].state) {
            b.pending = b.pending.max(Pending::Sort);
        }
    }

    /// Records that a cell of body `id` was overwritten.
    pub(super) fn body_removed(&mut self, id: u16) {
        if let Some(b) = self.bodies.get_mut(&id) {
            b.pending = Pending::Split;
        }
    }

    /// Per tick, before the movement pass: tidy the bodies edits touched,
    /// advance driftwood waterlogging on soak ticks, then move the ones that
    /// can.
    pub(super) fn update_bodies(&mut self) {
        let dirty: Vec<u16> = self
            .bodies
            .iter()
            .filter(|(_, b)| b.pending != Pending::None)
            .map(|(&id, _)| id)
            .collect();
        for id in dirty {
            self.tidy(id);
        }
        self.soak_driftwood();
        self.move_bodies();
    }

    /// Advances per-cell driftwood waterlogging and air drying every
    /// [`DRIFTWOOD_SOAK_EVERY`] ticks. Synchronous across all driftwood cells
    /// so update order never biases direction; marks only changed cells
    /// visually dirty (without waking chunks) and wakes free bodies whose
    /// wetness changed so they settle deeper or sink once density passes 1.0.
    fn soak_driftwood(&mut self) {
        let Some(wood) = self.driftwood else {
            return;
        };
        if !self.tick.is_multiple_of(DRIFTWOOD_SOAK_EVERY) {
            return;
        }
        let allow_dry = (self.tick / DRIFTWOOD_SOAK_EVERY).is_multiple_of(DRIFTWOOD_DRY_DIV);
        let ids: Vec<u16> = self
            .bodies
            .iter()
            .filter(|(_, b)| b.material == wood)
            .map(|(&id, _)| id)
            .collect();
        if ids.is_empty() {
            return;
        }
        let width = self.width;
        let height = self.height;
        let mut updates: Vec<(usize, u8, u16)> = Vec::new();
        for &id in &ids {
            let Some(b) = self.bodies.get(&id) else {
                continue;
            };
            for &i in &b.cells {
                let cell = self.cells[i];
                if cell.material != wood {
                    continue;
                }
                let cur = driftwood_wetness(cell.variant);
                let x = i % width;
                let y = i / width;
                let mut touches_water = false;
                let mut touches_air = false;
                let mut has_wood = false;
                let mut max_wood = 0u8;
                let neighbours = [
                    (x > 0).then(|| i - 1),
                    (x + 1 < width).then(|| i + 1),
                    (y > 0).then(|| i - width),
                    (y + 1 < height).then(|| i + width),
                ];
                for j in neighbours.into_iter().flatten() {
                    let nb = self.cells[j];
                    if nb.material == wood {
                        has_wood = true;
                        max_wood = max_wood.max(driftwood_wetness(nb.variant));
                    } else {
                        let p = self.props[usize::from(nb.material.0)];
                        if p.class == MovementClass::Liquid {
                            if p.density >= WATERY_DENSITY {
                                touches_water = true;
                            } else {
                                touches_air = true;
                            }
                        }
                    }
                }
                let target = if touches_water {
                    MAX_DRIFTWOOD_WETNESS
                } else if touches_air {
                    if has_wood {
                        max_wood.saturating_sub(DRIFTWOOD_AIR_DROP)
                    } else {
                        0
                    }
                } else if has_wood {
                    max_wood.saturating_sub(1)
                } else {
                    cur
                };
                let next = if cur < target {
                    cur + 1
                } else if allow_dry && cur > target {
                    cur - 1
                } else {
                    cur
                };
                if next != cur {
                    updates.push((i, next, id));
                }
            }
        }
        let mut last_woken = 0u16;
        for (i, next, id) in updates {
            let glued = self.cells[i].variant & GLUED_VARIANT_BIT != 0;
            self.cells[i].variant = driftwood_variant(next, glued);
            self.mark_visual(i % width, i / width);
            if id != last_woken {
                last_woken = id;
                if let Some(b) = self.bodies.get_mut(&id)
                    && !b.anchored
                {
                    b.awake = true;
                    b.still = 0;
                }
            }
        }
    }

    /// Migrates a pre-v11 save: clears the random colour-noise bits of
    /// driftwood cells and shapes so old saves start with dry driftwood
    /// (preserving `GLUED_VARIANT_BIT`).
    pub(super) fn migrate_driftwood_v10(&mut self) {
        let Some(wood) = self.driftwood else {
            return;
        };
        for c in &mut self.cells {
            if c.material == wood {
                c.variant &= GLUED_VARIANT_BIT;
            }
        }
        for b in self.bodies.values_mut() {
            if b.material == wood {
                b.shape.reset_variants_to_dry();
            }
        }
    }

    /// A fresh, empty body; 0 if every id is taken.
    fn new_body(&mut self, material: MaterialId, anchored: bool) -> u16 {
        // Every id taken: fail at once rather than scanning per cell.
        if self.bodies.len() >= usize::from(u16::MAX) {
            return 0;
        }
        for _ in 0..u16::MAX {
            let id = self.next_body;
            self.next_body = self.next_body.checked_add(1).unwrap_or(1);
            if let std::collections::btree_map::Entry::Vacant(e) = self.bodies.entry(id) {
                e.insert(Body {
                    material,
                    cells: Vec::new(),
                    bounds: Rect::EMPTY,
                    anchored,
                    pending: Pending::None,
                    awake: true,
                    held_until: 0,
                    shape: Shape::default(),
                    anchor: (0, 0),
                    drawn: 0,
                    pos: (0, 0),
                    vel: (0, 0),
                    angle: 0,
                    spin: 0,
                    still: 0,
                });
                return id;
            }
        }
        0
    }

    /// Whether cell `i` belongs to body `id` of `material`.
    #[inline]
    fn is_member(&self, i: usize, id: u16, material: MaterialId) -> bool {
        let c = self.cells[i];
        c.material == material && c.state == id
    }

    /// Drops stale entries and sorts the cell list; if cells were removed,
    /// splits the body into its 4-connected pieces. The piece holding the
    /// lowest cell index keeps the id; the others get new ids, in order of
    /// their lowest cell, and inherit the glued and held state.
    fn tidy(&mut self, id: u16) {
        let Some(b) = self.bodies.get_mut(&id) else {
            return;
        };
        let (material, split) = (b.material, b.pending == Pending::Split);
        let mut cells = core::mem::take(&mut b.cells);
        cells.retain(|&i| self.is_member(i, id, material));
        cells.sort_unstable();
        cells.dedup();
        if cells.is_empty() {
            self.bodies.remove(&id);
            return;
        }
        let pieces = if split {
            self.pieces(&cells, id, material)
        } else {
            vec![cells]
        };
        let mut pieces = pieces.into_iter();
        let first = pieces.next().unwrap_or_default();
        let first_bounds = bounds(&first, self.width);
        let Some(b) = self.bodies.get_mut(&id) else {
            return;
        };
        b.bounds = first_bounds;
        b.cells = first;
        b.pending = Pending::None;
        b.awake = true;
        b.still = 0;
        let (anchored, held_until, vel) = (b.anchored, b.held_until, b.vel);
        self.reshape(id);
        for piece in pieces {
            let new = self.new_body(material, anchored);
            for &i in &piece {
                self.cells[i].state = new;
            }
            if let Some(b) = self.bodies.get_mut(&new) {
                b.bounds = bounds(&piece, self.width);
                b.cells = piece;
                b.held_until = held_until;
                b.vel = vel;
            }
            self.reshape(new);
        }
    }

    /// Takes the body's current cells as its shape at angle 0.
    fn reshape(&mut self, id: u16) {
        let Some(b) = self.bodies.get(&id) else {
            return;
        };
        let w = self.width;
        let cells: Vec<_> = b
            .cells
            .iter()
            .map(|&i| {
                let mask = self.masks.get(&i).copied().unwrap_or(super::FULL_MASK);
                (
                    i % w,
                    i / w,
                    self.cells[i].variant & !GLUED_VARIANT_BIT,
                    mask,
                )
            })
            .collect();
        let (shape, anchor) = Shape::new(&cells);
        if let Some(b) = self.bodies.get_mut(&id) {
            b.anchor = anchor;
            b.shape = shape;
            b.drawn = 0;
            b.angle = 0;
            b.pos = b.drawn_com();
        }
    }

    /// The 4-connected pieces of a body's (sorted, valid) cells, each
    /// sorted, in order of their lowest cell.
    fn pieces(&self, cells: &[usize], id: u16, material: MaterialId) -> Vec<Vec<usize>> {
        let r = bounds(cells, self.width);
        let bw = r.x1 - r.x0;
        let local = |i: usize| (i / self.width - r.y0) * bw + (i % self.width - r.x0);
        let mut seen = vec![false; r.area()];
        let mut pieces = Vec::new();
        let mut stack = Vec::new();
        for &start in cells {
            if seen[local(start)] {
                continue;
            }
            seen[local(start)] = true;
            stack.push(start);
            let mut piece = Vec::new();
            while let Some(i) = stack.pop() {
                piece.push(i);
                let (x, y) = (i % self.width, i / self.width);
                let neighbours = [
                    (x > r.x0).then(|| i - 1),
                    (x + 1 < r.x1).then(|| i + 1),
                    (y > r.y0).then(|| i - self.width),
                    (y + 1 < r.y1).then(|| i + self.width),
                ];
                for j in neighbours.into_iter().flatten() {
                    if !seen[local(j)] && self.is_member(j, id, material) {
                        seen[local(j)] = true;
                        stack.push(j);
                    }
                }
            }
            piece.sort_unstable();
            pieces.push(piece);
        }
        pieces
    }

    /// Steps every awake body, lowest first so a stack falls together.
    fn move_bodies(&mut self) {
        let tick = self.tick;
        let mut order = Vec::new();
        for (&id, b) in &self.bodies {
            if b.anchored || b.held_until >= tick {
                continue;
            }
            if b.awake || self.work_touches(b.bounds) {
                order.push((core::cmp::Reverse(b.bounds.y1), id));
            }
        }
        order.sort_unstable();
        for (_, id) in order {
            let awake = self.step_body(id);
            if let Some(b) = self.bodies.get_mut(&id) {
                b.awake = awake;
            }
        }
    }

    /// Whether a chunk overlapping `r` (grown by a cell each way) has work
    /// this tick, i.e. something changed within reach of the body.
    pub(in crate::world) fn work_touches(&self, r: Rect) -> bool {
        if r.is_empty() {
            return false;
        }
        let r = Rect {
            x0: r.x0.saturating_sub(1),
            y0: r.y0.saturating_sub(1),
            x1: (r.x1 + 1).min(self.width),
            y1: (r.y1 + 1).min(self.height),
        };
        let cs = super::CHUNK_SIZE;
        (r.y0 / cs..=(r.y1 - 1) / cs).any(|cy| {
            (r.x0 / cs..=(r.x1 - 1) / cs).any(|cx| {
                let w = self.chunks[cy * self.chunks_x + cx].work;
                !w.intersect(r).is_empty()
            })
        })
    }
}

impl World {
    /// The body table as saved (T8).
    pub(super) fn save_bodies(&self) -> Vec<v1::Body> {
        self.bodies
            .iter()
            .map(|(&id, b)| v1::Body {
                id,
                material: b.material.0,
                #[allow(clippy::cast_possible_truncation)] // cells < MAX_SIDE²
                cells: b.cells.iter().map(|&i| i as u32).collect(),
                bounds: rect_v1(b.bounds),
                anchored: b.anchored,
                pending: match b.pending {
                    Pending::None => 0,
                    Pending::Sort => 1,
                    Pending::Split => 2,
                },
                awake: b.awake,
                held_until: b.held_until,
                shape: b.shape.to_v1(),
                anchor: b.anchor,
                drawn: b.drawn,
                pos: b.pos,
                vel: b.vel,
                angle: b.angle,
                spin: b.spin,
                still: b.still,
            })
            .collect()
    }

    /// The reference-shape masks of bodies that have partial ones, as
    /// saved (version 10).
    pub(super) fn save_shape_masks(&self) -> Vec<v10::ShapeMasks> {
        self.bodies
            .iter()
            .filter(|(_, b)| !b.shape.masks().is_empty())
            .map(|(&body, b)| v10::ShapeMasks {
                body,
                masks: b.shape.masks().to_vec(),
            })
            .collect()
    }

    /// Restores bodies' reference-shape masks (after [`World::load_bodies`]).
    pub(super) fn load_shape_masks(
        &mut self,
        saved: Vec<v10::ShapeMasks>,
    ) -> Result<(), SaveError> {
        for s in saved {
            let Some(b) = self.bodies.get_mut(&s.body) else {
                return Err(SaveError::Invalid(format!(
                    "outline masks for missing body {}",
                    s.body
                )));
            };
            b.shape.set_masks(s.masks)?;
        }
        Ok(())
    }

    /// Restores the body table from a save.
    pub(super) fn load_bodies(&mut self, saved: Vec<v1::Body>) -> Result<(), SaveError> {
        let invalid = |what: &str, id: u16| SaveError::Invalid(format!("body {id}: {what}"));
        self.bodies.clear();
        for b in saved {
            let id = b.id;
            let material = MaterialId(b.material);
            if id == 0 || self.bodies.contains_key(&id) {
                return Err(invalid("id 0 or repeated", id));
            }
            if self.materials.get(material).is_none() {
                return Err(invalid("unknown material", id));
            }
            let pending = match b.pending {
                0 => Pending::None,
                1 => Pending::Sort,
                2 => Pending::Split,
                _ => return Err(invalid("unknown pending step", id)),
            };
            // Poses within a few grids of the tank and speeds far above
            // any reachable one, so the fixed-point maths can't overflow.
            let side = i64::try_from(4 * super::MAX_SIDE).unwrap_or(i64::MAX);
            let near =
                |(x, y): (i64, i64), unit: i64| x.abs() <= side * unit && y.abs() <= side * unit;
            let calm = b.vel.0.abs() <= 64 * ONE && b.vel.1.abs() <= 64 * ONE;
            if !(near(b.anchor, 1) && near(b.pos, ONE) && calm && b.spin.abs() <= TURN) {
                return Err(invalid("pose out of range", id));
            }
            let cells = b
                .cells
                .iter()
                .map(|&i| self.cell_index(i, "a body"))
                .collect::<Result<Vec<_>, _>>()?;
            self.bodies.insert(
                id,
                Body {
                    material,
                    cells,
                    bounds: rect_from_v1(b.bounds, self.width, self.height)?,
                    anchored: b.anchored,
                    pending,
                    awake: b.awake,
                    held_until: b.held_until,
                    shape: Shape::from_v1(b.shape)?,
                    anchor: b.anchor,
                    drawn: b.drawn,
                    pos: b.pos,
                    vel: b.vel,
                    angle: b.angle,
                    spin: b.spin,
                    still: b.still,
                },
            );
        }
        Ok(())
    }
}

fn info(id: u16, b: &Body) -> BodyInfo {
    BodyInfo {
        id: BodyId(id),
        material: b.material,
        bounds: b.bounds,
        cells: b.cells.len(),
        anchored: b.anchored,
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0..TURN
        angle: b.angle.rem_euclid(TURN) as u16,
    }
}

/// The rectangle covering cell indices `cells` (non-empty) in a grid
/// `width` cells wide.
fn bounds(cells: &[usize], width: usize) -> Rect {
    let mut r = Rect::EMPTY;
    for &i in cells {
        let (x, y) = (i % width, i / width);
        r = r.union(Rect {
            x0: x,
            y0: y,
            x1: x + 1,
            y1: y + 1,
        });
    }
    r
}

#[cfg(test)]
mod tests {
    use super::super::{Brush, FULL_MASK, World, WorldConfig};
    use crate::MaterialTable;

    /// An outline edited in place (a scoop's carve, a stroke's dab filling
    /// an edge) reaches the body's reference shape by the next tick, so a
    /// later turn, which redraws from the shape, keeps the edit.
    #[test]
    fn outline_edits_reach_the_shape() {
        let mut w = World::new(
            WorldConfig {
                width: 60,
                height: 40,
                seed: 3,
                water_level: 1.0,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let rock = w.materials().id("rock").unwrap();
        w.paint(30, 20, 6, rock, Brush::Replace).unwrap();
        let id = w.body_at(30, 20).unwrap().0;
        w.set_anchored(super::BodyId(id), true);
        w.tick();
        let shape = w.bodies[&id].shape.masks().to_vec();
        // Cut a hollow beside the stone, near enough to trim its outline
        // (`carve` only trims, so any centre is safe to try).
        let before = w.masks.clone();
        let carved = (31..44).any(|c| {
            w.carve(c, 50 - c, 3);
            w.masks != before
        });
        assert!(carved, "no carve trimmed the stone");
        assert!(w.masks.values().all(|&m| m != 0 && m != FULL_MASK));
        w.tick();
        let after = w.bodies[&id].shape.masks().to_vec();
        assert_ne!(after, shape, "the shape took the carved outline");
        // Taking the cells as the shape again changes nothing: in sync.
        w.reshape(id);
        assert_eq!(w.bodies[&id].shape.masks(), after.as_slice());
    }
}
