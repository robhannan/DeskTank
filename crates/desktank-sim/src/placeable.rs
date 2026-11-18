//! Placeables (SPEC §2.5, T5): devices stamped into the tank as a fixed
//! footprint of device cells, one per click.
//!
//! - The **bubbler** (an air stone) drops from where it is placed to the
//!   first thing below it and releases air bubbles from its top.
//! - The **filter outlet** stays where it is placed, like a nozzle on the
//!   glass, and its jet points away from the nearer side wall.
//!
//! Both are solid cells of their own material (`assets/materials/`), so
//! scooping removes them like any other cell and their behaviour follows the
//! cells (`assets/fields.toml`).

use crate::{World, WorldError};

/// A device the palette can place.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Placeable {
    /// Releases air bubbles.
    Bubbler,
    /// Makes a water current.
    FilterOutlet,
}

impl Placeable {
    /// Every placeable, in palette order.
    pub const ALL: [Self; 2] = [Self::Bubbler, Self::FilterOutlet];

    /// The material its cells are made of.
    #[must_use]
    pub fn material_name(self) -> &'static str {
        match self {
            Self::Bubbler => "bubbler",
            Self::FilterOutlet => "filter_outlet",
        }
    }

    /// The placeable whose material is `name`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.material_name() == name)
    }

    /// Its shape, top row first, centred on the placement point; `#` is a
    /// device cell.
    #[must_use]
    pub fn footprint(self) -> &'static [&'static str] {
        match self {
            Self::Bubbler => &[".###.", "#####"],
            Self::FilterOutlet => &["###", "###", "###"],
        }
    }

    /// Whether it drops onto whatever is below the placement point.
    fn drops(self) -> bool {
        self == Self::Bubbler
    }
}

impl World {
    /// Places `what` centred on cell `(x, y)`, filling only fluid cells
    /// (like a pour). A bubbler first drops to rest on the first solid,
    /// substrate or edge below. Returns how many cells changed.
    ///
    /// # Errors
    ///
    /// Returns an error if the material table lacks the device material.
    pub fn place(&mut self, what: Placeable, x: i64, y: i64) -> Result<usize, WorldError> {
        let material = self
            .materials()
            .id(what.material_name())
            .ok_or(WorldError::MissingMaterial(what.material_name()))?;
        let shape = what.footprint();
        let rows = i64::try_from(shape.len()).unwrap_or(0);
        let cols = i64::try_from(shape[0].len()).unwrap_or(0);
        let (x0, mut y0) = (x - cols / 2, y - rows / 2);
        let free = |w: &Self, cx: i64, cy: i64| match (usize::try_from(cx), usize::try_from(cy)) {
            (Ok(cx), Ok(cy)) => w.cell(cx, cy).is_some_and(|c| w.is_fluid(c.material)),
            _ => false,
        };
        if what.drops() {
            let height = i64::try_from(self.height()).unwrap_or(0);
            // Drop while every cell under the bottom row is free.
            while y0 + rows < height && (x0..x0 + cols).all(|cx| free(self, cx, y0 + rows)) {
                y0 += 1;
            }
        }
        let mut changed = 0;
        for (dy, row) in (0..).zip(shape.iter()) {
            for (dx, ch) in (0..).zip(row.bytes()) {
                let (cx, cy) = (x0 + dx, y0 + dy);
                if ch == b'#'
                    && free(self, cx, cy)
                    && let (Ok(cx), Ok(cy)) = (usize::try_from(cx), usize::try_from(cy))
                {
                    self.set_cell(cx, cy, material)?;
                    changed += 1;
                }
            }
        }
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MaterialTable, WorldConfig};

    #[test]
    fn bubbler_drops_to_the_floor_and_outlet_stays() {
        let config = WorldConfig {
            width: 64,
            height: 48,
            seed: 1,
            water_level: 1.0,
        };
        let mut w = World::new(config, MaterialTable::builtin().unwrap()).unwrap();
        assert_eq!(w.place(Placeable::Bubbler, 20, 5).unwrap(), 8);
        let bubbler = w.materials().id("bubbler").unwrap();
        assert_eq!(w.cell(20, 47).unwrap().material, bubbler);
        assert_eq!(w.cell(20, 46).unwrap().material, bubbler);
        assert_ne!(w.cell(18, 46).unwrap().material, bubbler);
        assert_eq!(w.place(Placeable::FilterOutlet, 40, 10).unwrap(), 9);
        let outlet = w.materials().id("filter_outlet").unwrap();
        assert_eq!(w.cell(39, 9).unwrap().material, outlet);
        // Placing over itself changes nothing.
        assert_eq!(w.place(Placeable::FilterOutlet, 40, 10).unwrap(), 0);
        assert_eq!(Placeable::from_name("bubbler"), Some(Placeable::Bubbler));
    }
}
