//! Whole plants placed in one click (#470): a juvenile or mature plant of
//! any species, written into the world as ordinary plant cells.
//!
//! [`World::plant_placement`] snaps a pointer cell to where the species can
//! stand (rooted plants on a substrate they root in, epiphytes and mats
//! against what they cling to, floaters at the water surface, marimo on
//! whatever is below), builds the plant's cells from the species' form the
//! way growth would shape them, and returns them as a [`PlantGhost`] for the
//! host to preview. [`World::place_plant`] writes the same cells and makes
//! the plant, with the same table, part counts, bounds and (for floaters and
//! balls) raft as a grown one, so growth, splitting, saves and the rest need
//! nothing special.
//!
//! The shape is a pure function of the species, stage, snapped base and
//! flip (and the world seed): flipping mirrors it left to right. Parts that
//! don't fit (rock, glass, other plants, the wrong substrate) are left out;
//! a spot where the plant can't stand at all still gets an ideal silhouette
//! (clipped to the grid) with `fits` false, so the host can draw it red.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::many_single_char_names,
    clippy::too_many_lines
)] // cell maths on small signed offsets, one generator per form

use std::collections::BTreeMap;
use std::sync::Arc;

use super::PlantId;
use crate::material::MaterialId;
use crate::plants::{
    BallForm, EpiphyteForm, FloaterForm, Form, MAX_POSITION, MatForm, Part, PlantSpecies,
    PlantSpeciesId, PlantStage, RosetteForm, StemForm, plant_variant,
};
use crate::rng::splitmix64;
use crate::world::{World, WorldError};

/// Share of its mature size a juvenile plant has.
const JUVENILE: f64 = 0.38;
/// Share of its mature size a mature plant has: a little short of the size
/// where growth stops, so a placed plant still grows a bit.
const MATURE: f64 = 0.95;
/// Most rows a pointer inside the substrate climbs to find the open cell
/// above it.
const CLIMB: i64 = 6;
/// Rows around the configured top water row a floater looks for the
/// surface in.
const SURFACE_REACH: usize = 3;
/// Most cells an ideal (red) mat ghost shows.
const IDEAL_MAT: usize = 24;

/// One cell of a [`PlantGhost`]: where, which part and its variant byte
/// (colour noise above the part-local position), exactly as the placed
/// plant's cell would be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlantGhostCell {
    /// Column.
    pub x: usize,
    /// Row.
    pub y: usize,
    /// The plant part.
    pub part: Part,
    /// The cell's variant byte ([`crate::plants::plant_variant`]).
    pub variant: u8,
}

/// The whole plant [`World::plant_placement`] would put down.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlantGhost {
    /// The species.
    pub species: PlantSpeciesId,
    /// How grown it is.
    pub stage: PlantStage,
    /// Its base cell (the plant's `base`, also its first cell): the first
    /// stem of a stem plant, a rosette's or carpet's first crown cell, a
    /// floater's first floating leaf, an epiphyte's or mat's first cell
    /// and a ball's centre.
    pub base: (usize, usize),
    /// Its cells, each after the cell it grows from.
    pub cells: Vec<PlantGhostCell>,
    /// Whether it can be placed here: the species can stand at the
    /// snapped spot with its base in place and the tank isn't at its plant
    /// cap. If not, the cells are the ideal shape, clipped to the grid.
    pub fits: bool,
}

/// A cell of a plant being built.
#[derive(Clone, Copy, Debug)]
struct Placed {
    x: usize,
    y: usize,
    part: Part,
    position: u8,
}

/// The plant being built: its cells so far and what the world allows.
struct Builder<'w> {
    w: &'w World,
    /// Only open, suitable cells count (a real spot); otherwise the ideal
    /// silhouette, clipped to the grid.
    strict: bool,
    /// −1 if the plant is flipped: every sideways step is mirrored.
    mir: i64,
    /// What it hashes its choices from: its species, row and the column it
    /// mirrors about, so a flipped plant is the mirror image.
    salt: u64,
    /// Where its roots go (empty: hanging in water, a floater's).
    substrates: &'w [MaterialId],
    cells: Vec<Placed>,
    taken: BTreeMap<usize, usize>,
    /// Its base is in place (strict builds only).
    core: bool,
}

