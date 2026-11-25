//! Snapshots of a world for the save format (T8, [`crate::save`]): the
//! world's own state here, each part's in its module (`bodies`, `life`,
//! `plants`, `creatures`).

use super::algae::{DENSITY_MASK, FRONT_CAP};
use super::{CHUNK_SIZE, World, WorldConfig, variant};
use crate::grid::{Cell, Rect};
use crate::material::{MaterialId, MaterialTable};
use crate::plants::PlantSpeciesId;
use crate::save::{SaveError, runs, unrun, v1, v9, v10, v11, v12, v13, v14, v15, v16};
use crate::species::SpeciesId;

impl World {
    /// The world as a version 11 snapshot.
    pub(crate) fn to_v11(&self) -> v11::Save {
        v11::Save {
            base: self.to_v10(),
        }
    }

    /// A world restored from a version 11 snapshot, on `materials`.
    pub(crate) fn from_v11(s: v11::Save, materials: MaterialTable) -> Result<Self, SaveError> {
        Self::from_v10(s.base, materials)
    }

    /// The world as a version 12 snapshot.
    pub(crate) fn to_v12(&self) -> v12::Save {
        let (devices, next_device) = self.save_devices();
        v12::Save {
            base: self.to_v11(),
            devices,
            next_device,
        }
    }

    /// A world restored from a version 12 snapshot, on `materials`.
    pub(crate) fn from_v12(s: v12::Save, materials: MaterialTable) -> Result<Self, SaveError> {
        Self::from_v10_with(
            s.base.base,
            materials,
            Some((&s.devices, s.next_device, None)),
        )
    }

    /// The world as a version 13 snapshot.
    pub(crate) fn to_v13(&self) -> v13::Save {
        v13::Save {
            base: self.to_v12(),
            filtration: self.save_device_filtration(),
        }
    }

    /// A world restored from a version 13 snapshot, on `materials`.
    pub(crate) fn from_v13(s: v13::Save, materials: MaterialTable) -> Result<Self, SaveError> {
        Self::from_v10_with(
            s.base.base.base,
            materials,
            Some((&s.base.devices, s.base.next_device, Some(&s.filtration))),
        )
    }

    pub(crate) fn to_v14(&self) -> v14::Save {
        v14::Save {
            base: self.to_v13(),
            raft_rolls: self.save_raft_rolls(),
        }
    }

    pub(crate) fn from_v14(s: v14::Save, materials: MaterialTable) -> Result<Self, SaveError> {
        let rafts = &s.base.base.base.base.base.rafts;
        if s.raft_rolls.len() != rafts.len() {
            return Err(SaveError::Invalid(
                "raft roll count differs from raft count".into(),
            ));
        }
        let rolls: Vec<_> = rafts.iter().map(|r| r.plant).zip(s.raft_rolls).collect();
        let mut world = Self::from_v13(s.base, materials)?;
        world.load_raft_rolls(rolls);
        Ok(world)
    }

    /// The world as a version 15 snapshot.
    pub(crate) fn to_v15(&self) -> v15::Save {
        v15::Save {
            base: self.to_v14(),
            rest: self.save_rest(),
        }
    }

    /// A world restored from a version 15 snapshot, on `materials`.
    pub(crate) fn from_v15(s: v15::Save, materials: MaterialTable) -> Result<Self, SaveError> {
        let mut w = Self::from_v14(s.base, materials)?;
        w.load_rest(&s.rest)?;
        Ok(w)
    }

    pub(crate) fn to_v16(&self) -> v16::Save {
        v16::Save {
            base: self.to_v15(),
            coat: runs(self.coat().iter().copied()),
            front: runs(self.front_glass_algae().iter().copied()),
            back: runs(self.back_glass_algae().iter().copied()),
            sides: runs(self.side_glass_algae().iter().copied()),
        }
    }

