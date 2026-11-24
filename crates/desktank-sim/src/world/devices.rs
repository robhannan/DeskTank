//! The device table (`docs/spec/devices.md`, D27): placing prefab devices, removing them
//! whole, saving them and turning legacy filter outlet cells into internal
//! filters.
//!
//! Devices add no per-tick work of their own: the jet their nozzles drive
//! is rebuilt only when one is added or removed (`life.rs`), and nothing
//! else runs until filtering lands (DV3). The jet's current pass is the
//! outlet's as before: it scans its blocks each tick for floating and
//! liquid cells to carry.

use std::collections::{BTreeMap, BTreeSet};

use super::{LIFE_DAY_SECONDS, TICKS_PER_SECOND, World, WorldError};
use crate::device::{
    Device, DeviceId, DeviceKind, DeviceKinds, DeviceStatus, MAX_DEVICES, Mount, RinseKind,
};
use crate::hash::Fnv64;
use crate::material::{MaterialId, MaterialTable, MovementClass};
use crate::save::{SaveError, v12, v13};

/// The world's devices.
#[derive(Clone, Debug)]
pub(super) struct Devices {
    kinds: DeviceKinds,
    pub(super) table: BTreeMap<u16, Device>,
    /// Next id to try (ids are 1..=65535).
    next: u16,
    /// The material footprint cells are made of.
    material: Option<MaterialId>,
    /// Times the jet was rebuilt for a device change (tests).
    pub(super) rebuilds: u64,
    /// Intake-neighbour cell lookups made by [`World::run_intakes`] (tests).
    pub(super) intake_checks: u64,
    /// Cached bed-top row for each `Mount::Floor` intake `(device_id, ix)`,
    /// invalidated when cells change at or below `top - 1` in column `ix`.
    floor_intake_tops: BTreeMap<(u16, usize), usize>,
    /// The under-gravel filter each connected floor panel works for, by
    /// panel id; a panel not connected edge-to-edge to a filter is absent.
    /// Refreshed whenever the table changes.
    floor_owners: BTreeMap<u16, DeviceId>,
}

/// Closest a floor device may be dropped to another before it snaps flush to it.
const FLOOR_SNAP_CELLS: usize = 4;

impl Devices {
    pub(super) fn new(materials: &MaterialTable) -> Result<Self, WorldError> {
        Ok(Self {
            kinds: DeviceKinds::builtin().map_err(|e| WorldError::Device(e.to_string()))?,
            table: BTreeMap::new(),
            next: 1,
            material: materials.id("device"),
            rebuilds: 0,
            intake_checks: 0,
            floor_intake_tops: BTreeMap::new(),
            floor_owners: BTreeMap::new(),
        })
    }

    /// Whether floor devices `a` and `b` touch edge to edge.
    fn floor_adjacent(&self, a: &Device, b: &Device) -> bool {
        let (wa, wb) = (self.kinds.get(a.kind).size.0, self.kinds.get(b.kind).size.0);
        a.x + wa == b.x || b.x + wb == a.x
    }

    /// The under-gravel filter each floor panel is connected to, by panel id.
    ///
    /// Floor devices (`Mount::Floor` and `Mount::FloorPanel`) are connected
    /// when they touch edge to edge. Each panel reachable from a filter
    /// through a chain of touching floor devices maps to the nearest filter
    /// (fewest links; the lowest id on a tie). Panels with no such chain are
    /// absent.
    pub(super) fn connected_floor_owners(&self) -> BTreeMap<u16, DeviceId> {
        let mount = |d: &Device| self.kinds.get(d.kind).mount;
        let mut owners: BTreeMap<u16, DeviceId> = BTreeMap::new();
        let mut frontier: Vec<(&Device, DeviceId)> = self
            .table
            .values()
            .filter(|d| mount(d) == Mount::Floor)
            .map(|d| (d, d.id))
            .collect();
        while !frontier.is_empty() {
            let mut next: BTreeMap<u16, DeviceId> = BTreeMap::new();
            for &(from, owner) in &frontier {
                for panel in self.table.values() {
                    if mount(panel) == Mount::FloorPanel
                        && !owners.contains_key(&panel.id.0)
                        && self.floor_adjacent(from, panel)
                    {
                        next.entry(panel.id.0)
                            .and_modify(|o| *o = (*o).min(owner))
                            .or_insert(owner);
                    }
                }
            }
            frontier = next
                .iter()
                .filter_map(|(id, &owner)| self.table.get(id).map(|d| (d, owner)))
                .collect();
            owners.extend(next);
        }
        owners
    }

    /// Recomputes the cached panel owners after the table changed.
    pub(super) fn refresh_floor_owners(&mut self) {
        self.floor_owners = self.connected_floor_owners();
    }

    /// Whether `d` is working: any device but a floor panel not connected to
    /// an under-gravel filter.
    pub(super) fn is_connected(&self, d: &Device) -> bool {
        self.kinds.get(d.kind).mount != Mount::FloorPanel || self.floor_owners.contains_key(&d.id.0)
    }

    /// Invalidates cached bed-top rows for any floor intake column whose
    /// substrate (at or below `top - 1`) overlaps the dirty rect `[x0..x1) × [y0..y1)`.
    pub(super) fn invalidate_floor_intakes_in(&mut self, x0: usize, x1: usize, y1: usize) {
        if !self.floor_intake_tops.is_empty() {
            self.floor_intake_tops
                .retain(|&(_, ix), &mut top| ix < x0 || ix >= x1 || y1 < top);
        }
    }

    pub(super) fn clear_floor_intake_tops(&mut self) {
        self.floor_intake_tops.clear();
    }

