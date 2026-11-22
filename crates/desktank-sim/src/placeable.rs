//! Placeables (`docs/spec/interaction-and-saves.md` §2.5, T5): devices the palette places, one per click.
//!
//! - The **bubbler** (an air stone) drops from where it is placed to the
//!   first thing below it and releases air bubbles from its top. It is
//!   still a fixed footprint of its own solid cells (`assets/materials/`),
//!   scooped a cell at a time, until the prefab bubbler replaces it (DV7).
//! - The **internal filter** is a prefab device (D27, [`crate::device`]):
//!   it goes on the nearer side glass at the clicked height, and its nozzle
//!   jets away from the glass. It replaced the bare filter outlet (DV1).

use crate::device::DeviceKind;
use crate::{World, WorldError};

/// The bubbler's shape, top row first, centred on the placement point; `#`
/// is a device cell.
const BUBBLER: [&str; 2] = [".###.", "#####"];

/// A device the palette can place.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Placeable {
    /// Releases air bubbles.
    Bubbler,
    /// An internal filter on the side glass: makes a water current.
    InternalFilter,
}

impl Placeable {
    /// Every placeable, in palette order.
    pub const ALL: [Self; 2] = [Self::Bubbler, Self::InternalFilter];

    /// The material its cells are made of.
    #[must_use]
    pub fn material_name(self) -> &'static str {
        match self {
            Self::Bubbler => "bubbler",
            Self::InternalFilter => "device",
        }
    }

    /// Its name in scripts.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Bubbler => "bubbler",
            Self::InternalFilter => "internal_filter",
        }
    }

    /// The placeable named `name`; `filter_outlet`, the device the
    /// internal filter replaced, names it too.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        if name == "filter_outlet" {
            return Some(Self::InternalFilter);
        }
        Self::ALL.into_iter().find(|p| p.name() == name)
    }

    /// The prefab device kind it places, if it is one.
    #[must_use]
    pub fn device(self) -> Option<DeviceKind> {
        match self {
            Self::Bubbler => None,
            Self::InternalFilter => Some(DeviceKind::InternalFilter),
        }
    }
}

impl World {
    /// Places `what` at cell `(x, y)` ([`World::place_flipped`], unflipped).
    ///
    /// # Errors
    ///
    /// As [`World::place_flipped`].
    pub fn place(&mut self, what: Placeable, x: i64, y: i64) -> Result<usize, WorldError> {
        self.place_flipped(what, x, y, false)
    }

    /// Places `what` at cell `(x, y)`. A bubbler is centred there, filling
    /// only fluid cells (like a pour), after dropping to rest on the first
    /// solid, substrate or edge below. A prefab device goes where
    /// [`World::device_placement`] puts it (`flip` mirrors it onto the
    /// other glass), whole or not at all. Returns how many cells changed.
    ///
    /// # Errors
    ///
    /// Returns an error if the material table lacks the device material.
    pub fn place_flipped(
        &mut self,
        what: Placeable,
        x: i64,
        y: i64,
        flip: bool,
    ) -> Result<usize, WorldError> {
        if let Some(kind) = what.device() {
            return Ok(match self.add_device(kind, x, y, flip)? {
                Some(_) => self.device_kinds().get(kind).cells,
                None => 0,
            });
        }
        let material = self
            .materials()
            .id(what.material_name())
            .ok_or(WorldError::MissingMaterial(what.material_name()))?;
        let shape = BUBBLER;
        let rows = i64::try_from(shape.len()).unwrap_or(0);
        let cols = i64::try_from(shape[0].len()).unwrap_or(0);
        let (x0, mut y0) = (x - cols / 2, y - rows / 2);
        let free = |w: &Self, cx: i64, cy: i64| match (usize::try_from(cx), usize::try_from(cy)) {
            (Ok(cx), Ok(cy)) => w.cell(cx, cy).is_some_and(|c| w.is_fluid(c.material)),
            _ => false,
        };
        let height = i64::try_from(self.height()).unwrap_or(0);
        // Drop while every cell under the bottom row is free.
        while y0 + rows < height && (x0..x0 + cols).all(|cx| free(self, cx, y0 + rows)) {
            y0 += 1;
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
    fn bubbler_drops_to_the_floor_and_the_filter_goes_on_the_glass() {
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
        let cells = w.device_kinds().get(DeviceKind::InternalFilter).cells;
        assert_eq!(w.place(Placeable::InternalFilter, 40, 20).unwrap(), cells);
        let device = w.materials().id("device").unwrap();
        // On the right glass (x 40 of 64), jetting left.
        let d = *w.devices().next().unwrap();
        assert!(d.facing_left);
        assert_eq!(d.x, 64 - 14);
        assert_eq!(w.cell(60, 20).unwrap().material, device);
        // A second one finds no room under the first's footprint.
        assert_eq!(w.place(Placeable::InternalFilter, 60, 20).unwrap(), 0);
        assert_eq!(Placeable::from_name("bubbler"), Some(Placeable::Bubbler));
        assert_eq!(
            Placeable::from_name("filter_outlet"),
            Some(Placeable::InternalFilter)
        );
    }
}