    pub(crate) fn from_v16(s: v16::Save, materials: MaterialTable) -> Result<Self, SaveError> {
        let mut world = Self::from_v15(s.base, materials)?;
        let coat = unrun(&s.coat, world.cells.len(), "the surface coat")?;
        let front = unrun(&s.front, world.algae.front.len(), "the front glass film")?;
        let back = unrun(&s.back, world.algae.back.len(), "the back wall film")?;
        let sides = unrun(&s.sides, world.algae.sides.len(), "the side wall film")?;
        if front.iter().any(|&v| v > FRONT_CAP)
            || back
                .iter()
                .chain(sides.iter().flatten())
                .any(|&v| v > DENSITY_MASK)
        {
            return Err(SaveError::Invalid(
                "glass film exceeds its density cap".into(),
            ));
        }
        for (i, v) in coat.into_iter().enumerate().filter(|&(_, v)| v != 0) {
            // Only green film (variety 1) exists; 2-15 are reserved.
            // Exposure changes when objects move, so an otherwise eligible
            // cell may be temporarily dry or buried between field updates.
            if v & DENSITY_MASK == 0 || v >> 4 != 1 || !world.algae_surface(world.cells[i]) {
                return Err(SaveError::Invalid(format!("invalid film on cell {i}")));
            }
            world.write_coat(i, v);
        }
        world.algae.glass = front
            .iter()
            .chain(&back)
            .chain(sides.iter().flatten())
            .filter(|&&v| v != 0)
            .count();
        world.algae.front = front;
        world.algae.back = back;
        world.algae.sides = sides;
        Ok(world)
    }

    /// The old material disappears into water. A legacy cell represented
    /// volume in the water and could touch several different surfaces.
    pub(crate) fn migrate_algae(&mut self) {
        let (Some(algae), Some(water)) = (self.legacy_algae, self.life.water) else {
            return;
        };
        for i in 0..self.cells.len() {
            if self.cells[i].material == algae {
                self.write_cell(i % self.width, i / self.width, water, &mut None);
            }
        }
    }

    /// A world restored from a pre-v11 snapshot: resets driftwood cells'
    /// random colour-noise bits to dry (preserving glue).
    pub(crate) fn from_v10_migrated(
        s: v10::Save,
        materials: MaterialTable,
    ) -> Result<Self, SaveError> {
        let mut w = Self::from_v10(s, materials)?;
        w.migrate_driftwood_v10();
        Ok(w)
    }

    /// The world as a version 10 snapshot.
    pub(crate) fn to_v10(&self) -> v10::Save {
        v10::Save {
            base: self.to_v9(),
            masks: self.save_masks(),
            shapes: self.save_shape_masks(),
        }
    }

    /// A world restored from a version 10 snapshot, on `materials`.
    pub(crate) fn from_v10(s: v10::Save, materials: MaterialTable) -> Result<Self, SaveError> {
        Self::from_v10_with(s, materials, None)
    }

    /// [`World::from_v10`] with a version 12/13 device table.
    #[allow(clippy::type_complexity)]
    fn from_v10_with(
        s: v10::Save,
        materials: MaterialTable,
        devices: Option<(&[v12::Device], u16, Option<&[v13::DeviceFiltration]>)>,
    ) -> Result<Self, SaveError> {
        let mut w = Self::from_v9_with(s.base, materials, devices)?;
        w.load_masks(&s.masks)?;
        w.load_shape_masks(s.shapes)?;
        Ok(w)
    }