    pub(super) fn values(&self) -> impl Iterator<Item = &Device> {
        self.table.values()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    pub(super) fn kinds(&self) -> &DeviceKinds {
        &self.kinds
    }

    /// The material footprint cells are made of, if the table has it.
    pub(super) fn material(&self) -> Option<MaterialId> {
        self.material
    }

    /// The device a cell of `material` with `state` belongs to, if any.
    pub(super) fn owner(&self, material: MaterialId, state: u16) -> Option<DeviceId> {
        (Some(material) == self.material && self.table.contains_key(&state))
            .then_some(DeviceId(state))
    }
}

impl World {
    /// The device kinds (art and placement rules).
    #[must_use]
    pub fn device_kinds(&self) -> &DeviceKinds {
        self.devices.kinds()
    }

    /// Every device, by id.
    pub fn devices(&self) -> impl Iterator<Item = &Device> {
        self.devices.values()
    }

    /// The device `id`, if it is in the tank.
    #[must_use]
    pub fn device(&self, id: DeviceId) -> Option<&Device> {
        self.devices.table.get(&id.0)
    }

    /// The device whose footprint covers cell `(x, y)`, if any.
    #[must_use]
    pub fn device_at(&self, x: usize, y: usize) -> Option<DeviceId> {
        let c = self.cell(x, y)?;
        self.devices.owner(c.material, c.state)
    }

    /// Times the device jet has been rebuilt (for idle-cost tests).
    #[must_use]
    pub fn device_rebuilds(&self) -> u64 {
        self.devices.rebuilds
    }

    /// Intake-neighbour cell checks performed by filtration (for tests).
    #[must_use]
    pub fn intake_cell_checks(&self) -> u64 {
        self.devices.intake_checks
    }

    /// Where a `kind` placed at cell `(x, y)` would go, and whether it fits
    /// there.
    ///
    /// - [`Mount::SideGlass`]: on the nearer side glass (the other one if
    ///   `flip`), centred on `y` if it fits, else at the nearest height within
    ///   its own height where every footprint cell is fluid and under the water
    ///   line.
    /// - [`Mount::Bed`]: horizontally centred on `x` (symmetric, ignoring
    ///   `flip`), dropped straight down onto substrate, hardscape or the tank
    ///   floor wholly under the water line (lifting up within its own height if
    ///   the pointer is on or inside the bed).
    /// - [`Mount::Rim`]: horizontally centred on `x` and hung on the top rim
    ///   (`y = 0`), partly above the water line (`flip` mirrors left/right),
    ///   requiring every footprint cell to be fluid (air or water) and every
    ///   intake cell to reach under the water line.
    /// - [`Mount::Floor`]: horizontally centred on `x` and laid flush with the
    ///   tank floor, under any powder substrate already there (`flip` mirrors
    ///   left/right). Every footprint cell must be fluid or powder, the nozzle
    ///   cell fluid (the spray bar not buried), and every powder cell it
    ///   displaces must find room in the water above or beside it. A device
    ///   with a telescoping tube (the under-gravel filter) starts just under
    ///   the water line and reaches down to the floor.
    /// - [`Mount::FloorPanel`]: as a floor mount (symmetric, ignoring `flip`,
    ///   with no nozzle to keep clear).
    ///
    /// A floor mount dropped on, or within a few cells of, another floor
    /// device snaps flush against its nearer side when it fits there.
    ///
    /// The returned device's id is 0.
    #[must_use]
    pub fn device_placement(&self, kind: DeviceKind, x: i64, y: i64, flip: bool) -> (Device, bool) {
        let art = self.devices.kinds.get(kind);
        let (w, h) = art.size;
        let (w_i, h_i) = (
            i64::try_from(w).unwrap_or(i64::MAX),
            i64::try_from(h).unwrap_or(i64::MAX),
        );
        let top = i64::try_from(self.surface_row).unwrap_or(0);
        let bottom = i64::try_from(self.height).unwrap_or(0) - h_i;
        let want = (y - h_i / 2).clamp(top, bottom.max(top));
        let (x0, facing_left) = match art.mount {
            Mount::SideGlass => {
                let right_half = usize::try_from(x).is_ok_and(|x| 2 * x + 1 >= self.width);
                let facing_left = right_half != flip;
                let x0 = if facing_left {
                    self.width.saturating_sub(w)
                } else {
                    0
                };
                (x0, facing_left)
            }
            Mount::Bed | Mount::FloorPanel => {
                let max_x = i64::try_from(self.width.saturating_sub(w)).unwrap_or(0);
                let x0 = usize::try_from((x - w_i / 2).clamp(0, max_x)).unwrap_or(0);
                (x0, false)
            }
            Mount::Rim | Mount::Floor => {
                let max_x = i64::try_from(self.width.saturating_sub(w)).unwrap_or(0);
                let x0 = usize::try_from((x - w_i / 2).clamp(0, max_x)).unwrap_or(0);
                (x0, flip)
            }
        };
        let at = |y0: i64| Device {
            y: usize::try_from(y0).unwrap_or(0),
            ..Self::blank_device(kind, x0, h, facing_left)
        };
        if art.mount == Mount::Rim {
            let dev = at(0);
            let ok = w <= self.width
                && h <= self.height
                && self.device_fits(&dev)
                && dev.intakes(art).all(|(_, iy)| iy >= self.surface_row);
            return (dev, ok);
        }
        if art.mount.is_floor() {
            return self.floor_device_placement(kind, x0, facing_left);
        }
        if w <= self.width && bottom >= top {
            match art.mount {
                Mount::SideGlass => {
                    for k in 0..=h_i {
                        for y0 in [want - k, want + k] {
                            if (top..=bottom).contains(&y0) && self.device_fits(&at(y0)) {
                                return (at(y0), true);
                            }
                        }
                    }
                }
                Mount::Bed => {
                    // Lift upward when the pointer is on/inside a bed, or nudge
                    // downward past an overhang only while the device still
                    // reaches the clicked row `y` (never jumping through a
                    // shelf below the pointer).
                    let max_start = bottom.min(y);
                    let start = (0..=h_i).find_map(|k| {
                        [want - k, want + k].into_iter().find(|&y0| {
                            (top..=bottom).contains(&y0)
                                && (y0 <= want || y0 <= max_start)
                                && self.device_fits(&at(y0))
                        })
                    });
                    if let Some(mut y0) = start {
                        while y0 < bottom && self.device_fits(&at(y0 + 1)) {
                            y0 += 1;
                        }
                        return (at(y0), true);
                    }
                }
                Mount::Rim | Mount::Floor | Mount::FloorPanel => unreachable!("handled above"),
            }
        }
        (at(want), false)
    }

