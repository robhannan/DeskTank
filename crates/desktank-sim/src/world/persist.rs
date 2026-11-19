//! Snapshots of a world for the save format (T8, [`crate::save`]): the
//! world's own state here, each part's in its module (`bodies`, `life`,
//! `plants`, `creatures`).

use super::{CHUNK_SIZE, World, WorldConfig, variant};
use crate::grid::{Cell, Rect};
use crate::material::{MaterialId, MaterialTable};
use crate::plants::PlantSpeciesId;
use crate::save::{SaveError, runs, unrun, v1, v3};
use crate::species::SpeciesId;

impl World {
    /// The world as a version 3 snapshot.
    pub(crate) fn to_v3(&self) -> v3::Save {
        let base = |i: usize| variant(self.seed, 0, i);
        #[allow(clippy::cast_possible_truncation)] // sides ≤ MAX_SIDE
        v3::Save {
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
                .map(|s| v3::Morphs {
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
        }
    }

    /// A world restored from a version 3 snapshot, on `materials`.
    pub(crate) fn from_v3(s: v3::Save, materials: MaterialTable) -> Result<Self, SaveError> {
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
        w.load_life(s.life)?;
        w.load_plants(s.plants)?;
        w.load_creatures(s.creatures)?;
        Ok(w)
    }

    /// Every material and species id the save names must still name the
    /// same thing, and every species' morphs must start with the saved ones.
    fn check_names(&self, s: &v3::Save) -> Result<(), SaveError> {
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
