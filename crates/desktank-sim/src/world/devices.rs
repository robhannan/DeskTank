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

use super::{World, WorldError};
use crate::device::{Device, DeviceId, DeviceKind, DeviceKinds, DeviceStatus, MAX_DEVICES, Mount};
use crate::hash::Fnv64;
use crate::material::{MaterialId, MaterialTable};
use crate::save::{SaveError, v12};

/// The world's devices.
#[derive(Clone, Debug)]
pub(super) struct Devices {
    kinds: DeviceKinds,
    table: BTreeMap<u16, Device>,
    /// Next id to try (ids are 1..=65535).
    next: u16,
    /// The material footprint cells are made of.
    material: Option<MaterialId>,
    /// Times the jet was rebuilt for a device change (tests).
    pub(super) rebuilds: u64,
}

impl Devices {
    pub(super) fn new(materials: &MaterialTable) -> Result<Self, WorldError> {
        Ok(Self {
            kinds: DeviceKinds::builtin().map_err(|e| WorldError::Device(e.to_string()))?,
            table: BTreeMap::new(),
            next: 1,
            material: materials.id("device"),
            rebuilds: 0,
        })
    }

    pub(super) fn values(&self) -> impl Iterator<Item = &Device> {
        self.table.values()
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

    /// Where a `kind` placed at cell `(x, y)` would go, and whether it fits
    /// there. An internal filter goes on the nearer side glass (the other
    /// one if `flip`), centred on `y` if it fits, else at the nearest
    /// height within its own height where every footprint cell is fluid
    /// and under the water line. The device's id is 0.
    #[must_use]
    pub fn device_placement(&self, kind: DeviceKind, x: i64, y: i64, flip: bool) -> (Device, bool) {
        let art = self.devices.kinds.get(kind);
        let (w, h) = art.size;
        let right_half = usize::try_from(x).is_ok_and(|x| 2 * x + 1 >= self.width);
        let facing_left = right_half != flip;
        let Mount::SideGlass = art.mount;
        let x0 = if facing_left {
            self.width.saturating_sub(w)
        } else {
            0
        };
        let h_i = i64::try_from(h).unwrap_or(i64::MAX);
        let top = i64::try_from(self.surface_row).unwrap_or(0);
        let bottom = i64::try_from(self.height).unwrap_or(0) - h_i;
        let want = (y - h_i / 2).clamp(top, bottom.max(top));
        let at = |y0: i64| Device {
            id: DeviceId(0),
            kind,
            x: x0,
            y: usize::try_from(y0).unwrap_or(0),
            facing_left,
        };
        if w <= self.width && bottom >= top {
            for k in 0..=h_i {
                for y0 in [want - k, want + k] {
                    if (top..=bottom).contains(&y0) && self.device_fits(&at(y0)) {
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
        let (nx, ny) = d.nozzle(self.devices.kinds.get(d.kind));
        let [vx, vy] = self.current_at_cell(nx, ny);
        Some(DeviceStatus {
            kind: d.kind,
            flow: vx.hypot(vy),
            facing_left: d.facing_left,
        })
    }

    /// Nozzle tips and their unit jet directions (`+x` right, `+y` down),
    /// for the 2D current field.
    pub(super) fn device_jets(&self) -> Vec<((usize, usize), [f32; 2])> {
        self.devices
            .values()
            .map(|d| {
                let art = self.devices.kinds.get(d.kind);
                (d.nozzle(art), d.nozzle_dir(art))
            })
            .collect()
    }

    /// Intake cells and their inward pull directions (into the device),
    /// for the 2D current field.
    pub(super) fn device_intakes(&self) -> Vec<((usize, usize), [f32; 2])> {
        self.devices
            .values()
            .flat_map(|d| {
                let art = self.devices.kinds.get(d.kind);
                let [dx, dy] = d.nozzle_dir(art);
                let inward = [-dx, -dy];
                d.intakes(art).map(move |pos| (pos, inward))
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

    /// Restores the device table (after the cells, before the life lists,
    /// so the current is worked out with the devices in place).
    pub(super) fn load_devices(
        &mut self,
        saved: &[v12::Device],
        next: u16,
    ) -> Result<(), SaveError> {
        if saved.len() > MAX_DEVICES {
            return Err(SaveError::Invalid(format!("{} devices", saved.len())));
        }
        let mut table = BTreeMap::new();
        for s in saved {
            let kind = DeviceKind::from_name(&s.kind)
                .ok_or_else(|| SaveError::Mismatch(format!("unknown device kind {:?}", s.kind)))?;
            let d = Device {
                id: DeviceId(s.id),
                kind,
                x: s.x as usize,
                y: s.y as usize,
                facing_left: s.facing_left,
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
    }
}