    /// A device of `kind` at column `x` with no id, load or maturity, and
    /// a box `height` rows tall (its anchor row is set by the caller).
    fn blank_device(kind: DeviceKind, x: usize, height: usize, facing_left: bool) -> Device {
        Device {
            id: DeviceId(0),
            kind,
            x,
            y: 0,
            height,
            facing_left,
            media_load: 0.0,
            maturity: 0.0,
            intake_guard: false,
        }
    }

    /// Where a floor-mounted `kind` whose pointer column puts its left edge
    /// at `x0` goes, and whether it fits there (see `World::device_placement`).
    fn floor_device_placement(
        &self,
        kind: DeviceKind,
        x0: usize,
        facing_left: bool,
    ) -> (Device, bool) {
        let art = self.devices.kinds.get(kind);
        let (w, h) = art.size;
        // A telescoping tube starts just under the water line (or as low as
        // the box allows) and reaches the floor; any other floor mount is
        // its art's height, flush with the floor.
        let (y0, span) = if art.tube_stretch.is_some() {
            let y0 = (self.surface_row + 1).min(self.height.saturating_sub(h));
            (y0, self.height.saturating_sub(y0))
        } else {
            (self.height.saturating_sub(h), h)
        };
        let make = |x: usize| Device {
            y: y0,
            ..Self::blank_device(kind, x, span, facing_left)
        };
        let fits = |dev: &Device| {
            w <= self.width
                && h <= self.height
                && y0 >= self.surface_row
                && self.floor_placement_plan(dev).is_some()
        };
        // Snap flush against a floor device the pointer is on or beside.
        let snapped = self
            .floor_snap_columns(x0, w)
            .into_iter()
            .map(make)
            .find(fits);
        if let Some(dev) = snapped {
            return (dev, true);
        }
        let dev = make(x0);
        let ok = fits(&dev);
        (dev, ok)
    }

    /// Left columns, nearest `x0` first, that lay a floor device `w` wide
    /// flush against the left or right edge of each floor device that `x0`
    /// overlaps or lies within `FLOOR_SNAP_CELLS` of.
    fn floor_snap_columns(&self, x0: usize, w: usize) -> Vec<usize> {
        let mut snaps: Vec<usize> = Vec::new();
        for o in self.devices.values() {
            let oa = self.devices.kinds.get(o.kind);
            if !oa.mount.is_floor() {
                continue;
            }
            let (ox, ow) = (o.x, oa.size.0);
            if x0 <= ox + ow + FLOOR_SNAP_CELLS && ox <= x0 + w + FLOOR_SNAP_CELLS {
                if ox + ow + w <= self.width {
                    snaps.push(ox + ow);
                }
                if ox >= w {
                    snaps.push(ox - w);
                }
            }
        }
        snaps.sort_by_key(|&c| (c.abs_diff(x0), c));
        snaps.dedup();
        snaps
    }

    /// Whether every footprint cell of `d` is in the grid and fluid.
    fn device_fits(&self, d: &Device) -> bool {
        let art = self.devices.kinds.get(d.kind);
        d.x + art.size.0 <= self.width
            && d.y + d.span(art) <= self.height
            && d.cells(art).all(|(x, y)| {
                let m = self.cells[y * self.width + x].material;
                self.is_fluid(m)
            })
    }

    /// Whether a floor-mounted `d` (an under-gravel filter or panel) can be laid at its
    /// position, and where each powder cell it displaces goes: `(cx, cy, tx, ty)`
    /// moves the powder cell at `(cx, cy)` in its footprint to `(tx, ty)`.
    ///
    /// Every footprint cell must be fluid or powder and the nozzle cell
    /// (the spray bar), if it has one, must be fluid. Each displaced powder cell takes the
    /// lowest free fluid cell above it in its own column (the bed rises by the
    /// plate's thickness) or, in a column the device fills right up (the
    /// uplift tube), the lowest free fluid cell in the nearest other column
    /// of the device's width. A cell is free if it is fluid, under the water
    /// line, outside the footprint, unclaimed, and the substrate would come
    /// to rest there: on the bed, the floor or the plate, never on top of a
    /// tube. `None` if any powder cell has nowhere to go.
    pub(super) fn floor_placement_plan(
        &self,
        d: &Device,
    ) -> Option<Vec<(usize, usize, usize, usize)>> {
        let art = self.devices.kinds.get(d.kind);
        let w = art.size.0;
        if d.x + w > self.width || d.y + d.span(art) > self.height {
            return None;
        }
        let mut displaced = Vec::new();
        for (cx, cy) in d.cells(art) {
            let m = self.cells[cy * self.width + cx].material;
            if self.is_powder(m) {
                displaced.push((cx, cy));
            } else if !self.is_fluid(m) {
                return None;
            }
        }
        if let Some((nx, ny)) = d.nozzle(art)
            && !self.is_fluid(self.cells[ny * self.width + nx].material)
        {
            return None;
        }
        // Substrate may rest on the plate (its top row is the intake row)
        // but not on the tubes above it.
        let plate_top = d
            .intakes(art)
            .map(|(_, iy)| iy)
            .min()
            .unwrap_or(d.y + d.span(art));
        let free = |claimed: &BTreeSet<(usize, usize)>, tx: usize, ty: usize| {
            if ty < self.surface_row
                || !self.is_fluid(self.cells[ty * self.width + tx].material)
                || d.covers(art, tx, ty)
                || claimed.contains(&(tx, ty))
            {
                return false;
            }
            for sy in ty + 1..self.height {
                if d.covers(art, tx, sy) {
                    return sy >= plate_top;
                }
                if !self.is_fluid(self.cells[sy * self.width + tx].material) {
                    return true;
                }
            }
            true
        };
        let mut claimed = BTreeSet::new();
        let mut plan = Vec::with_capacity(displaced.len());
        for (cx, cy) in displaced {
            let above = (self.surface_row..cy)
                .rev()
                .find(|&ty| free(&claimed, cx, ty));
            let to = above.map(|ty| (cx, ty)).or_else(|| {
                (1..w).find_map(|dx| {
                    [cx.checked_sub(dx), cx.checked_add(dx)]
                        .into_iter()
                        .flatten()
                        .filter(|tx| (d.x..d.x + w).contains(tx))
                        .find_map(|tx| {
                            (self.surface_row..self.height)
                                .rev()
                                .find(|&ty| free(&claimed, tx, ty))
                                .map(|ty| (tx, ty))
                        })
                })
            });
            let (tx, ty) = to?;
            claimed.insert((tx, ty));
            plan.push((cx, cy, tx, ty));
        }
        Some(plan)
    }

