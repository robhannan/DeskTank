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
use crate::material::{MaterialId, MaterialTable};
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
}

impl Devices {
    pub(super) fn new(materials: &MaterialTable) -> Result<Self, WorldError> {
        Ok(Self {
            kinds: DeviceKinds::builtin().map_err(|e| WorldError::Device(e.to_string()))?,
            table: BTreeMap::new(),
            next: 1,
            material: materials.id("device"),
            rebuilds: 0,
            intake_checks: 0,
        })
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
            Mount::Bed => {
                let max_x = i64::try_from(self.width.saturating_sub(w)).unwrap_or(0);
                let x0 = usize::try_from((x - w_i / 2).clamp(0, max_x)).unwrap_or(0);
                (x0, false)
            }
        };
        let at = |y0: i64| Device {
            id: DeviceId(0),
            kind,
            x: x0,
            y: usize::try_from(y0).unwrap_or(0),
            facing_left,
            media_load: 0.0,
            maturity: 0.0,
            intake_guard: false,
        };
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
            }
        }
        (at(want), false)
    }

    /// Whether every footprint cell of `d` is in the grid and fluid.
    fn device_fits(&self, d: &Device) -> bool {
        let art = self.devices.kinds.get(d.kind);
        d.x + art.size.0 <= self.width
            && d.y + art.size.1 <= self.height
            && d.cells(art).all(|(x, y)| {
                let m = self.cells[y * self.width + x].material;
                self.is_fluid(m)
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
        for (cx, cy) in cells {
            self.write_cell(cx, cy, material, &mut None);
            self.cells[cy * self.width + cx].state = id;
        }
        self.devices.table.insert(id, d);
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
        let (nx, ny) = d.nozzle(art);
        let [dx, dy] = d.nozzle_dir(art);
        let [vx, vy] = self.nozzle_flow(nx, ny, [dx, dy]);
        Some(DeviceStatus {
            kind: d.kind,
            flow: (vx * vx + vy * vy).sqrt(),
            facing_left: d.facing_left,
            media_load: d.media_load,
            maturity: d.maturity,
            intake_guard: d.intake_guard,
            fry_safe: art.fry_safe,
        })
    }

    /// Rinses or replaces device `id`'s filter media (D27, DV3). Returns
    /// whether the device exists.
    pub fn rinse_device(&mut self, id: DeviceId, kind: RinseKind) -> bool {
        let floor = self.life.eco.filtration.clog_floor;
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
        if self.devices.is_empty() {
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
            let mut checks = 0u64;
            for d in self.devices.table.values() {
                let art = self.devices.kinds.get(d.kind);
                for (ix, iy) in d.intakes(art) {
                    for (dx, dy) in RING {
                        checks += 1;
                        if let Some(j) = self.at(ix, iy, dx, dy)
                            && cfg.capture.contains(self.cells[j].material)
                        {
                            captured.push((d.id.0, j));
                        }
                    }
                }
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
            for d in self.devices.table.values_mut() {
                if d.maturity < 1.0 {
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

    /// Each device's nozzle (position and unit direction), its intake cells
    /// with outward open-water directions, and its scaled flow factor
    /// (`flow_scale * flow_factor`), for the 2D current field.
    #[allow(clippy::type_complexity)] // internal tuple for recompute_current
    pub(super) fn device_flows(
        &self,
    ) -> Vec<(
        ((usize, usize), [f32; 2]),
        Vec<((usize, usize), [f32; 2])>,
        f32,
    )> {
        let floor = self.life.eco.filtration.clog_floor;
        self.devices
            .values()
            .map(|d| {
                let art = self.devices.kinds.get(d.kind);
                (
                    (d.nozzle(art), d.nozzle_dir(art)),
                    d.intakes(art)
                        .map(|(ix, iy)| ((ix, iy), d.intake_outward(art, ix, iy)))
                        .collect(),
                    art.flow_scale * d.flow_factor_with(floor),
                )
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
            let d = Device {
                id: DeviceId(s.id),
                kind,
                x: s.x as usize,
                y: s.y as usize,
                facing_left: s.facing_left,
                media_load,
                maturity,
                intake_guard,
            };
            let art = self.devices.kinds.get(kind);
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
}