    /// The world as a version 9 snapshot.
    pub(crate) fn to_v9(&self) -> v9::Save {
        let base = |i: usize| variant(self.seed, 0, i);
        #[allow(clippy::cast_possible_truncation)] // sides ≤ MAX_SIDE
        v9::Save {
            width: self.width as u32,
            height: self.height as u32,
            seed: self.seed,
            tick: self.tick,
            last_change_tick: self.last_change_tick,
            surface_row: self.surface_row as u32,
            materials: self
                .materials
                .iter()
                .map(|m| named(m.id.0, &m.name))
                .collect(),
            plant_species: self
                .plant_species()
                .iter()
                .map(|s| named(s.id.0, &s.name))
                .collect(),
            species: self
                .species()
                .iter()
                .map(|s| named(s.id.0, &s.name))
                .collect(),
            morphs: self
                .species()
                .iter()
                .map(|s| v9::Morphs {
                    species: s.id.0,
                    names: s.morphs.clone(),
                })
                .collect(),
            grid: v1::Grid {
                materials: runs(self.cells.iter().map(|c| c.material.0)),
                variants: self
                    .cells
                    .iter()
                    .enumerate()
                    .map(|(i, c)| c.variant ^ base(i))
                    .collect(),
                states: runs(self.cells.iter().map(|c| c.state)),
            },
            chunks: self
                .chunks
                .iter()
                .map(|c| v1::Chunk {
                    work_next: rect_v1(c.work_next),
                    changed_next: rect_v1(c.changed_next),
                    idle_ticks: c.idle_ticks,
                    awake: c.awake,
                })
                .collect(),
            next_body: self.next_body,
            bodies: self.save_bodies(),
            life: self.save_life(),
            plants: self.save_plants(),
            creatures: self.save_creatures(),
            rafts: self.save_rafts(),
            colours: self.save_plant_colours(),
        }
    }

    /// A world restored from a version 9 snapshot, on `materials`.
    #[cfg(test)]
    pub(crate) fn from_v9(s: v9::Save, materials: MaterialTable) -> Result<Self, SaveError> {
        Self::from_v9_with(s, materials, None)
    }

    /// [`World::from_v9`] with a version 12/13 device table, restored before
    /// the life lists so the current is worked out with the devices.
    #[allow(clippy::type_complexity)]
    fn from_v9_with(
        s: v9::Save,
        materials: MaterialTable,
        devices: Option<(&[v12::Device], u16, Option<&[v13::DeviceFiltration]>)>,
    ) -> Result<Self, SaveError> {
        let config = WorldConfig {
            width: s.width as usize,
            height: s.height as usize,
            seed: s.seed,
            ..WorldConfig::default()
        };
        let mut w = Self::new(config, materials)?;
        w.check_names(&s)?;
        let (width, height) = (w.width, w.height);
        if s.surface_row as usize > height {
            return Err(SaveError::Invalid(format!(
                "surface row {} below the floor",
                s.surface_row
            )));
        }
        w.surface_row = s.surface_row as usize;
        w.tick = s.tick;
        #[allow(clippy::cast_possible_truncation)] // the stamp wraps, as in `tick`
        {
            w.stamp = s.tick as u32;
        }
        w.last_change_tick = s.last_change_tick;

        let n = width * height;
        let materials = unrun(&s.grid.materials, n, "the material plane")?;
        let states = unrun(&s.grid.states, n, "the state plane")?;
        if s.grid.variants.len() != n {
            return Err(SaveError::Invalid(format!(
                "{} variants for {n} cells",
                s.grid.variants.len()
            )));
        }
        for (i, ((&m, &state), &v)) in materials
            .iter()
            .zip(&states)
            .zip(&s.grid.variants)
            .enumerate()
        {
            let material = MaterialId(m);
            if w.materials.get(material).is_none() {
                return Err(SaveError::Invalid(format!("unknown material id {m}")));
            }
            w.cells[i] = Cell {
                material,
                variant: v ^ variant(w.seed, 0, i),
                state,
            };
        }

        if s.chunks.len() != w.chunks.len() {
            return Err(SaveError::Invalid(format!(
                "{} chunks, the grid has {}",
                s.chunks.len(),
                w.chunks.len()
            )));
        }
        for (c, saved) in w.chunks.iter_mut().zip(&s.chunks) {
            let inside = |r: v1::Rect| {
                let r = rect_from_v1(r, width, height)?;
                if r.intersect(c.bounds) == r {
                    Ok(r)
                } else {
                    Err(SaveError::Invalid(format!("chunk work {r:?} outside it")))
                }
            };
            c.work_next = inside(saved.work_next)?;
            c.changed_next = inside(saved.changed_next)?;
            c.work = Rect::EMPTY;
            c.changed = Rect::EMPTY;
            c.idle_ticks = saved.idle_ticks;
            c.awake = saved.awake;
        }

        w.next_body = s.next_body;
        w.load_bodies(s.bodies)?;
        if let Some((devices, next, filtration)) = devices {
            w.load_devices(devices, next, filtration)?;
        }
        w.load_life(s.life)?;
        w.load_plants(s.plants)?;
        w.load_rafts(s.rafts)?;
        w.load_plant_colours(&s.colours)?;
        w.load_creatures(s.creatures)?;
        Ok(w)
    }