impl<'w> Builder<'w> {
    fn new(w: &'w World, s: &'w PlantSpecies, key: u64, flip: bool, strict: bool) -> Self {
        Self {
            w,
            strict,
            mir: if flip { -1 } else { 1 },
            salt: splitmix64((u64::from(s.id.0) << 56) ^ key),
            substrates: s.form.substrates().unwrap_or(&[]),
            cells: Vec::new(),
            taken: BTreeMap::new(),
            core: true,
        }
    }

    fn index(&self, x: i64, y: i64) -> Option<usize> {
        (x >= 0 && y >= 0 && (x as usize) < self.w.width && (y as usize) < self.w.height)
            .then(|| y as usize * self.w.width + x as usize)
    }

    fn material(&self, x: i64, y: i64) -> Option<MaterialId> {
        self.index(x, y).map(|i| self.w.cells[i].material)
    }

    fn is_water(&self, x: i64, y: i64) -> bool {
        self.material(x, y)
            .is_some_and(|m| Some(m) == self.w.plants.water)
    }

    /// Open air (or another fluid that isn't water).
    fn air_like(&self, x: i64, y: i64) -> bool {
        self.material(x, y)
            .is_some_and(|m| Some(m) != self.w.plants.water && self.w.is_fluid(m))
    }

    /// Whether the cell at `(x, y)` is substrate the plant roots in (any
    /// cell, for an ideal build).
    fn substrate(&self, x: i64, y: i64) -> bool {
        self.index(x, y).is_some()
            && (!self.strict
                || self
                    .material(x, y)
                    .is_some_and(|m| self.substrates.contains(&m)))
    }

    /// Water with air above it (any cell, for an ideal build).
    fn surface(&self, x: i64, y: i64) -> bool {
        self.index(x, y).is_some()
            && (!self.strict || (self.is_water(x, y) && self.air_like(x, y - 1)))
    }

    /// Whether `part` can go at `(x, y)`: inside the grid, free, of a
    /// material the tank has and (for a real spot) where the part belongs.
    fn fit(&self, x: i64, y: i64, part: Part) -> bool {
        let Some(i) = self.index(x, y) else {
            return false;
        };
        if self.taken.contains_key(&i) || self.w.plants.material(part).is_none() {
            return false;
        }
        if !self.strict {
            return true;
        }
        match part {
            Part::Root if !self.substrates.is_empty() => self.substrate(x, y),
            _ => self.is_water(x, y),
        }
    }

    fn put(&mut self, x: i64, y: i64, part: Part, position: i64) -> bool {
        if !self.fit(x, y, part) {
            return false;
        }
        let Some(i) = self.index(x, y) else {
            return false;
        };
        self.taken.insert(i, self.cells.len());
        self.cells.push(Placed {
            x: x as usize,
            y: y as usize,
            part,
            position: position.clamp(0, i64::from(MAX_POSITION)) as u8,
        });
        true
    }

    /// Whether a cell of `part` is already at `(x, y)`.
    fn own(&self, x: i64, y: i64, part: Part) -> bool {
        self.index(x, y)
            .and_then(|i| self.taken.get(&i))
            .is_some_and(|&k| self.cells[k].part == part)
    }

    /// Whether a `part` cell at `(x, y)` would touch no other cell of
    /// `part` than the one at `from`.
    fn lone(&self, x: i64, y: i64, part: Part, from: (i64, i64)) -> bool {
        [(-1, 0), (1, 0), (0, -1), (0, 1)]
            .into_iter()
            .all(|(dx, dy)| (x + dx, y + dy) == from || !self.own(x + dx, y + dy, part))
    }

    /// A number in `0..1` from the plant's salt and `key`.
    fn unit(&self, key: u64) -> f64 {
        (splitmix64(self.w.seed ^ splitmix64(self.salt ^ key)) >> 40) as f64 / (1u64 << 24) as f64
    }

    /// Roots down from the first root (already at `(x, y)`) `depth` cells,
    /// with short side roots every other row, alternating sides.
    fn roots(&mut self, x: i64, y: i64, depth: i64) {
        for k in 2..=depth {
            if !self.put(x, y + k - 1, Part::Root, k) {
                break;
            }
        }
        let mut side = 1;
        for k in (2..depth).step_by(2) {
            let (px, py) = (x, y + k - 1);
            if !self.own(px, py, Part::Root) {
                break;
            }
            side = -side;
            let dx = side * self.mir;
            let reach = (1 + (depth - k) / 4).min(3);
            let mut from = (px, py);
            for step in 1..=reach {
                let (nx, ny) = (px + dx * step, py);
                if !self.lone(nx, ny, Part::Root, from) || !self.put(nx, ny, Part::Root, k + step) {
                    break;
                }
                from = (nx, ny);
            }
        }
    }
}