    /// Whether cells of `material` are powder (substrate).
    fn is_powder(&self, material: MaterialId) -> bool {
        self.props[usize::from(material.0)].class == MovementClass::Powder
    }

    /// Whether `(x, y)` is above the plate of a floor-mounted filter (an
    /// under-gravel filter, or a floor panel connected to one): some such
    /// device covers the floor cell `(x, height - 1)` of that column and `y`
    /// is above the floor row. A panel with no filter connected does nothing.
    pub(super) fn is_over_floor_filter(&self, x: usize, y: usize) -> bool {
        if self.devices.is_empty() || y + 1 >= self.height {
            return false;
        }
        self.devices.values().any(|d| {
            let art = self.devices.kinds.get(d.kind);
            art.mount.is_floor()
                && self.devices.is_connected(d)
                && d.covers(art, x, self.height - 1)
        })
    }

    /// Adds a `kind` at its placement for cell `(x, y)`
    /// ([`World::device_placement`]). Returns its id, or `None` if it
    /// doesn't fit there or the tank already holds [`MAX_DEVICES`].
    ///
    /// # Errors
    ///
    /// Returns an error if the material table has no `device` material.
    pub fn add_device(
        &mut self,
        kind: DeviceKind,
        x: i64,
        y: i64,
        flip: bool,
    ) -> Result<Option<DeviceId>, WorldError> {
        let material = self
            .devices
            .material
            .ok_or(WorldError::MissingMaterial("device"))?;
        let (mut d, ok) = self.device_placement(kind, x, y, flip);
        if !ok || self.devices.table.len() >= MAX_DEVICES {
            return Ok(None);
        }
        let Some(id) = self.next_device_id() else {
            return Ok(None);
        };
        d.id = DeviceId(id);
        let art = self.devices.kinds.get(kind);
        let cells: Vec<_> = d.cells(art).collect();
        if art.mount.is_floor() {
            // Lift the substrate the plate displaces onto the bed above it,
            // keeping each cell exactly (material, variant and state).
            let plan = self.floor_placement_plan(&d).unwrap_or_default();
            let moved: Vec<_> = plan
                .iter()
                .map(|&(cx, cy, _, _)| self.cells[cy * self.width + cx])
                .collect();
            for (cell, &(_, _, tx, ty)) in moved.into_iter().zip(&plan) {
                self.write_cell(tx, ty, cell.material, &mut None);
                self.cells[ty * self.width + tx] = cell;
            }
        }
        for (cx, cy) in cells {
            self.write_cell(cx, cy, material, &mut None);
            self.cells[cy * self.width + cx].state = id;
        }
        self.devices.table.insert(id, d);
        self.devices.refresh_floor_owners();
        self.devices.clear_floor_intake_tops();
        self.mark_devices_dirty();
        Ok(Some(d.id))
    }

    fn next_device_id(&mut self) -> Option<u16> {
        let mut id = self.devices.next.max(1);
        for _ in 0..u16::MAX {
            if !self.devices.table.contains_key(&id) {
                self.devices.next = id.checked_add(1).unwrap_or(1);
                return Some(id);
            }
            id = id.checked_add(1).unwrap_or(1);
        }
        None
    }

    /// Removes device `id` whole: its footprint cells become water under
    /// the water line and air above it. Returns whether it was there.
    pub fn remove_device(&mut self, id: DeviceId) -> bool {
        let Some(d) = self.devices.table.remove(&id.0) else {
            return false;
        };
        self.devices.refresh_floor_owners();
        self.devices.clear_floor_intake_tops();
        let (Some(water), Some(air)) = (self.materials.id("water"), self.materials.id("air"))
        else {
            return true;
        };
        let art = self.devices.kinds.get(d.kind);
        let cells: Vec<_> = d.cells(art).collect();
        for (x, y) in cells {
            let c = self.cells[y * self.width + x];
            if Some(c.material) == self.devices.material && c.state == id.0 {
                let to = if y < self.surface_row { air } else { water };
                self.write_cell(x, y, to, &mut None);
            }
        }
        self.mark_devices_dirty();
        true
    }

    /// Called by `write_cell` when it overwrote a cell of `material` with
    /// `state`: a device's cell takes the whole device with it.
    pub(super) fn device_cell_removed(&mut self, material: MaterialId, state: u16) {
        if let Some(id) = self.devices.owner(material, state) {
            self.remove_device(id);
        }
    }