    /// Every material and species id the save names must still name the
    /// same thing, and every species' morphs must start with the saved ones.
    fn check_names(&self, s: &v9::Save) -> Result<(), SaveError> {
        let check = |what: &str, saved: &[v1::Named], now: &dyn Fn(u8) -> Option<String>| {
            for e in saved {
                let current = now(e.id);
                if current.as_deref() != Some(e.name.as_str()) {
                    return Err(SaveError::Mismatch(format!(
                        "{what} {} was {:?}, now {}",
                        e.id,
                        e.name,
                        current.map_or_else(|| "unknown".to_owned(), |n| format!("{n:?}"))
                    )));
                }
            }
            Ok(())
        };
        check("material", &s.materials, &|id| {
            self.materials.get(MaterialId(id)).map(|m| m.name.clone())
        })?;
        check("plant species", &s.plant_species, &|id| {
            self.plant_species()
                .get(PlantSpeciesId(id))
                .map(|p| p.name.clone())
        })?;
        check("species", &s.species, &|id| {
            self.species().get(SpeciesId(id)).map(|p| p.name.clone())
        })?;
        for m in &s.morphs {
            let now = self.species().get(SpeciesId(m.species));
            if !now.is_some_and(|sp| sp.morphs.starts_with(&m.names)) {
                return Err(SaveError::Mismatch(format!(
                    "species {} morphs were {:?}, now {:?}",
                    m.species,
                    m.names,
                    now.map(|sp| &sp.morphs)
                )));
            }
        }
        Ok(())
    }

    /// Checks a saved cell index.
    pub(super) fn cell_index(&self, i: u32, what: &str) -> Result<usize, SaveError> {
        let i = i as usize;
        if i < self.cells.len() {
            Ok(i)
        } else {
            Err(SaveError::Invalid(format!(
                "{what} cell {i} outside the grid"
            )))
        }
    }

    /// The chunk holding cell `i`.
    pub(super) fn chunk_of(&self, i: usize) -> usize {
        let (x, y) = (i % self.width, i / self.width);
        (y / CHUNK_SIZE) * self.chunks_x + x / CHUNK_SIZE
    }
}

fn named(id: u8, name: &str) -> v1::Named {
    v1::Named {
        id,
        name: name.to_owned(),
    }
}

#[allow(clippy::cast_possible_truncation)] // within MAX_SIDE
pub(super) fn rect_v1(r: Rect) -> v1::Rect {
    v1::Rect {
        x0: r.x0 as u32,
        y0: r.y0 as u32,
        x1: r.x1 as u32,
        y1: r.y1 as u32,
    }
}

/// A saved rectangle, which must lie within a `width × height` grid (or be
/// empty).
pub(super) fn rect_from_v1(r: v1::Rect, width: usize, height: usize) -> Result<Rect, SaveError> {
    let rect = Rect {
        x0: r.x0 as usize,
        y0: r.y0 as usize,
        x1: r.x1 as usize,
        y1: r.y1 as usize,
    };
    if rect.is_empty() {
        return Ok(Rect::EMPTY);
    }
    if rect.x1 <= width && rect.y1 <= height {
        Ok(rect)
    } else {
        Err(SaveError::Invalid(format!(
            "rectangle {rect:?} outside the grid"
        )))
    }
}