/// `n · frac`, rounded, at least 1.
fn scaled(n: i64, frac: f64) -> i64 {
    ((n as f64 * frac).round() as i64).max(1)
}

impl World {
    /// The whole plant of `species` and `stage` that placing it at
    /// `(x, y)` would put down, with `flip` mirroring it left to right (see
    /// [`PlantGhost`]). The pointer snaps to where the species can stand:
    /// a rooted plant's base to the nearest open water above a substrate it
    /// roots in; an epiphyte or mat to open water touching what it clings
    /// to; a floater to the surface in that column; a marimo to rest on what
    /// is below it. Where it can't, the ghost sits at the pointer with
    /// `fits` false. `None` for a species the world lacks.
    #[must_use]
    pub fn plant_placement(
        &self,
        species: PlantSpeciesId,
        stage: PlantStage,
        x: i64,
        y: i64,
        flip: bool,
    ) -> Option<PlantGhost> {
        let table = Arc::clone(&self.plants.species);
        let s = table.get(species)?;
        let (px, py) = (
            x.clamp(0, self.width as i64 - 1),
            y.clamp(0, self.height as i64 - 1),
        );
        let (ax, ay) = match &s.form {
            Form::Rosette(f) | Form::Carpet(f) => (
                (px - crown_shift(f, flip)).clamp(0, self.width as i64 - 1),
                py,
            ),
            _ => (px, py),
        };
        let snapped = self.plant_anchor(s, ax, ay);
        let cap_ok = self.plants.table.len() < self.plants.cap.min(usize::from(u16::MAX));
        let mut ghost = snapped.and_then(|(sx, sy)| {
            let (cells, core) = self.build_plant(s, stage, (sx, sy), (px, flip), true);
            (core && !cells.is_empty()).then_some((cells, cap_ok))
        });
        if ghost.is_none() {
            // Nowhere it can stand: the ideal shape at the pointer.
            let base = match &s.form {
                Form::Floater(_) => (ax, self.surface_row as i64),
                _ => (ax, ay),
            };
            let (cells, _) = self.build_plant(s, stage, base, (px, flip), false);
            ghost = Some((cells, false));
        }
        let (cells, fits) = ghost?;
        let cells: Vec<PlantGhostCell> = cells
            .iter()
            .map(|c| PlantGhostCell {
                x: c.x,
                y: c.y,
                part: c.part,
                variant: plant_variant(
                    self.ghost_noise(species, c.y * self.width + c.x),
                    c.position,
                ),
            })
            .collect();
        let base = cells
            .first()
            .map_or((px as usize, py as usize), |c| (c.x, c.y));
        Some(PlantGhost {
            species,
            stage,
            base,
            cells,
            fits,
        })
    }

    /// Places the whole plant [`World::plant_placement`] shows at
    /// `(x, y)`: a new plant of `species` whose cells are those of the
    /// ghost, as a grown plant's are (a floater or ball gets its raft).
    /// Returns it, or `None` if it doesn't fit there or the tank is at its
    /// plant cap.
    ///
    /// # Errors
    ///
    /// Returns an error if the species is unknown.
    pub fn place_plant(
        &mut self,
        species: PlantSpeciesId,
        stage: PlantStage,
        x: i64,
        y: i64,
        flip: bool,
    ) -> Result<Option<PlantId>, WorldError> {
        let ghost = self
            .plant_placement(species, stage, x, y, flip)
            .ok_or(WorldError::UnknownPlantSpecies(species))?;
        if !ghost.fits {
            return Ok(None);
        }
        let base = ghost.base.1 * self.width + ghost.base.0;
        let id = self.new_plant(species, base, None);
        if id == 0 {
            return Ok(None);
        }
        for c in &ghost.cells {
            self.put_variant(c.y * self.width + c.x, c.part, id, c.variant);
        }
        Ok(Some(PlantId(id)))
    }

    /// Colour noise for a ghost cell: from the seed, species and cell, not
    /// the tick, so a ghost doesn't flicker as the pointer rests.
    fn ghost_noise(&self, species: PlantSpeciesId, i: usize) -> u8 {
        splitmix64(self.seed ^ splitmix64((u64::from(species.0) << 48) ^ i as u64)).to_le_bytes()[0]
    }

