//! Rigid bodies (SPEC §2.1 *Rigid bodies*, D17): the cells of one rigid pour
//! move together as one object.
//!
//! A body is the set of rigid cells whose `state` holds its id. The world
//! keeps a table of bodies (id → material, cells, bounds, glued flag) in a
//! `BTreeMap`, so every pass over it runs in id order. The table can be
//! rebuilt from the grid alone: the id is in each cell's `state` and the
//! glued flag in the low bit of its variant ([`GLUED_VARIANT_BIT`]).
//!
//! Once per tick, before the movement pass, a body that isn't glued moves
//! one cell down if every cell directly under it is a fluid (liquid or
//! buoyant) lighter than it, or else one cell up if every cell directly
//! above it is a heavier fluid. The fluid it displaces moves into the cells
//! it vacates, so mass is conserved and the body keeps its shape. Powders,
//! floaters, other bodies, solids and the grid edges hold it.
//!
//! A body is only checked when something could have changed its support:
//! it moved, was created, split or unglued, or a chunk it overlaps has work
//! this tick. So a resting body costs nothing and its chunks can sleep.

use crate::grid::Rect;
use crate::material::{GLUED_VARIANT_BIT, MaterialId, MovementClass};

use super::World;

/// Ticks a body stays still after cells were last poured into it, so a
/// stroke can draw a shape before it drops (2, so a host that runs two
/// ticks before its next stamp still holds it).
const HOLD_TICKS: u64 = 2;

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

    /// Records that a cell of body `id` was overwritten.
    pub(super) fn body_removed(&mut self, id: u16) {
        if let Some(b) = self.bodies.get_mut(&id) {
            b.pending = Pending::Split;
        }
    }

    /// Per tick, before the movement pass: tidy the bodies edits touched,
    /// then move the ones that can.
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
        self.move_bodies();
    }

    /// A fresh, empty body; 0 if every id is taken.
    fn new_body(&mut self, material: MaterialId, anchored: bool) -> u16 {
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
        let (anchored, held_until) = (b.anchored, b.held_until);
        for piece in pieces {
            let new = self.new_body(material, anchored);
            for &i in &piece {
                self.cells[i].state = new;
            }
            if let Some(b) = self.bodies.get_mut(&new) {
                b.bounds = bounds(&piece, self.width);
                b.cells = piece;
                b.held_until = held_until;
            }
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

    /// Moves every body that can move one cell, lowest first so a stack
    /// falls together.
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
            let moved = [1, -1].into_iter().find(|&dy| self.can_move(id, dy));
            if let Some(dy) = moved {
                self.shift(id, dy);
            }
            if let Some(b) = self.bodies.get_mut(&id) {
                b.awake = moved.is_some();
            }
        }
    }

    /// Whether a chunk overlapping `r` (grown by a cell each way) has work
    /// this tick, i.e. something changed within reach of the body.
    fn work_touches(&self, r: Rect) -> bool {
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

    /// Whether body `id` can move one row along `dy` (+1 down, -1 up):
    /// every cell next to it that way is a fluid lighter (down) or heavier
    /// (up) than it.
    fn can_move(&self, id: u16, dy: isize) -> bool {
        let Some(b) = self.bodies.get(&id) else {
            return false;
        };
        let density = self.props[usize::from(b.material.0)].density;
        b.cells.iter().all(|&i| {
            let Some(j) = self.at(i % self.width, i / self.width, 0, dy) else {
                return false;
            };
            if self.is_member(j, id, b.material) {
                return true;
            }
            let p = self.props[usize::from(self.cells[j].material.0)];
            let way = if dy > 0 {
                p.density < density
            } else {
                p.density > density
            };
            p.class.is_fluid() && way
        })
    }

    /// Moves body `id` one row along `dy`. Each cell swaps with the one
    /// beyond it, leading edge first, so the fluid there passes through the
    /// body to the cells it vacates.
    fn shift(&mut self, id: u16, dy: isize) {
        let Some(b) = self.bodies.get_mut(&id) else {
            return;
        };
        let mut cells = core::mem::take(&mut b.cells);
        let w = self.width;
        if dy > 0 {
            for c in cells.iter_mut().rev() {
                self.swap(*c, *c + w);
                *c += w;
            }
        } else {
            for c in &mut cells {
                self.swap(*c, *c - w);
                *c -= w;
            }
        }
        if let Some(b) = self.bodies.get_mut(&id) {
            b.cells = cells;
            b.bounds.y0 = b.bounds.y0.saturating_add_signed(dy);
            b.bounds.y1 = b.bounds.y1.saturating_add_signed(dy);
        }
    }
}

fn info(id: u16, b: &Body) -> BodyInfo {
    BodyInfo {
        id: BodyId(id),
        material: b.material,
        bounds: b.bounds,
        cells: b.cells.len(),
        anchored: b.anchored,
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
