//! Placeables (`docs/spec/interaction-and-saves.md` §2.5, T5): prefab devices
//! the palette places, one per click (D27, [`crate::device`]).
//!
//! - The **bubbler** (DV7) is a bed-mounted prefab air-stone diffuser: it drops
//!   onto substrate, hardscape or the floor and emits a dense curtain of rising
//!   air bubbles across its porous face.
//! - The **internal filter** (DV1) goes on the nearer side glass at the clicked
//!   height, and its nozzle jets away from the glass. It replaced the bare
//!   filter outlet.
//! - The **sponge filter** (DV4) is a bed-mounted prefab device: it drops onto
//!   substrate, hardscape or the floor, emits rising uplift bubbles and gentle
//!   upward flow, is intrinsically fry-safe, and invites shrimp to graze it.
//! - The **HOB filter** (DV5) is a rim-mounted prefab device: it hangs on the
//!   top edge partly above the water line, pours a waterfall into the surface,
//!   and drops its intake tube in the back depth band so fish swim in front.
//! - The **under-gravel filter** (DV6) is a floor-mounted prefab device: it lies
//!   on the tank floor under any substrate already there (which settles on top
//!   of it), draws water down through the gravel and up its corner uplift tube,
//!   and is intrinsically fry-safe, but rooted plants over its plate decline.

use crate::device::DeviceKind;
use crate::{World, WorldError};

/// A device the palette can place.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Placeable {
    /// A bed-mounted air-stone diffuser bubbler: releases air bubbles.
    Bubbler,
    /// An internal filter on the side glass: makes a water current.
    InternalFilter,
    /// A bed-mounted air-driven sponge filter: gentle uplift, bubbles, fry-safe.
    SpongeFilter,
    /// A rim-mounted hang-on-back waterfall filter: ripples the surface, back-band intake.
    HobFilter,
    /// A floor-mounted under-gravel filter: slotted plate under the substrate, uplift tube.
    UnderGravelFilter,
}

impl Placeable {
    /// Every placeable, in palette order.
    pub const ALL: [Self; 5] = [
        Self::Bubbler,
        Self::InternalFilter,
        Self::SpongeFilter,
        Self::HobFilter,
        Self::UnderGravelFilter,
    ];

    /// The material its cells are made of.
    #[must_use]
    pub fn material_name(self) -> &'static str {
        "device"
    }

    /// Its name in scripts.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Bubbler => "bubbler",
            Self::InternalFilter => "internal_filter",
            Self::SpongeFilter => "sponge_filter",
            Self::HobFilter => "hob_filter",
            Self::UnderGravelFilter => "under_gravel_filter",
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

    /// The prefab device kind it places.
    #[must_use]
    pub fn device(self) -> Option<DeviceKind> {
        Some(match self {
            Self::Bubbler => DeviceKind::Bubbler,
            Self::InternalFilter => DeviceKind::InternalFilter,
            Self::SpongeFilter => DeviceKind::SpongeFilter,
            Self::HobFilter => DeviceKind::HobFilter,
            Self::UnderGravelFilter => DeviceKind::UnderGravelFilter,
        })
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

    /// Places `what` at cell `(x, y)` where [`World::device_placement`] puts it
    /// (`flip` mirrors flippable mounts), whole or not at all. Returns how many
    /// cells changed.
    ///
    /// # Errors
    ///
    /// Returns an error if the material table lacks the `device` material.
    pub fn place_flipped(
        &mut self,
        what: Placeable,
        x: i64,
        y: i64,
        flip: bool,
    ) -> Result<usize, WorldError> {
        let Some(kind) = what.device() else {
            return Ok(0);
        };
        Ok(match self.add_device(kind, x, y, flip)? {
            Some(_) => self.device_kinds().get(kind).cells,
            None => 0,
        })
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
        let bubbler_art = w.device_kinds().get(DeviceKind::Bubbler).clone();
        assert_eq!(
            w.place(Placeable::Bubbler, 20, 5).unwrap(),
            bubbler_art.cells
        );
        let device = w.materials().id("device").unwrap();
        let b = *w.devices().next().unwrap();
        assert_eq!(b.kind, DeviceKind::Bubbler);
        assert_eq!(b.y, 48 - bubbler_art.size.1);
        assert_eq!(w.cell(20, 47).unwrap().material, device);
        assert_eq!(w.device_at(20, 47), Some(b.id));
        let cells = w.device_kinds().get(DeviceKind::InternalFilter).cells;
        assert_eq!(w.place(Placeable::InternalFilter, 40, 20).unwrap(), cells);
        // On the right glass (x 40 of 64), jetting left.
        let d = *w
            .devices()
            .find(|d| d.kind == DeviceKind::InternalFilter)
            .unwrap();
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