    /// Where the base of a plant of `s` goes for a pointer at `(x, y)`, if
    /// it can stand near it.
    fn plant_anchor(&self, s: &PlantSpecies, x: i64, y: i64) -> Option<(i64, i64)> {
        let (cx, cy) = (x as usize, y as usize);
        match &s.form {
            Form::Stem(_) | Form::Rosette(_) | Form::Carpet(_) => {
                self.rooted_anchor(s.form.substrates()?, cx, cy)
            }
            Form::Epiphyte(EpiphyteForm { on, .. }) | Form::Mat(MatForm { on, .. }) => {
                self.cling_anchor(on, cx, cy)
            }
            Form::Floater(_) => self.surface_anchor(cx),
            Form::Ball(_) => self.rest_anchor(cx, cy),
        }
        .map(|(ax, ay)| (ax as i64, ay as i64))
    }

    fn cell_material(&self, x: usize, y: usize) -> MaterialId {
        self.cells[y * self.width + x].material
    }

    fn is_water_at(&self, x: usize, y: usize) -> bool {
        Some(self.cell_material(x, y)) == self.plants.water
    }

    /// Whether the pointer may pass through the cell on its way down: any
    /// fluid, or a plant (to point at a bed behind it).
    fn passable(&self, x: usize, y: usize) -> bool {
        let m = self.cell_material(x, y);
        self.is_fluid(m) || self.plants.part(m).is_some()
    }

    /// The nearest open water cell above a substrate cell of `substrates`,
    /// down the column of `cx` from `cy` (a pointer inside the substrate
    /// climbs out of it first).
    fn rooted_anchor(
        &self,
        substrates: &[MaterialId],
        cx: usize,
        cy: usize,
    ) -> Option<(usize, usize)> {
        let sub = |y: usize| substrates.contains(&self.cell_material(cx, y));
        let mut y = cy;
        let mut climbed = 0;
        while y > 0 && sub(y) && climbed < CLIMB {
            y -= 1;
            climbed += 1;
        }
        while y < self.height {
            if self.is_water_at(cx, y) && y + 1 < self.height && sub(y + 1) {
                return Some((cx, y));
            }
            if !self.passable(cx, y) {
                return None;
            }
            y += 1;
        }
        None
    }

    /// An open water cell touching a cell of `on`: at the pointer or its
    /// four neighbours, else the first one down the pointer's column.
    fn cling_anchor(&self, on: &[MaterialId], cx: usize, cy: usize) -> Option<(usize, usize)> {
        let touches = |x: usize, y: usize| {
            self.is_water_at(x, y)
                && self
                    .at(x, y, 0, -1)
                    .into_iter()
                    .chain(self.at(x, y, 0, 1))
                    .chain(self.at(x, y, -1, 0))
                    .chain(self.at(x, y, 1, 0))
                    .any(|j| on.contains(&self.cells[j].material))
        };
        // A pointer inside what it clings to climbs out of it first.
        let mut cy = cy;
        for _ in 0..CLIMB {
            if cy > 0 && on.contains(&self.cell_material(cx, cy)) {
                cy -= 1;
            }
        }
        for (dx, dy) in [(0, 0), (0, -1), (-1, 0), (1, 0), (0, 1)] {
            if let Some(j) = self.at(cx, cy, dx, dy) {
                let (x, y) = (j % self.width, j / self.width);
                if touches(x, y) {
                    return Some((x, y));
                }
            }
        }
        for y in cy..self.height {
            if touches(cx, y) {
                return Some((cx, y));
            }
            if !self.passable(cx, y) {
                return None;
            }
        }
        None
    }

    /// The top water cell of column `cx` (water under air), near the
    /// configured surface row.
    fn surface_anchor(&self, cx: usize) -> Option<(usize, usize)> {
        let top = self.surface_row.saturating_sub(SURFACE_REACH);
        let bottom = (self.surface_row + SURFACE_REACH + 1).min(self.height);
        for y in top..bottom {
            if self.is_water_at(cx, y) {
                let air = y > 0
                    && self.at(cx, y, 0, -1).is_some_and(|a| {
                        let m = self.cells[a].material;
                        Some(m) != self.plants.water && self.is_fluid(m)
                    });
                return air.then_some((cx, y));
            }
            if !self.is_fluid(self.cell_material(cx, y)) {
                return None;
            }
        }
        None
    }