    /// The status the inspect card shows for device `id`.
    #[must_use]
    pub fn device_status(&self, id: DeviceId) -> Option<DeviceStatus> {
        let d = self.device(id)?;
        let art = self.devices.kinds.get(d.kind);
        let flow = self.effective_nozzle(d, art).map_or(0.0, |(nx, ny)| {
            let [dx, dy] = d.nozzle_dir(art);
            let [vx, vy] = self.nozzle_flow(nx, ny, [dx, dy]);
            (vx * vx + vy * vy).sqrt()
        });
        Some(DeviceStatus {
            kind: d.kind,
            is_filter: art.is_filter(),
            flow,
            facing_left: d.facing_left,
            media_load: d.media_load,
            maturity: d.maturity,
            intake_guard: d.intake_guard,
            fry_safe: art.fry_safe,
            connected: self.devices.is_connected(d),
        })
    }

    /// World cell where device `d`'s nozzle jet enters the water: for rim
    /// mounts whose spillway hangs above the water line, projects down to
    /// `surface_row` where the waterfall hits the surface. Returns `None` for
    /// non-jet devices (e.g. the prefab bubbler).
    fn effective_nozzle(
        &self,
        d: &Device,
        art: &crate::device::DeviceArt,
    ) -> Option<(usize, usize)> {
        let (nx, ny) = d.nozzle(art)?;
        if art.mount == Mount::Rim {
            Some((
                nx,
                ny.max(self.surface_row).min(self.height.saturating_sub(1)),
            ))
        } else {
            Some((nx, ny))
        }
    }

    /// Depth band of device cell `i` if it belongs to a submerged device
    /// section restricted to a single depth band (e.g. a HOB filter's
    /// back-band intake tube below the water line). Returns `None` for
    /// non-device cells, above-water cells (preserving air as a barrier in
    /// every band), and device sections that are solid in every depth band.
    #[inline]
    pub(super) fn device_cell_band(&self, i: usize) -> Option<u8> {
        let c = self.cells[i];
        if Some(c.material) != self.devices.material {
            return None;
        }
        let d = self.devices.table.get(&c.state)?;
        let from_row = d.back_band_row(self.devices.kinds.get(d.kind))?;
        let y = i / self.width;
        (y >= from_row && y >= self.surface_row).then_some(super::creatures::DEPTH_BANDS - 1)
    }

    /// Active surface-ripple zones `[x0, x1_exclusive, spillway_y, strength]`
    /// in grid cells for rim-mounted waterfall filters (D27, DV5). `strength`
    /// is the device's flow factor (`CLOG_FLOW_FLOOR..=1.0`), so clogging
    /// softens the waterfall and surface agitation.
    #[must_use]
    pub fn surface_ripples(&self) -> Vec<[f32; 4]> {
        if self.devices.is_empty() || self.surface_row >= self.height {
            return Vec::new();
        }
        let floor = self.life.eco.filtration.clog_floor;
        let mut out = Vec::new();
        for d in self.devices.values() {
            let art = self.devices.kinds.get(d.kind);
            if let Some((x0, x1, sy)) = d.spillway_world(art) {
                #[allow(clippy::cast_precision_loss)] // grid coords ≤ 8192
                out.push([x0 as f32, x1 as f32, sy as f32, d.flow_factor_with(floor)]);
            }
        }
        out
    }

    /// Whether any placed device or strong surface current drives
    /// water-surface ripples (O(1) over `<= MAX_DEVICES`).
    #[must_use]
    pub fn has_surface_ripples(&self) -> bool {
        self.has_surface_current_ripples()
            || (!self.devices.is_empty()
                && self.surface_row < self.height
                && self
                    .devices
                    .values()
                    .any(|d| self.devices.kinds.get(d.kind).spillway.is_some()))
    }

    /// Rinses or replaces device `id`'s filter media (D27, DV3). Returns
    /// whether the device exists and has filter media.
    pub fn rinse_device(&mut self, id: DeviceId, kind: RinseKind) -> bool {
        let floor = self.life.eco.filtration.clog_floor;
        let Some(dev_kind) = self.devices.table.get(&id.0).map(|d| d.kind) else {
            return false;
        };
        if !self.devices.kinds.get(dev_kind).is_filter() {
            return false;
        }
        let Some(d) = self.devices.table.get_mut(&id.0) else {
            return false;
        };
        let before = d.flow_factor_with(floor);
        d.media_load = 0.0;
        match kind {
            RinseKind::Gentle => {}
            RinseKind::Hard => d.maturity *= 0.25,
            RinseKind::Replace => d.maturity = 0.0,
        }
        let after = d.flow_factor_with(floor);
        if before.to_bits() != after.to_bits() {
            self.recompute_current();
        }
        true
    }

    /// Fits or removes device `id`'s intake guard (D27, DV3). Intrinsically
    /// fry-safe devices keep `intake_guard` off. Returns whether the device
    /// exists.
    pub fn set_device_intake_guard(&mut self, id: DeviceId, on: bool) -> bool {
        let Some(kind) = self.devices.table.get(&id.0).map(|d| d.kind) else {
            return false;
        };
        if self.devices.kinds.get(kind).fry_safe {
            return true;
        }
        if let Some(d) = self.devices.table.get_mut(&id.0) {
            d.intake_guard = on;
        }
        true
    }

    /// Toggles device `id`'s intake guard, returning its new state
    /// (intrinsically fry-safe devices always stay `false`).
    pub fn toggle_device_intake_guard(&mut self, id: DeviceId) -> Option<bool> {
        let kind = self.devices.table.get(&id.0)?.kind;
        if self.devices.kinds.get(kind).fry_safe {
            return Some(false);
        }
        let d = self.devices.table.get_mut(&id.0)?;
        d.intake_guard = !d.intake_guard;
        Some(d.intake_guard)
    }