    /// The lowest open water cell down column `cx` from `cy`: the one
    /// resting on something that isn't a fluid.
    fn rest_anchor(&self, cx: usize, cy: usize) -> Option<(usize, usize)> {
        for y in cy..self.height {
            if y + 1 < self.height
                && self.is_water_at(cx, y)
                && !self.is_fluid(self.cell_material(cx, y + 1))
            {
                return Some((cx, y));
            }
            if !self.is_fluid(self.cell_material(cx, y)) {
                return None;
            }
        }
        None
    }

    /// The cells of a plant of `s` at `stage` with its anchor at `anchor`
    /// (see [`World::plant_placement`]) for a pointer in column `pivot`, and
    /// whether its base is in place.
    /// A real (`strict`) build that doesn't fit within the species' mature
    /// size shrinks until it does.
    fn build_plant(
        &self,
        s: &PlantSpecies,
        stage: PlantStage,
        anchor: (i64, i64),
        (pivot, flip): (i64, bool),
        strict: bool,
    ) -> (Vec<Placed>, bool) {
        let key = ((pivot as u64) << 20) ^ anchor.1 as u64;
        let cap = s.growth.size.max(1) as usize;
        let mut frac = match stage {
            PlantStage::Juvenile => JUVENILE,
            PlantStage::Mature => MATURE,
        };
        let mature = stage == PlantStage::Mature;
        let mut last = None;
        for _ in 0..24 {
            let mut b = Builder::new(self, s, key, flip, strict);
            match &s.form {
                Form::Stem(f) => gen_stem(&mut b, f, frac, anchor),
                Form::Rosette(f) | Form::Carpet(f) => gen_rosette(&mut b, f, frac, mature, anchor),
                Form::Floater(f) => gen_floater(&mut b, f, frac, anchor),
                Form::Epiphyte(f) => gen_epiphyte(&mut b, f, frac, anchor),
                Form::Mat(f) => gen_mat(&mut b, f, s.growth.size, frac, anchor),
                Form::Ball(f) => gen_ball(&mut b, f, frac, anchor),
            }
            let done = b.cells.len() <= cap;
            last = Some(b);
            if done {
                break;
            }
            frac *= 0.92;
        }
        let mut b = last.unwrap_or_else(|| Builder::new(self, s, key, flip, strict));
        b.cells.truncate(cap);
        (b.cells, b.core)
    }
}

/// How far left of the pointer a rosette's or carpet's first crown cell
/// is, so the crown is centred on it (a flip centres it the other way for
/// an even crown).
fn crown_shift(f: &RosetteForm, flip: bool) -> i64 {
    let crown = i64::from(f.crown.max(1));
    if flip { crown / 2 } else { (crown - 1) / 2 }
}

fn gen_stem(b: &mut Builder<'_>, f: &StemForm, frac: f64, (bx, by): (i64, i64)) {
    let h = scaled(i64::from(f.height), frac);
    if !b.put(bx, by, Part::Stem, 1) {
        b.core = false;
        return;
    }
    if !b.put(bx, by + 1, Part::Root, 1) {
        b.core = false;
    }
    // The upright stem, trailing sideways along the surface where it tops
    // out.
    let mut nodes = vec![(bx, by, 1i64)];
    let (mut x, mut y, mut pos) = (bx, by, 1i64);
    while pos < h {
        if b.fit(x, y - 1, Part::Stem) {
            y -= 1;
            pos += 1;
            b.put(x, y, Part::Stem, pos);
            nodes.push((x, y, pos));
        } else if b.air_like(x, y - 1) {
            let dx = b.mir;
            while pos < h
                && b.fit(x + dx, y, Part::Stem)
                && b.air_like(x + dx, y - 1)
                && b.lone(x + dx, y, Part::Stem, (x, y))
            {
                x += dx;
                pos += 1;
                b.put(x, y, Part::Stem, pos);
            }
            break;
        } else {
            break;
        }
    }
    let node = |pos: i64| pos >= i64::from(f.internode) && pos % i64::from(f.internode) == 0;
    // Side shoots at some nodes: a stub that steps out two cells, then
    // rises.
    if f.branching > 0.0 {
        let column = nodes.clone();
        for &(x, y, pos) in &column {
            if !node(pos) || !b.own(x, y - 1, Part::Stem) || pos + 3 >= h {
                continue;
            }
            for rel in [-1i64, 1] {
                let dx = rel * b.mir;
                let key = ((pos as u64) << 2) | (rel + 1) as u64;
                if b.unit(key) >= f64::from(f.branching)
                    || !b.fit(x + dx, y, Part::Stem)
                    || !b.lone(x + dx, y, Part::Stem, (x, y))
                    || !b.fit(x + 2 * dx, y, Part::Stem)
                    || !b.fit(x + 2 * dx, y - 1, Part::Stem)
                    || b.own(x + 2 * dx, y + 1, Part::Stem)
                    || b.own(x + 3 * dx, y, Part::Stem)
                {
                    continue;
                }
                b.put(x + dx, y, Part::Stem, pos + 1);
                b.put(x + 2 * dx, y, Part::Stem, pos + 2);
                let top = pos + 2 + (h - pos - 2) * 3 / 5;
                let (bxp, mut byp, mut bp) = (x + 2 * dx, y, pos + 2);
                while bp < top && b.fit(bxp, byp - 1, Part::Stem) {
                    byp -= 1;
                    bp += 1;
                    b.put(bxp, byp, Part::Stem, bp);
                    nodes.push((bxp, byp, bp));
                }
            }
        }
    }
    // Pairs of leaves at the nodes.
    let leaf = scaled(i64::from(f.leaf_length), frac);
    let all = nodes.clone();
    for (x, y, pos) in all {
        if !node(pos) {
            continue;
        }
        for rel in [-1i64, 1] {
            let dx = rel * b.mir;
            for k in 1..=leaf {
                let (lx, ly) = (x + dx * k, y);
                if b.own(lx, ly + 1, Part::Stem) || !b.put(lx, ly, Part::Leaf, k) {
                    break;
                }
            }
        }
    }
    b.roots(bx, by + 1, scaled(i64::from(f.root_depth), frac));
}

fn gen_rosette(
    b: &mut Builder<'_>,
    f: &RosetteForm,
    frac: f64,
    mature: bool,
    (bx, by): (i64, i64),
) {
    let crown = i64::from(f.crown.max(1));
    if !b.put(bx, by, Part::Stem, 0) {
        b.core = false;
        return;
    }
    if !b.put(bx, by + 1, Part::Root, 1) {
        b.core = false;
    }
    // The crown, rightwards over the substrate.
    let mut x1 = bx;
    for k in 1..crown {
        if b.substrate(bx + k, by + 1) && b.put(bx + k, by, Part::Stem, 0) {
            x1 = bx + k;
        } else {
            break;
        }
    }
    let centre2 = 2 * bx + crown - 1;
    let side = |x: i64| -> (i64, i64) {
        let off = 2 * x - centre2;
        (off.signum(), off.abs() / 2)
    };
    // Where blades start: above each crown cell and beside its ends,
    // those nearest the middle first.
    let mut starts: Vec<(i64, i64)> = (bx..=x1).map(|x| (x, by - 1)).collect();
    starts.push((bx - 1, by));
    starts.push((x1 + 1, by));
    let mir = b.mir;
    starts.sort_by_key(|&(x, y)| ((2 * x - centre2).abs(), i64::from(y == by), mir * x));
    // The petiole of the end the plant is mirrored from is the longest.
    let first = if b.mir < 0 { x1 } else { bx };
    let blades = scaled(i64::from(f.blades), frac).min(i64::from(f.blades)) as usize;
    let long = i64::from(f.blade_length);
    let mut tips = Vec::new();
    for &(sx, sy) in starts.iter().take(blades) {
        if !b.put(sx, sy, Part::Leaf, 1) {
            continue;
        }
        let rel = ((2 * sx - centre2) * b.mir + 64) as u64;
        let (mut x, mut y, mut pos) = (sx, sy, 1i64);
        let target = if f.pad > 0 {
            scaled(petiole_limit(b, first, long, sx), frac).min(petiole_limit(b, first, long, sx))
        } else {
            scaled(long, frac * (0.8 + 0.2 * b.unit(0x2000 | rel))).min(long)
        };
        while pos < target {
            let (dir, out) = if f.pad > 0 { side(sx) } else { side(x) };
            let lean = if f.pad > 0 {
                (i64::from(f.lean) / (out + 1)).max(3)
            } else {
                (i64::from(f.lean) - out).max(3)
            };
            let step_out = f.lean > 0 && dir != 0 && pos >= 2 && (pos - 2) % lean == 0;
            let (dx, dy) = if step_out { (dir, 0) } else { (0, -1) };
            if !b.put(x + dx, y + dy, Part::Leaf, pos + 1) {
                break;
            }
            x += dx;
            y += dy;
            pos += 1;
        }
        if f.pad > 0 && (mature || pos < target) {
            tips.push((x, y));
        }
    }
    // A leaf pad spread from each petiole's tip, floating at the surface.
    let pad = i64::from(f.pad);
    for (x, y) in tips {
        let upper = |b: &mut Builder<'_>, x: i64, y: i64| {
            // The cell above a submerged pad cell, unless that is itself at
            // the surface.
            if !(b.strict && b.air_like(x, y - 2)) {
                b.put(x, y - 1, Part::Leaf, i64::from(f.pad_top()));
            }
        };
        upper(b, x, y);
        for dir in [-b.mir, b.mir] {
            for k in 1..=pad {
                let (px, py) = (x + dir * k, y);
                let part = if b.strict
                    && b.surface(px, py)
                    && b.w.plants.material(Part::Float).is_some()
                {
                    Part::Float
                } else {
                    Part::Leaf
                };
                if !b.put(px, py, part, long + k) {
                    break;
                }
                if k < pad {
                    upper(b, px, py);
                }
            }
        }
    }
    b.roots(bx, by + 1, scaled(i64::from(f.root_depth), frac));
}