    /// Sets device `id`'s media load in `[0.0, 1.0]` (for tests and debug
    /// scenes). Returns whether the device exists.
    pub fn set_device_media_load(&mut self, id: DeviceId, load: f32) -> bool {
        let floor = self.life.eco.filtration.clog_floor;
        let Some(d) = self.devices.table.get_mut(&id.0) else {
            return false;
        };
        let before = d.flow_factor_with(floor);
        d.media_load = if load.is_finite() {
            load.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let after = d.flow_factor_with(floor);
        if before.to_bits() != after.to_bits() {
            self.recompute_current();
        }
        true
    }

    /// Sets device `id`'s biological maturity in `[0.0, 1.0]` (for tests and
    /// debug scenes). Returns whether the device exists.
    pub fn set_device_maturity(&mut self, id: DeviceId, maturity: f32) -> bool {
        let Some(d) = self.devices.table.get_mut(&id.0) else {
            return false;
        };
        d.maturity = if maturity.is_finite() {
            maturity.clamp(0.0, 1.0)
        } else {
            0.0
        };
        true
    }

    /// Effective biological filtration capacity across all filters in
    /// `[0.0, 1.0]`: each filter contributes `maturity * flow_factor`.
    #[must_use]
    pub fn filter_bio_capacity(&self) -> f32 {
        if self.devices.is_empty() {
            return 0.0;
        }
        let floor = self.life.eco.filtration.clog_floor;
        self.devices
            .values()
            .filter(|d| self.devices.kinds.get(d.kind).is_filter())
            .map(|d| d.maturity * d.flow_factor_with(floor))
            .sum::<f32>()
            .clamp(0.0, 1.0)
    }

    /// Adds `delta` to device `id`'s media load; returns whether its
    /// quantized flow factor changed.
    pub(super) fn add_device_load(&mut self, id: DeviceId, delta: f32) -> bool {
        let floor = self.life.eco.filtration.clog_floor;
        let Some(d) = self.devices.table.get_mut(&id.0) else {
            return false;
        };
        let before = d.flow_factor_with(floor);
        d.media_load = (d.media_load + delta).clamp(0.0, 1.0);
        let after = d.flow_factor_with(floor);
        before.to_bits() != after.to_bits()
    }

    /// Filtration pass (D27, DV3): checks only the immediate 8-neighbours of
    /// each device's intake cells, capturing suspended particles into media
    /// load, and advances biological maturity once per world second.
    pub(super) fn run_intakes(&mut self) {
        if !self
            .devices
            .values()
            .any(|d| self.devices.kinds.get(d.kind).is_filter())
        {
            return;
        }
        let cfg = self.life.eco.filtration;
        let load_per_cell = if cfg.capacity > 0.0 {
            1.0 / cfg.capacity
        } else {
            0.0
        };
        if let Some(water) = self.life.water
            && !cfg.capture.is_empty()
        {
            const RING: [(isize, isize); 8] = [
                (-1, -1),
                (0, -1),
                (1, -1),
                (-1, 0),
                (1, 0),
                (-1, 1),
                (0, 1),
                (1, 1),
            ];
            let mut captured: Vec<(u16, usize)> = Vec::new();
            let mut new_tops: Vec<((u16, usize), usize)> = Vec::new();
            let mut checks = 0u64;
            for d in self.devices.table.values() {
                let art = self.devices.kinds.get(d.kind);
                // A floor panel filters for the under-gravel filter it is
                // connected to (and does nothing, at no cost, if none is).
                let credit = if art.mount == Mount::FloorPanel {
                    let Some(&owner) = self.devices.floor_owners.get(&d.id.0) else {
                        continue;
                    };
                    owner.0
                } else {
                    d.id.0
                };
                for (ix, iy) in d.intakes(art) {
                    // An under-gravel filter draws through the bed: its
                    // intakes capture at the top of the substrate above them
                    // (debris resting there is a neighbour of the last
                    // substrate cell, not part of the bed). The bed-top row
                    // is cached per intake column and invalidated only when
                    // cells change at or below `top - 1` in that column, so
                    // an idle bed stays O(1) per tick.
                    let iy = if art.mount.is_floor() {
                        if let Some(&cached) = self.devices.floor_intake_tops.get(&(d.id.0, ix)) {
                            cached.max(self.surface_row)
                        } else {
                            let mut y = iy;
                            while y > self.surface_row {
                                checks += 1;
                                let m = self.cells[(y - 1) * self.width + ix].material;
                                if self.is_powder(m) && !cfg.capture.contains(m) {
                                    y -= 1;
                                } else {
                                    break;
                                }
                            }
                            new_tops.push(((d.id.0, ix), y));
                            y
                        }
                    } else {
                        iy
                    };
                    for (dx, dy) in RING {
                        checks += 1;
                        if let Some(j) = self.at(ix, iy, dx, dy)
                            && cfg.capture.contains(self.cells[j].material)
                        {
                            captured.push((credit, j));
                        }
                    }
                }
            }
            for (k, top) in new_tops {
                self.devices.floor_intake_tops.insert(k, top);
            }
            self.devices.intake_checks += checks;
            let mut flow_changed = false;
            for (dev_id, j) in captured {
                // Re-check in case two intakes shared a neighbour cell.
                if !cfg.capture.contains(self.cells[j].material) {
                    continue;
                }
                self.write_cell(j % self.width, j / self.width, water, &mut None);
                flow_changed |= self.add_device_load(DeviceId(dev_id), load_per_cell);
            }
            if flow_changed {
                self.mark_devices_dirty();
            }
        }
        // Biological media colonisation: once per world second.
        if self.tick.is_multiple_of(u64::from(TICKS_PER_SECOND)) && cfg.maturity_days > 0.0 {
            #[allow(clippy::cast_precision_loss)] // LIFE_DAY_SECONDS = 600
            let base_rate = 1.0 / (cfg.maturity_days * LIFE_DAY_SECONDS as f32);
            let kinds = &self.devices.kinds;
            for d in self.devices.table.values_mut() {
                if kinds.get(d.kind).is_filter() && d.maturity < 1.0 {
                    let feed_mult = 1.0 + 0.5 * d.media_load;
                    d.maturity = (d.maturity + base_rate * feed_mult).min(1.0);
                }
            }
        }
    }

    /// Whether any placed device invites shrimp grazing (O(1) over ≤ `MAX_DEVICES`).
    pub(super) fn has_grazeable_device(&self) -> bool {
        !self.devices.is_empty()
            && self
                .devices
                .values()
                .any(|d| self.devices.kinds.get(d.kind).graze_rows.is_some())
    }

    /// Each jet-driving device's nozzle (position and unit direction), its
    /// intake cells with outward open-water directions (and whether the intake
    /// draws down through a substrate bed, as a floor-mounted plate's do), and
    /// its scaled flow factor (`flow_scale * flow_factor`), for the 2D current
    /// field.
    #[allow(clippy::type_complexity)] // internal tuple for recompute_current
    pub(super) fn device_flows(
        &self,
    ) -> Vec<(
        ((usize, usize), [f32; 2]),
        Vec<((usize, usize), [f32; 2], bool)>,
        f32,
    )> {
        let floor = self.life.eco.filtration.clog_floor;
        self.devices
            .values()
            .filter_map(|d| {
                let art = self.devices.kinds.get(d.kind);
                let nozzle = self.effective_nozzle(d, art)?;
                if art.flow_scale <= 0.0 {
                    return None;
                }
                let mut intakes: Vec<_> = d
                    .intakes(art)
                    .map(|(ix, iy)| {
                        (
                            (ix, iy),
                            d.intake_outward(art, ix, iy),
                            art.mount.is_floor(),
                        )
                    })
                    .collect();
                // Panels connected to this filter draw water down through the
                // gravel over them too.
                for (&panel_id, &owner) in &self.devices.floor_owners {
                    if owner != d.id {
                        continue;
                    }
                    if let Some(p) = self.devices.table.get(&panel_id) {
                        let pa = self.devices.kinds.get(p.kind);
                        intakes.extend(p.intakes(pa).map(|(ix, iy)| ((ix, iy), [0.0, -1.0], true)));
                    }
                }
                Some((
                    (nozzle, d.nozzle_dir(art)),
                    intakes,
                    art.flow_scale * d.flow_factor_with(floor),
                ))
            })
            .collect()
    }

    pub(super) fn hash_devices(&self, h: &mut Fnv64) {
        for d in self.devices.values() {
            h.write(&d.id.0.to_le_bytes());
            h.write(d.kind.name().as_bytes());
            h.write(&(d.x as u64).to_le_bytes());
            h.write(&(d.y as u64).to_le_bytes());
            h.write(&[u8::from(d.facing_left)]);
            h.write(&d.media_load.to_bits().to_le_bytes());
            h.write(&d.maturity.to_bits().to_le_bytes());
            h.write(&[u8::from(d.intake_guard)]);
        }
    }

    pub(super) fn save_devices(&self) -> (Vec<v12::Device>, u16) {
        let devices = self
            .devices
            .values()
            .map(|d| v12::Device {
                id: d.id.0,
                kind: d.kind.name().to_owned(),
                x: u32::try_from(d.x).unwrap_or(u32::MAX),
                y: u32::try_from(d.y).unwrap_or(u32::MAX),
                facing_left: d.facing_left,
            })
            .collect();
        (devices, self.devices.next)
    }

    pub(super) fn save_device_filtration(&self) -> Vec<v13::DeviceFiltration> {
        self.devices
            .values()
            .map(|d| v13::DeviceFiltration {
                media_load: d.media_load,
                maturity: d.maturity,
                intake_guard: d.intake_guard,
            })
            .collect()
    }

    /// Restores the device table (after the cells, before the life lists,
    /// so the current is worked out with the devices in place).
    pub(super) fn load_devices(
        &mut self,
        saved: &[v12::Device],
        next: u16,
        filtration: Option<&[v13::DeviceFiltration]>,
    ) -> Result<(), SaveError> {
        if saved.len() > MAX_DEVICES {
            return Err(SaveError::Invalid(format!("{} devices", saved.len())));
        }
        if let Some(f) = filtration
            && f.len() != saved.len()
        {
            return Err(SaveError::Invalid(format!(
                "{} device filtration records for {} devices",
                f.len(),
                saved.len()
            )));
        }
        let mut table = BTreeMap::new();
        for (i, s) in saved.iter().enumerate() {
            let kind = DeviceKind::from_name(&s.kind)
                .ok_or_else(|| SaveError::Mismatch(format!("unknown device kind {:?}", s.kind)))?;
            let (media_load, maturity, intake_guard) = if let Some(f) = filtration.map(|f| f[i]) {
                if !(f.media_load.is_finite()
                    && (0.0..=1.0).contains(&f.media_load)
                    && f.maturity.is_finite()
                    && (0.0..=1.0).contains(&f.maturity))
                {
                    return Err(SaveError::Invalid(format!(
                        "device {} filtration out of range",
                        s.id
                    )));
                }
                (f.media_load, f.maturity, f.intake_guard)
            } else {
                (0.0, 0.0, false)
            };
            let art = self.devices.kinds.get(kind);
            let (x, y) = (s.x as usize, s.y as usize);
            let d = Device {
                id: DeviceId(s.id),
                kind,
                x,
                y,
                // A telescoping tube reaches the floor from wherever it
                // starts, so the saved anchor is all that fixes its height.
                height: if art.tube_stretch.is_some() {
                    self.height.saturating_sub(y)
                } else {
                    art.size.1
                },
                facing_left: s.facing_left,
                media_load,
                maturity,
                intake_guard,
            };
            if s.id == 0
                || d.x + art.size.0 > self.width
                || d.y + art.size.1 > self.height
                || table.insert(s.id, d).is_some()
            {
                return Err(SaveError::Invalid(format!("device {} out of place", s.id)));
            }
        }
        // Each footprint cell must hold its device (which also rules out
        // overlapping footprints: a cell holds one id).
        for d in table.values() {
            let art = self.devices.kinds.get(d.kind);
            let ok = d.cells(art).all(|(x, y)| {
                let c = self.cells[y * self.width + x];
                Some(c.material) == self.devices.material && c.state == d.id.0
            });
            if !ok {
                return Err(SaveError::Invalid(format!(
                    "device {}'s footprint isn't its cells",
                    d.id.0
                )));
            }
        }
        self.devices.table = table;
        self.devices.next = next.max(1);
        self.devices.refresh_floor_owners();
        self.devices.clear_floor_intake_tops();
        Ok(())
    }

    /// Turns legacy filter outlet cells into internal filters (D27), as
    /// loading a version 11 or older save does: each connected group of
    /// outlet cells is cleared and an internal filter placed at its centre,
    /// on the glass its jet pointed away from. A group with no room for a
    /// filter is left as legacy outlet cells, which still drive the jet.
    pub fn adopt_legacy_outlets(&mut self) {
        let Some(outlet) = self.materials.id("filter_outlet") else {
            return;
        };
        let (Some(water), Some(air)) = (self.materials.id("water"), self.materials.id("air"))
        else {
            return;
        };
        let mut seen = BTreeSet::new();
        let mut groups = Vec::new();
        for i in 0..self.cells.len() {
            if self.cells[i].material != outlet || !seen.insert(i) {
                continue;
            }
            let mut group = vec![i];
            let mut k = 0;
            while k < group.len() {
                let j = group[k];
                k += 1;
                let (x, y) = (j % self.width, j / self.width);
                let near = [
                    (x > 0).then(|| j - 1),
                    (x + 1 < self.width).then(|| j + 1),
                    (y > 0).then(|| j - self.width),
                    (y + 1 < self.height).then(|| j + self.width),
                ];
                for n in near.into_iter().flatten() {
                    if self.cells[n].material == outlet && seen.insert(n) {
                        group.push(n);
                    }
                }
            }
            groups.push(group);
        }
        for group in groups {
            let n = group.len();
            let (sx, sy) = group.iter().fold((0, 0), |(sx, sy), &i| {
                (sx + i % self.width, sy + i / self.width)
            });
            let (cx, cy) = (sx / n, sy / n);
            for &i in &group {
                let (x, y) = (i % self.width, i / self.width);
                let to = if y < self.surface_row { air } else { water };
                self.write_cell(x, y, to, &mut None);
            }
            let (x, y) = (
                i64::try_from(cx).unwrap_or(0),
                i64::try_from(cy).unwrap_or(0),
            );
            if !matches!(
                self.add_device(DeviceKind::InternalFilter, x, y, false),
                Ok(Some(_))
            ) {
                for &i in &group {
                    self.write_cell(i % self.width, i / self.width, outlet, &mut None);
                }
            }
        }
        if !self.devices.is_empty() {
            self.refresh_all_blocked();
            self.recompute_current_quiet();
        }
    }

    /// Turns legacy `bubbler` cells into prefab bubblers (D27, DV7): each
    /// connected group of legacy bubbler cells is cleared and a bed-mounted
    /// prefab bubbler placed at its centre. A group with no room for one
    /// (boxed in by rock, say) is left untouched as legacy bubbler cells,
    /// which still emit bubbles, without waking chunks or mutating state.
    pub fn adopt_legacy_bubblers(&mut self) {
        if self.devices.material.is_none() {
            return;
        }
        let Some(bubbler) = self.materials.id("bubbler") else {
            return;
        };
        if self.material_counts()[usize::from(bubbler.0)] == 0 {
            return;
        }
        let (Some(water), Some(air)) = (self.materials.id("water"), self.materials.id("air"))
        else {
            return;
        };
        let mut seen = BTreeSet::new();
        let mut groups = Vec::new();
        for i in 0..self.cells.len() {
            if self.cells[i].material != bubbler || !seen.insert(i) {
                continue;
            }
            let mut group = vec![i];
            let mut k = 0;
            while k < group.len() {
                let j = group[k];
                k += 1;
                let (x, y) = (j % self.width, j / self.width);
                let near = [
                    (x > 0).then(|| j - 1),
                    (x + 1 < self.width).then(|| j + 1),
                    (y > 0).then(|| j - self.width),
                    (y + 1 < self.height).then(|| j + self.width),
                ];
                for n in near.into_iter().flatten() {
                    if self.cells[n].material == bubbler && seen.insert(n) {
                        group.push(n);
                    }
                }
            }
            groups.push(group);
        }
        let mut adopted = false;
        for group in groups {
            if self.devices.table.len() >= MAX_DEVICES {
                break;
            }
            let n = group.len();
            let (sx, sy) = group.iter().fold((0, 0), |(sx, sy), &i| {
                (sx + i % self.width, sy + i / self.width)
            });
            let (cx, cy) = (sx / n, sy / n);
            let (x, y) = (
                i64::try_from(cx).unwrap_or(0),
                i64::try_from(cy).unwrap_or(0),
            );
            // Probe whether a prefab bubbler fits once the group's own cells
            // are cleared, without waking chunks or mutating state if it doesn't.
            let saved: Vec<_> = group.iter().map(|&i| (i, self.cells[i])).collect();
            for &i in &group {
                let row = i / self.width;
                self.cells[i].material = if row < self.surface_row { air } else { water };
            }
            let (_, fits) = self.device_placement(DeviceKind::Bubbler, x, y, false);
            for (i, cell) in saved {
                self.cells[i] = cell;
            }
            if !fits {
                continue;
            }
            for &i in &group {
                let (gx, gy) = (i % self.width, i / self.width);
                let to = if gy < self.surface_row { air } else { water };
                self.write_cell(gx, gy, to, &mut None);
            }
            if matches!(
                self.add_device(DeviceKind::Bubbler, x, y, false),
                Ok(Some(_))
            ) {
                adopted = true;
            } else {
                for &i in &group {
                    self.write_cell(i % self.width, i / self.width, bubbler, &mut None);
                }
            }
        }
        if adopted {
            self.refresh_all_blocked();
            self.recompute_current_quiet();
        }
    }
}