/// The length of a tiger lotus type's petiole that rose from column `from`
/// of a crown whose first end is column `first`: the first one's the
/// longest, the others reach between 55% and all of it.
fn petiole_limit(b: &Builder<'_>, first: i64, long: i64, from: i64) -> i64 {
    if from == first {
        return long;
    }
    let u = b.unit(0x1000 | ((from - first) * b.mir + 64) as u64);
    ((long as f64 * (0.55 + 0.45 * u)) as i64).max(2)
}

fn gen_floater(b: &mut Builder<'_>, f: &FloaterForm, frac: f64, (bx, by): (i64, i64)) {
    let fronds = scaled(i64::from(f.fronds), frac);
    let strands = scaled(i64::from(f.roots), frac);
    let length = scaled(i64::from(f.root_length), frac);
    if !b.surface(bx, by) || !b.put(bx, by, Part::Float, 0) {
        b.core = false;
        return;
    }
    // Fronds side by side: 0, +1, −1, +2, −2 … from the base.
    let mut open = [true, true];
    let mut placed = vec![bx];
    let mut reach = 1;
    while (placed.len() as i64) < fronds && (open[0] || open[1]) {
        for (k, dir) in [b.mir, -b.mir].into_iter().enumerate() {
            if placed.len() as i64 >= fronds || !open[k] {
                continue;
            }
            let x = bx + dir * reach;
            if b.surface(x, by) && b.put(x, by, Part::Float, reach) {
                placed.push(x);
            } else {
                open[k] = false;
            }
        }
        reach += 1;
    }
    // Root strands hang from every other frond, nearest the base first.
    let mut count = 0;
    let mut columns = placed.clone();
    let mir = b.mir;
    columns.sort_by_key(|&x| ((x - bx).abs(), mir * x));
    for x in columns {
        if count >= strands {
            break;
        }
        if (x - bx).abs() % 2 != 0 {
            continue;
        }
        if b.put(x, by + 1, Part::Root, 1) {
            count += 1;
            for k in 2..=length {
                if !b.lone(x, by + k, Part::Root, (x, by + k - 1))
                    || !b.put(x, by + k, Part::Root, k)
                {
                    break;
                }
            }
        }
    }
}

fn gen_epiphyte(b: &mut Builder<'_>, f: &EpiphyteForm, frac: f64, (bx, by): (i64, i64)) {
    if !b.put(bx, by, Part::Rhizome, 1) {
        b.core = false;
        return;
    }
    // A rhizome creeping each way from the base along what it clings to.
    let total = scaled(i64::from(f.rhizome_length), frac).min(i64::from(f.rhizome_length));
    let arms = [total / 2, (total - 1) / 2];
    let cling = |b: &Builder<'_>, x: i64, y: i64| {
        b.fit(x, y, Part::Rhizome)
            && (!b.strict
                || [(0, -1), (0, 1), (-1, 0), (1, 0)]
                    .into_iter()
                    .any(|(dx, dy)| {
                        b.material(x + dx, y + dy)
                            .is_some_and(|m| f.on.contains(&m))
                    }))
    };
    let mut rhizome = vec![(bx, by, 1i64)];
    for (arm, dir) in arms.into_iter().zip([b.mir, -b.mir]) {
        let (mut x, mut y, mut pos) = (bx, by, 1i64);
        for _ in 0..arm {
            let step = [(dir, 0), (0, 1), (0, -1)].into_iter().find(|&(dx, dy)| {
                cling(b, x + dx, y + dy) && b.lone(x + dx, y + dy, Part::Rhizome, (x, y))
            });
            let Some((dx, dy)) = step else {
                break;
            };
            x += dx;
            y += dy;
            pos += 1;
            if !b.put(x, y, Part::Rhizome, pos) {
                break;
            }
            rhizome.push((x, y, pos));
        }
    }
    // Leaves rising from every `spacing`th cell, leaning away from the base.
    let length = scaled(i64::from(f.leaf_length), frac).min(i64::from(f.leaf_length));
    let lean = i64::from(f.lean).max(3);
    for (x0, y0, pos0) in rhizome {
        if (pos0 - 1) % i64::from(f.spacing.max(1)) != 0 || !b.put(x0, y0 - 1, Part::Leaf, 1) {
            continue;
        }
        let (mut x, mut y, mut pos) = (x0, y0 - 1, 1i64);
        while pos < length {
            let dir = (x - bx).signum();
            let step_out = f.lean > 0 && dir != 0 && pos >= 2 && (pos - 2) % lean == 0;
            let (dx, dy) = if step_out { (dir, 0) } else { (0, -1) };
            if !b.put(x + dx, y + dy, Part::Leaf, pos + 1) {
                break;
            }
            x += dx;
            y += dy;
            pos += 1;
        }
    }
}

fn gen_mat(b: &mut Builder<'_>, f: &MatForm, size: u32, frac: f64, (bx, by): (i64, i64)) {
    // A mat spreads over an area: its cells grow with the square of how
    // grown it is.
    let mut n = scaled(i64::from(size), frac * frac) as usize;
    if !b.strict {
        n = n.min(IDEAL_MAT);
    }
    if !b.put(bx, by, Part::Stem, 0) {
        b.core = false;
        return;
    }
    let dirs = [(b.mir, 0), (0, -1), (-b.mir, 0), (0, 1)];
    let touches = |b: &Builder<'_>, x: i64, y: i64| {
        !b.strict
            || dirs.iter().any(|&(dx, dy)| {
                b.material(x + dx, y + dy)
                    .is_some_and(|m| f.on.contains(&m))
            })
    };
    // The layer against what it clings to first, then the layers built up
    // off it.
    let flat = if f.layers > 1 {
        (n * 7).div_ceil(10)
    } else {
        n
    };
    let mut k = 0;
    while k < b.cells.len() && b.cells.len() < flat {
        let c = b.cells[k];
        k += 1;
        for (dx, dy) in dirs {
            let (x, y) = (c.x as i64 + dx, c.y as i64 + dy);
            if b.cells.len() < flat && touches(b, x, y) {
                b.put(x, y, Part::Stem, 0);
            }
        }
    }
    let mut k = 0;
    while k < b.cells.len() && b.cells.len() < n {
        let c = b.cells[k];
        k += 1;
        for (dx, dy) in dirs {
            let (x, y) = (c.x as i64 + dx, c.y as i64 + dy);
            if b.cells.len() >= n {
                break;
            }
            if touches(b, x, y) {
                b.put(x, y, Part::Stem, 0);
            } else if i64::from(c.position) + 1 < i64::from(f.layers) {
                b.put(x, y, Part::Stem, i64::from(c.position) + 1);
            }
        }
    }
}

fn gen_ball(b: &mut Builder<'_>, f: &BallForm, frac: f64, (ax, ay): (i64, i64)) {
    let reach = (f64::from(f.radius) + 0.5) * frac.sqrt();
    let rc = reach.floor() as i64;
    // The anchor is the cell the ball rests on; an ideal ball is centred
    // on the pointer.
    let (cx, cy) = if b.strict { (ax, ay - rc) } else { (ax, ay) };
    let mut disc: Vec<(i64, i64, i64)> = Vec::new();
    for dy in -rc - 1..=rc + 1 {
        for dx in -rc - 1..=rc + 1 {
            let d2 = dx * dx + dy * dy;
            if (d2 as f64) <= reach * reach {
                disc.push((d2, dy, dx * b.mir));
            }
        }
    }
    disc.sort_unstable();
    for (d2, dy, dx) in disc {
        let out = (d2 as f64).sqrt().round() as i64;
        let ok = b.put(cx + dx, cy + dy, Part::Stem, out.min(i64::from(f.radius)));
        if !ok && d2 == 0 {
            b.core = false;
            return;
        }
    }
}
