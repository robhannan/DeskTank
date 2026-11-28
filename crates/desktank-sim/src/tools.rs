//! Interact tools (SPEC §2.5): pour, scoop, glue, net, inspect and
//! placing devices, driven by pointer events in cell coordinates.
//!
//! This is the platform-free half of interaction, shared by every host
//! (`desktank window`, the web, later the overlay): a host maps its pointer
//! to cells (`desktank_render::Layout::cell_at`), hit-tests its palette
//! (`desktank_render::PaletteLayout`), and feeds the rest here. Strokes are
//! interpolated between pointer samples, so a fast drag leaves no gaps.
//!
//! A pour stroke of a rigid material (rock) makes one body, held still
//! while the button is down; it drops when released (D17). Glue toggles
//! the bodies under the brush between glued (static) and free.
//!
//! The net catches the creature under the pointer and carries it while the
//! button is down; letting go above the water (or outside the tank) lifts it
//! out. Inspect reports the creature under the pointer, if any, or the
//! plant, and the cell's material. A creature slot in the palette adds one
//! of its species per click. A plant slot (one per species, like the
//! creatures) uses its species as the start picked for it with
//! [`Tools::set_plant_start`] (seeds, cuttings, fragments, runners or
//! buds, or a whole juvenile or mature plant; the species' default
//! propagule until one is picked): seeds pour as powder with the brush, any
//! other propagule places one small piece of the species' real plant cells
//! per click ([`World::place_propagule`]), and a juvenile or mature start
//! places a whole plant, snapped to where it can stand and flippable
//! ([`World::place_plant`], [`Tools::plant_ghost`]).
//!
//! The palette is a two-level tree ([`PaletteTab`], [`Tools::palette_tree`];
//! the categories are [`crate::palette`]'s): hosts show the top tabs, the
//! open tab's sub-tabs (when it has more than one) and the open
//! sub-category's items, so every slot stays a short reach (and a number
//! key) away.

use std::collections::BTreeMap;

pub use crate::CreatureId;
pub use crate::palette::{Holds, PalettePath};
use crate::{
    BodyId, Brush, MaterialId, Placeable, PlantGhost, PlantId, PlantSpeciesId, PlantStage,
    PlantStart, Propagule, SpeciesId, World, WorldError,
};

/// Smallest brush radius in cells (a single cell).
pub const MIN_RADIUS: u32 = 0;
/// Largest brush radius in cells.
pub const MAX_RADIUS: u32 = 24;
/// Brush radius a new [`Tools`] starts with.
pub const DEFAULT_RADIUS: u32 = 3;

/// Available magnification factors for [`Tool::Magnify`]. All are even so
/// `grid.scale * zoom` stays divisible by four at every even cell scale
/// (keeping 4×4 fine plant and sprite art) and cell centres land on whole
/// pixels even at scale 1.
pub const ZOOM_LEVELS: [u32; 4] = [2, 4, 6, 8];
/// Magnification factor a new [`Tools`] starts with.
pub const DEFAULT_ZOOM: u32 = ZOOM_LEVELS[0];

/// A top category of the palette and its non-empty sub-categories, as
/// [`Tools::palette_tree`] gives them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteTab {
    /// Its name (`"fish"`).
    pub name: &'static str,
    /// Its label (`"Fish"`).
    pub label: &'static str,
    /// Its sub-categories with items, in palette order. Hosts show a
    /// sub-tab row only when there are several.
    pub subs: Vec<PaletteSub>,
}

/// A sub-category of the palette and its items.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteSub {
    /// Where it is in the tree (its name and label).
    pub path: PalettePath,
    /// Its items, in order.
    pub items: Vec<PaletteItem>,
}

/// What the primary button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    /// Pour the selected material into fluid cells.
    Pour,
    /// Erase: air above the water line, water below.
    Scoop,
    /// Glue rigid bodies in place, or free them (D17).
    Glue,
    /// Pick up a creature and move it.
    Net,
    /// Show what is under the pointer.
    Inspect,
    /// Zoom in on a region around the pointer.
    Magnify,
    /// Place a device, once per click.
    Place(Placeable),
    /// Add a creature of this species, once per click.
    AddCreature(SpeciesId),
    /// Use this plant species as the propagule picked for it
    /// ([`Tools::plant_propagule`]): pour seeds, or place one piece.
    Plant(PlantSpeciesId),
}

/// One entry in the palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PaletteItem {
    /// Select this material and the pour tool.
    Material(MaterialId),
    /// Select this tool (never [`Tool::Pour`]; pick a material for that).
    Tool(Tool),
}

/// A pointer button, as the host maps it: left (or a one-finger touch) is
/// primary, right (or two fingers) secondary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    /// Uses the selected tool.
    Primary,
    /// Always scoops.
    Secondary,
}

/// What inspect found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inspection {
    /// A cell and its material.
    Cell {
        /// Column.
        x: usize,
        /// Row.
        y: usize,
        /// The material there.
        material: MaterialId,
    },
    /// A plant, and the cell under the pointer (one of its cells).
    Plant {
        /// The plant.
        id: PlantId,
        /// Column under the pointer.
        x: usize,
        /// Row under the pointer.
        y: usize,
        /// The material there.
        material: MaterialId,
    },
    /// A device (D27), and the cell under the pointer (one of its cells).
    Device {
        /// The device.
        id: crate::DeviceId,
        /// Column under the pointer.
        x: usize,
        /// Row under the pointer.
        y: usize,
    },
    /// A creature, and the cell under the pointer.
    Creature {
        /// The creature.
        id: CreatureId,
        /// Its species.
        species: SpeciesId,
        /// Column under the pointer.
        x: usize,
        /// Row under the pointer.
        y: usize,
        /// The material there.
        material: MaterialId,
    },
}

/// Why adding a creature with [`Tool::AddCreature`] was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddRefused {
    /// The tank is already at its population cap (`cap` stocked creatures,
    /// or [`crate::MAX_LIVE_FOOD`] live foods when `live_food` is true).
    PopulationCap {
        /// The limit that was reached.
        cap: usize,
        /// Whether the species is live food (capped at [`crate::MAX_LIVE_FOOD`]
        /// rather than [`World::creature_cap`]).
        live_food: bool,
    },
    /// No free water near the pointer to fit the creature.
    NoRoom,
}

/// The result of a pointer event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing visible changed.
    None,
    /// The brush changed this many cells.
    Painted(usize),
    /// Inspect found something to show.
    Inspected(Inspection),
    /// Glue changed this many bodies.
    Glued(usize),
    /// The net caught this creature.
    Caught(CreatureId),
    /// The net let this creature go in the water.
    Dropped(CreatureId),
    /// The net lifted this creature out of the tank.
    Removed(CreatureId),
    /// A creature was added.
    Added(CreatureId),
    /// Adding a creature was refused (tank full or no room there).
    AddRefused {
        /// The species the tool tried to add.
        species: SpeciesId,
        /// Why it was refused.
        reason: AddRefused,
    },
    /// The magnify tool cycled to this magnification factor.
    Zoomed(u32),
}

/// A stroke in progress: which button, what it does, where it last was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stroke {
    button: Button,
    action: Action,
    last: (i64, i64),
    /// The rigid body a pour stroke is filling.
    body: Option<BodyId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Pour,
    /// Pour a plant species' seeds.
    Plant,
    Scoop,
    /// Glue (`Some(true)`) or free bodies; `None` until the stroke first
    /// touches a body, which it toggles, setting the way for the rest.
    Glue(Option<bool>),
    /// Carrying a creature, held by the cell offset from the pointer to its
    /// body centre (or nothing: the hit test missed).
    Net(Option<(CreatureId, (i64, i64))>),
    Inspect,
    /// Panning or clicking the magnifier; a click without moving cycles zoom
    /// on release.
    Magnify {
        moved: bool,
    },
    /// Placed a device or added a creature on press; the rest of the drag
    /// does nothing.
    Place,
}

/// The tool state machine: the selected tool and material, the brush size,
/// the pointer and any stroke in progress.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tools {
    tool: Tool,
    material: MaterialId,
    /// The start picked for each plant species (none: the species'
    /// default propagule). One a species doesn't offer (a different world's
    /// species table) falls back to its default ([`Tools::plant_start`]).
    plant_picks: BTreeMap<PlantSpeciesId, PlantStart>,
    /// The explicit morph picked for each creature species.
    morphs: BTreeMap<SpeciesId, u8>,
    radius: u32,
    /// Index into [`ZOOM_LEVELS`] for [`Tool::Magnify`].
    zoom: usize,
    pointer: Option<(i64, i64)>,
    stroke: Option<Stroke>,
    /// Mirror the next placed device onto the other glass.
    flip: bool,
}

impl Tools {
    /// Pour `material` with the default brush.
    #[must_use]
    pub fn new(material: MaterialId) -> Self {
        Self {
            tool: Tool::Pour,
            material,
            plant_picks: BTreeMap::new(),
            morphs: BTreeMap::new(),
            radius: DEFAULT_RADIUS,
            zoom: 0,
            pointer: None,
            stroke: None,
            flip: false,
        }
    }

    /// Whether the next device placed is mirrored onto the other glass.
    #[must_use]
    pub fn flipped(&self) -> bool {
        self.flip
    }

    /// Toggles [`Tools::flipped`]; returns the new setting.
    pub fn flip(&mut self) -> bool {
        self.flip = !self.flip;
        self.flip
    }

    /// Whether the selected tool places something that supports flipping: a
    /// device that mounts on either side glass, or a whole plant (juvenile
    /// or mature), which it mirrors left to right.
    #[must_use]
    pub fn can_flip(&self, world: &World) -> bool {
        match self.tool {
            Tool::Place(p) => p
                .device()
                .is_some_and(|kind| world.device_kinds().get(kind).mount.flippable()),
            Tool::Plant(_) => self.plant_start(world).is_some_and(|p| p.stage().is_some()),
            _ => false,
        }
    }

    /// The whole plant a click would put down at the pointer, snapped to
    /// where it can stand ([`World::plant_placement`]), if a plant slot
    /// is selected with a juvenile or mature start. Its cells show where
    /// the plant would go, and `fits` whether it can.
    #[must_use]
    pub fn plant_ghost(&self, world: &World) -> Option<PlantGhost> {
        let species = self.selected_plant()?;
        let stage = self.plant_start(world)?.stage()?;
        let (x, y) = self.pointer?;
        world.plant_placement(species, stage, x, y, self.flip)
    }

    /// The ghost of the device the place tool would put down at the
    /// pointer, and whether it fits there ([`World::device_placement`]).
    #[must_use]
    pub fn ghost(&self, world: &World) -> Option<(crate::Device, bool)> {
        let Tool::Place(p) = self.tool else {
            return None;
        };
        let kind = p.device()?;
        let (x, y) = self.pointer?;
        Some(world.device_placement(kind, x, y, self.flip))
    }

    /// The whole rigid ornament a click would put down at the pointer.
    #[must_use]
    pub fn ornament_ghost(&self, world: &World) -> Option<crate::ornament::OrnamentGhost> {
        let Tool::Place(p) = self.tool else {
            return None;
        };
        if p.device().is_some() {
            return None;
        }
        let material = world.materials().id(p.material_name())?;
        let (x, y) = self.pointer?;
        world.ornament_placement(material, x, y)
    }

    /// The palette for `world` as a tree: the top categories in
    /// [`PalettePath::all`] order, each with its non-empty sub-categories
    /// (top categories with none are left out). Tools are scoop, net,
    /// inspect, glue and magnify; devices the placeables whose material `world`
    /// has; the rest are the materials, plant species and creature species
    /// whose TOML names the sub-category (`palette = "…"`), each kind by
    /// `palette_order`, then ID. Assets naming no sub-category aren't offered (the cheat-only
    /// species, plant tissue, air, device cells).
    #[must_use]
    pub fn palette_tree(world: &World) -> Vec<PaletteTab> {
        let materials = world.materials();
        let mut tabs: Vec<PaletteTab> = Vec::new();
        for path in PalettePath::all() {
            let items: Vec<_> = match path.holds() {
                Holds::Tools => [
                    Tool::Scoop,
                    Tool::Net,
                    Tool::Inspect,
                    Tool::Glue,
                    Tool::Magnify,
                ]
                .map(PaletteItem::Tool)
                .to_vec(),
                Holds::Devices | Holds::Ornaments => Placeable::ALL
                    .into_iter()
                    .filter(|p| match p.device() {
                        Some(_) => {
                            path.holds() == Holds::Devices
                                && materials.id(p.material_name()).is_some()
                        }
                        None => materials
                            .id(p.material_name())
                            .and_then(|id| materials.get(id))
                            .is_some_and(|m| m.palette == Some(path)),
                    })
                    .map(|p| PaletteItem::Tool(Tool::Place(p)))
                    .collect(),
                Holds::Materials => {
                    let mut named: Vec<_> = materials
                        .iter()
                        .filter(|m| m.palette == Some(path))
                        .collect();
                    named.sort_by_key(|m| (m.palette_order, m.id));
                    named
                        .into_iter()
                        .map(|m| PaletteItem::Material(m.id))
                        .collect()
                }
                Holds::Plants => world
                    .plant_species()
                    .iter()
                    .filter(|s| s.palette == Some(path))
                    .map(|s| PaletteItem::Tool(Tool::Plant(s.id)))
                    .collect(),
                // Cheat-only species name no category (their load checks).
                Holds::Creatures => world
                    .species()
                    .iter()
                    .filter(|s| s.palette == Some(path))
                    .map(|s| PaletteItem::Tool(Tool::AddCreature(s.id)))
                    .collect(),
            };
            if items.is_empty() {
                continue;
            }
            let sub = PaletteSub { path, items };
            match tabs.last_mut() {
                Some(tab) if tab.name == path.top_name() => tab.subs.push(sub),
                _ => tabs.push(PaletteTab {
                    name: path.top_name(),
                    label: path.top_label(),
                    subs: vec![sub],
                }),
            }
        }
        tabs
    }

    /// The palette for `world` as one list: [`Tools::palette_tree`]'s
    /// items, sub-category after sub-category.
    #[must_use]
    pub fn palette(world: &World) -> Vec<PaletteItem> {
        Self::palette_tree(world)
            .into_iter()
            .flat_map(|tab| tab.subs)
            .flat_map(|sub| sub.items)
            .collect()
    }

    /// The selected tool.
    #[must_use]
    pub fn tool(&self) -> Tool {
        self.tool
    }

    /// The material pour uses.
    #[must_use]
    pub fn material(&self) -> MaterialId {
        self.material
    }

    /// The species of the selected creature slot, if one is selected.
    #[must_use]
    pub fn selected_creature(&self) -> Option<SpeciesId> {
        match self.tool {
            Tool::AddCreature(s) => Some(s),
            _ => None,
        }
    }

    /// The explicit morph for a multi-morph species, if the pick is valid in this world.
    #[must_use]
    pub fn morph_for(&self, world: &World, species: SpeciesId) -> Option<u8> {
        let count = world.species().get(species)?.morphs.len();
        self.morphs
            .get(&species)
            .copied()
            .filter(|&m| count > 1 && usize::from(m) < count)
    }

    /// The explicit morph for the selected creature slot, if any.
    #[must_use]
    pub fn creature_morph(&self, world: &World) -> Option<u8> {
        self.morph_for(world, self.selected_creature()?)
    }

    /// Sets or clears the selected species' morph pick. Returns whether the
    /// selected slot has multiple morphs and the pick is valid.
    pub fn set_creature_morph(&mut self, world: &World, morph: Option<u8>) -> bool {
        let Some(species) = self.selected_creature() else {
            return false;
        };
        let valid = world.species().get(species).is_some_and(|s| {
            s.morphs.len() > 1 && morph.is_none_or(|m| usize::from(m) < s.morphs.len())
        });
        if valid {
            if let Some(m) = morph {
                self.morphs.insert(species, m);
            } else {
                self.morphs.remove(&species);
            }
        }
        valid
    }

    /// Advances the selected creature's morph, including automatic stocking
    /// (`Some(None)`) for species that cycle. Returns `None` if the slot
    /// doesn't have multiple morphs.
    pub fn next_creature_morph(&mut self, world: &World) -> Option<Option<u8>> {
        let species = self.selected_creature()?;
        let s = world.species().get(species)?;
        let count = s.morphs.len();
        if count <= 1 {
            return None;
        }
        let current = self.morph_for(world, species);
        let next = if s.cycle_palette {
            let index = current.map_or(0, |m| usize::from(m) + 1);
            (index < count).then_some(u8::try_from(index).ok()?)
        } else {
            let index = (usize::from(current.unwrap_or(s.palette_morph)) + 1) % count;
            Some(u8::try_from(index).ok()?)
        };
        self.set_creature_morph(world, next);
        Some(next)
    }

    /// Copies per-species morph picks when a host replaces its tank.
    pub fn keep_creature_picks(&mut self, other: &Self) {
        self.morphs.clone_from(&other.morphs);
    }

    /// The plant species the selected plant slot pours, if one is
    /// selected.
    #[must_use]
    pub fn selected_plant(&self) -> Option<PlantSpeciesId> {
        match self.tool {
            Tool::Plant(s) => Some(s),
            _ => None,
        }
    }

    /// The start `species` places, as `world` resolves it: the one picked
    /// for it if it offers it ([`Tools::plant_starts`]), else its
    /// [`crate::PlantSpecies::default_propagule`]. `None` for a species
    /// `world` lacks.
    #[must_use]
    pub fn start_for(&self, world: &World, species: PlantSpeciesId) -> Option<PlantStart> {
        let s = world.plant_species().get(species)?;
        Some(
            self.plant_picks
                .get(&species)
                .copied()
                .filter(|&p| match p {
                    PlantStart::Propagule(p) => s.grows_from(p),
                    PlantStart::Juvenile | PlantStart::Mature => true,
                })
                .unwrap_or_else(|| PlantStart::Propagule(s.default_propagule())),
        )
    }

    /// The starts the selected plant slot offers, in the order [`Tools::next_plant_start`]
    /// cycles them: the species' own propagules, then a juvenile and a mature
    /// plant. Empty if no plant slot is selected.
    #[must_use]
    pub fn plant_starts(&self, world: &World) -> Vec<PlantStart> {
        let Some(s) = self
            .selected_plant()
            .and_then(|species| world.plant_species().get(species))
        else {
            return Vec::new();
        };
        s.propagules()
            .map(PlantStart::Propagule)
            .chain([PlantStart::Juvenile, PlantStart::Mature])
            .collect()
    }

    /// The start the selected plant slot places ([`Tools::start_for`]), or
    /// `None` if no plant slot is selected.
    #[must_use]
    pub fn plant_start(&self, world: &World) -> Option<PlantStart> {
        self.start_for(world, self.selected_plant()?)
    }

    /// Picks the start the selected plant slot places, if its species offers
    /// it. Returns whether it was taken; the pick is kept for that species.
    pub fn set_plant_start(&mut self, world: &World, start: PlantStart) -> bool {
        let Some(species) = self.selected_plant() else {
            return false;
        };
        let ok = self.plant_starts(world).contains(&start);
        if ok {
            self.plant_picks.insert(species, start);
        }
        ok
    }

    /// Picks the next start the selected plant's species offers (wrapping).
    /// Returns it, or `None` if no plant slot is selected.
    pub fn next_plant_start(&mut self, world: &World) -> Option<PlantStart> {
        let species = self.selected_plant()?;
        let starts = self.plant_starts(world);
        let current = self.start_for(world, species)?;
        let next = starts
            .iter()
            .position(|&p| p == current)
            .and_then(|k| starts.get(k + 1))
            .or_else(|| starts.first())
            .copied()?;
        self.plant_picks.insert(species, next);
        Some(next)
    }

    /// The propagule `species` pours: the one picked for it, or its default
    /// when it places a whole plant. `None` for a species `world` lacks.
    #[must_use]
    pub fn propagule_for(&self, world: &World, species: PlantSpeciesId) -> Option<Propagule> {
        match self.start_for(world, species)? {
            PlantStart::Propagule(p) => Some(p),
            PlantStart::Juvenile | PlantStart::Mature => world
                .plant_species()
                .get(species)
                .map(crate::PlantSpecies::default_propagule),
        }
    }

    /// The propagule the selected plant slot places ([`Tools::plant_start`]),
    /// or `None` if no plant slot is selected or it places a whole plant.
    #[must_use]
    pub fn plant_propagule(&self, world: &World) -> Option<Propagule> {
        match self.plant_start(world)? {
            PlantStart::Propagule(p) => Some(p),
            PlantStart::Juvenile | PlantStart::Mature => None,
        }
    }

    /// Picks the propagule the selected plant slot places, if its species
    /// grows from it. Returns whether it was taken; the pick is kept for
    /// that species.
    pub fn set_plant_propagule(&mut self, world: &World, propagule: Propagule) -> bool {
        self.set_plant_start(world, PlantStart::Propagule(propagule))
    }

    /// Picks the next propagule the selected plant's species grows from
    /// (wrapping, skipping whole plants). Returns it, or `None` if no plant
    /// slot is selected.
    pub fn next_plant_propagule(&mut self, world: &World) -> Option<Propagule> {
        let species = self.selected_plant()?;
        let s = world.plant_species().get(species)?;
        let current = self.propagule_for(world, species)?;
        let next = match self.start_for(world, species)? {
            PlantStart::Propagule(_) => s
                .propagules()
                .find(|&p| p > current)
                .or_else(|| s.propagules().next())?,
            // From a whole plant, the first propagule.
            PlantStart::Juvenile | PlantStart::Mature => s.propagules().next()?,
        };
        self.plant_picks
            .insert(species, PlantStart::Propagule(next));
        Some(next)
    }

    /// Takes `other`'s plant picks as they are (even ones their species
    /// doesn't offer), for a host replacing its tank. Plant species ids are
    /// stable across worlds.
    pub fn keep_plant_picks(&mut self, other: &Self) {
        self.plant_picks.clone_from(&other.plant_picks);
    }

    /// Brush radius in cells.
    #[must_use]
    pub fn radius(&self) -> u32 {
        self.radius
    }

    /// Magnification factor for [`Tool::Magnify`] (one of [`ZOOM_LEVELS`]).
    #[must_use]
    pub fn zoom(&self) -> u32 {
        ZOOM_LEVELS[self.zoom]
    }

    /// Picks the closest magnification factor in [`ZOOM_LEVELS`] to `zoom`.
    /// Returns whether it changed.
    pub fn set_zoom(&mut self, zoom: u32) -> bool {
        let best = ZOOM_LEVELS
            .iter()
            .enumerate()
            .min_by_key(|(_, level)| level.abs_diff(zoom))
            .map_or(0, |(i, _)| i);
        let changed = best != self.zoom;
        self.zoom = best;
        changed
    }

    /// Steps the magnification level up (positive) or down (negative) within
    /// [`ZOOM_LEVELS`]. Returns whether it changed.
    pub fn step_zoom(&mut self, steps: i32) -> bool {
        let last = i64::try_from(ZOOM_LEVELS.len() - 1).unwrap_or(0);
        let next = (i64::try_from(self.zoom).unwrap_or(0) + i64::from(steps)).clamp(0, last);
        let next = usize::try_from(next).unwrap_or(0);
        let changed = next != self.zoom;
        self.zoom = next;
        changed
    }

    /// Advances to the next magnification level in [`ZOOM_LEVELS`] (wrapping)
    /// and returns it.
    pub fn cycle_zoom(&mut self) -> u32 {
        self.zoom = (self.zoom + 1) % ZOOM_LEVELS.len();
        self.zoom()
    }

    /// The magnified lens to show at the pointer, `(x, y, zoom)`, while
    /// [`Tool::Magnify`] is active and no secondary scoop stroke is running.
    #[must_use]
    pub fn magnifier(&self) -> Option<(i64, i64, u32)> {
        let active = match self.stroke {
            Some(s) => matches!(s.action, Action::Magnify { .. }),
            None => self.tool == Tool::Magnify,
        };
        let (x, y) = self.pointer?;
        active.then_some((x, y, self.zoom()))
    }

    /// The pointer cell, if the pointer is over the tank.
    #[must_use]
    pub fn pointer(&self) -> Option<(i64, i64)> {
        self.pointer
    }

    /// Whether `item` is the current selection.
    #[must_use]
    pub fn is_selected(&self, item: PaletteItem) -> bool {
        match item {
            PaletteItem::Material(m) => self.tool == Tool::Pour && m == self.material,
            PaletteItem::Tool(t) => self.tool == t,
        }
    }

    /// The brush outline to preview at the pointer, `(x, y, radius)` in
    /// cells: for pour, scoop and glue, seeds, or any stroke that uses the
    /// brush. A plant slot placing a piece (anything but seeds) has none.
    #[must_use]
    pub fn preview(&self, world: &World) -> Option<(i64, i64, u32)> {
        let painting = match self.stroke {
            Some(s) => matches!(
                s.action,
                Action::Pour | Action::Plant | Action::Scoop | Action::Glue(_)
            ),
            None => match self.tool {
                Tool::Pour | Tool::Scoop | Tool::Glue => true,
                // Only seeds pour with the brush; a placed piece or whole
                // plant has none.
                Tool::Plant(_) => self.plant_propagule(world) == Some(Propagule::Seeds),
                _ => false,
            },
        };
        let (x, y) = self.pointer?;
        painting.then_some((x, y, self.radius))
    }

    /// Selects a palette item. Ends any stroke in progress.
    pub fn select(&mut self, item: PaletteItem) {
        match item {
            PaletteItem::Material(m) => {
                self.material = m;
                self.tool = Tool::Pour;
            }
            PaletteItem::Tool(t) => self.tool = t,
        }
        self.stroke = None;
    }

    /// Grows (positive) or shrinks the brush by `steps` cells of radius,
    /// within [`MIN_RADIUS`]..=[`MAX_RADIUS`]. Returns whether it changed.
    pub fn resize(&mut self, steps: i32) -> bool {
        let r = i64::from(self.radius) + i64::from(steps);
        let r = u32::try_from(r.clamp(i64::from(MIN_RADIUS), i64::from(MAX_RADIUS)))
            .unwrap_or(DEFAULT_RADIUS);
        let changed = r != self.radius;
        self.radius = r;
        changed
    }

    /// A button went down at `cell`. Ignored while another button's stroke
    /// is in progress, so that stroke survives the second button.
    pub fn press(&mut self, world: &mut World, button: Button, cell: (i64, i64)) -> Outcome {
        self.pointer = Some(cell);
        if self.stroke.is_some() {
            return Outcome::None;
        }
        let action = match (button, self.tool) {
            (Button::Secondary, _) | (Button::Primary, Tool::Scoop) => Action::Scoop,
            (Button::Primary, Tool::Pour) => Action::Pour,
            (Button::Primary, Tool::Plant(_)) => match self.plant_start(world) {
                // Seeds pour as powder with the brush; any other
                // propagule is one placed piece per click, and a whole
                // plant one plant.
                Some(PlantStart::Propagule(Propagule::Seeds)) | None => Action::Plant,
                Some(_) => Action::Place,
            },
            (Button::Primary, Tool::Glue) => Action::Glue(None),
            (Button::Primary, Tool::Net) => {
                Action::Net(world.creature_at(cell.0, cell.1).and_then(|id| {
                    let (x, y) = world.creature(id)?.cell();
                    Some((id, (x - cell.0, y - cell.1)))
                }))
            }
            (Button::Primary, Tool::Inspect) => Action::Inspect,
            (Button::Primary, Tool::Magnify) => Action::Magnify { moved: false },
            (Button::Primary, Tool::Place(_) | Tool::AddCreature(_)) => Action::Place,
        };
        self.stroke = Some(Stroke {
            button,
            action,
            last: cell,
            body: None,
        });
        match action {
            Action::Pour | Action::Plant | Action::Scoop => {
                Outcome::Painted(self.stamp(world, cell))
            }
            Action::Glue(_) => Outcome::Glued(self.glue(world, cell)),
            Action::Inspect => inspect(world, cell),
            Action::Place => match self.tool {
                // The device material is in the table: the palette only
                // offers placeables it has.
                Tool::Place(p) => Outcome::Painted(
                    world
                        .place_flipped(p, cell.0, cell.1, self.flip)
                        .unwrap_or(0),
                ),
                // One piece per click; a full tank or no open water there
                // places nothing.
                Tool::Plant(species) => self
                    .plant_start(world)
                    .and_then(|start| match start {
                        PlantStart::Propagule(p) => world
                            .place_propagule(cell.0, cell.1, species, p)
                            .ok()
                            .flatten(),
                        PlantStart::Juvenile => {
                            self.place_whole(world, species, PlantStage::Juvenile, cell)
                        }
                        PlantStart::Mature => {
                            self.place_whole(world, species, PlantStage::Mature, cell)
                        }
                    })
                    .map_or(Outcome::None, |id| {
                        Outcome::Painted(world.plant(id).map_or(1, |p| p.size() as usize))
                    }),
                // No room (solid/air there) or a full tank reports why.
                Tool::AddCreature(s) => {
                    match world.add_palette_creature(s, cell.0, cell.1, self.morph_for(world, s)) {
                        Ok(id) => Outcome::Added(id),
                        Err(WorldError::PopulationCap(cap)) => Outcome::AddRefused {
                            species: s,
                            reason: AddRefused::PopulationCap {
                                cap,
                                live_food: world.species().get(s).is_some_and(|sp| sp.live_food),
                            },
                        },
                        Err(WorldError::NoRoom) => Outcome::AddRefused {
                            species: s,
                            reason: AddRefused::NoRoom,
                        },
                        Err(_) => Outcome::None,
                    }
                }
                _ => Outcome::None,
            },
            Action::Net(Some((id, _))) => {
                world.hold_creature(id);
                Outcome::Caught(id)
            }
            Action::Net(None) | Action::Magnify { .. } => Outcome::None,
        }
    }

    /// The pointer moved to `cell`. A paint stroke covers every cell on the
    /// way from the last sample.
    pub fn moved(&mut self, world: &mut World, cell: (i64, i64)) -> Outcome {
        self.pointer = Some(cell);
        let Some(mut stroke) = self.stroke else {
            return Outcome::None;
        };
        let from = stroke.last;
        stroke.last = cell;
        if let Action::Magnify { ref mut moved } = stroke.action
            && from != cell
        {
            *moved = true;
        }
        self.stroke = Some(stroke);
        match stroke.action {
            Action::Glue(_) => {
                let mut changed = 0;
                for p in line(from, cell).skip(1) {
                    changed += self.glue(world, p);
                }
                Outcome::Glued(changed)
            }
            Action::Pour | Action::Plant | Action::Scoop => {
                // Discs half a radius apart still overlap into a solid band,
                // and a wide brush costs a fraction of a disc per cell.
                let stride = usize::try_from(self.radius / 2).unwrap_or(1).max(1);
                let mut changed = 0;
                let mut points = line(from, cell).skip(1).peekable();
                let mut i = 0;
                while let Some(p) = points.next() {
                    i += 1;
                    if i % stride == 0 || points.peek().is_none() {
                        changed += self.stamp(world, p);
                    }
                }
                Outcome::Painted(changed)
            }
            Action::Inspect if from != cell => inspect(world, cell),
            // Carrying a creature moves it with the pointer, wherever its
            // body fits (it stays at the edge of the water while the
            // pointer is above it).
            Action::Net(Some((id, (ox, oy)))) => {
                world.move_creature(id, cell.0 + ox, cell.1 + oy);
                world.hold_creature(id);
                Outcome::None
            }
            Action::Net(None) | Action::Inspect | Action::Magnify { .. } | Action::Place => {
                Outcome::None
            }
        }
    }

    /// `button` was released. Ends the stroke it started. A netted creature
    /// is let go where it is, or lifted out of the tank if the pointer is
    /// above the water or outside the grid. A click (without crossing into
    /// another simulation cell) with [`Tool::Magnify`] cycles the
    /// magnification level.
    pub fn release(&mut self, world: &mut World, button: Button) -> Outcome {
        let Some(stroke) = self.stroke.filter(|s| s.button == button) else {
            return Outcome::None;
        };
        self.stroke = None;
        if matches!(stroke.action, Action::Magnify { moved: false }) {
            return Outcome::Zoomed(self.cycle_zoom());
        }
        let Action::Net(Some((id, _))) = stroke.action else {
            return Outcome::None;
        };
        let (x, y) = stroke.last;
        let surface = i64::try_from(world.surface_row()).unwrap_or(i64::MAX);
        let inside = |v: i64, len: usize| usize::try_from(v).is_ok_and(|v| v < len);
        if y < surface || !inside(x, world.width()) || !inside(y, world.height()) {
            world.remove_creature(id);
            return Outcome::Removed(id);
        }
        world.release_creature(id);
        Outcome::Dropped(id)
    }

    /// The pointer left the tank (or the host lost it). Ends any stroke; a
    /// netted creature is let go where it is.
    pub fn leave(&mut self, world: &mut World) {
        if let Some(Stroke {
            action: Action::Net(Some((id, _))),
            ..
        }) = self.stroke
        {
            world.release_creature(id);
        }
        self.pointer = None;
        self.stroke = None;
    }

    /// Re-applies a held paint stroke where the pointer rests, so holding
    /// still keeps pouring (and keeps a rigid body being poured still), and
    /// keeps a netted creature held. Hosts call this once per simulation
    /// tick run (several times after a catch-up).
    pub fn hold(&mut self, world: &mut World) -> usize {
        match self.stroke {
            Some(s) if matches!(s.action, Action::Pour | Action::Plant | Action::Scoop) => {
                self.stamp(world, s.last)
            }
            Some(Stroke {
                action: Action::Net(Some((id, _))),
                ..
            }) => {
                world.hold_creature(id);
                0
            }
            _ => 0,
        }
    }

    /// Places a whole plant at `cell` ([`World::place_plant`]), mirrored if
    /// flipped; `None` where it doesn't fit or the tank is full.
    fn place_whole(
        &self,
        world: &mut World,
        species: PlantSpeciesId,
        stage: PlantStage,
        cell: (i64, i64),
    ) -> Option<PlantId> {
        world
            .place_plant(species, stage, cell.0, cell.1, self.flip)
            .ok()
            .flatten()
    }

    /// One brush disc at `c` for the stroke in progress; returns how many
    /// cells changed. A pour adds rigid cells to the stroke's body.
    fn stamp(&mut self, world: &mut World, c: (i64, i64)) -> usize {
        let plant = self.selected_plant().zip(self.plant_propagule(world));
        let Some(stroke) = self.stroke.as_mut() else {
            return 0;
        };
        if stroke.action == Action::Plant {
            // A known species and a clipped disc: this can't fail.
            return plant
                .and_then(|(s, p)| world.paint_plant(c.0, c.1, self.radius, s, p).ok())
                .unwrap_or(0);
        }
        if stroke.action == Action::Pour {
            // Built-in materials and a clipped disc: this can't fail.
            let Ok((changed, body)) = world.paint_into(
                c.0,
                c.1,
                self.radius,
                self.material,
                Brush::Pour,
                stroke.body,
            ) else {
                return 0;
            };
            stroke.body = body;
            return changed;
        }
        scoop(world, c, self.radius)
    }

    /// Glues or frees the bodies under the brush disc at `c`, in the
    /// stroke's direction (set by the first body it touches). Returns how
    /// many bodies changed.
    fn glue(&mut self, world: &mut World, c: (i64, i64)) -> usize {
        let Some(stroke) = self.stroke.as_mut() else {
            return 0;
        };
        let Action::Glue(mut way) = stroke.action else {
            return 0;
        };
        let mut changed = 0;
        for id in bodies_in_disc(world, c, self.radius) {
            let Some(body) = world.body(id) else { continue };
            let glue = *way.get_or_insert(!body.anchored);
            if world.set_anchored(id, glue) {
                changed += 1;
            }
        }
        stroke.action = Action::Glue(way);
        changed
    }
}

/// The bodies with a cell in the disc at `c`, in id order.
fn bodies_in_disc(world: &World, c: (i64, i64), radius: u32) -> Vec<BodyId> {
    let r = i64::from(radius);
    let mut ids = Vec::new();
    for y in c.1 - r..=c.1 + r {
        for x in c.0 - r..=c.0 + r {
            let (dx, dy) = (x - c.0, y - c.1);
            if dx * dx + dy * dy > r * r + r {
                continue;
            }
            if let (Ok(x), Ok(y)) = (usize::try_from(x), usize::try_from(y))
                && let Some(id) = world.body_at(x, y)
            {
                ids.push(id);
            }
        }
    }
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// Erases a disc like [`World::paint`]'s: air above the water line and
/// water below, cell by cell, then carves the disc's round edge into the
/// rigid cells left around it ([`World::carve`]).
fn scoop(world: &mut World, c: (i64, i64), radius: u32) -> usize {
    let table = world.materials();
    let (Some(air), Some(water)) = (table.id("air"), table.id("water")) else {
        return 0;
    };
    let r = i64::from(radius);
    let surface = world.surface_row();
    let mut changed = 0;
    for y in c.1 - r..=c.1 + r {
        for x in c.0 - r..=c.0 + r {
            let (dx, dy) = (x - c.0, y - c.1);
            if dx * dx + dy * dy > r * r + r {
                continue;
            }
            let (Ok(x), Ok(y)) = (usize::try_from(x), usize::try_from(y)) else {
                continue;
            };
            if let Some(count) = world.remove_ornament_at(x, y) {
                changed += count;
                continue;
            }
            let to = if y < surface { air } else { water };
            if world.cell(x, y).is_some_and(|cell| cell.material != to)
                && world.set_cell(x, y, to).is_ok()
            {
                changed += 1;
            }
        }
    }
    world.carve(c.0, c.1, radius);
    changed
}

fn inspect(world: &World, c: (i64, i64)) -> Outcome {
    let (Ok(x), Ok(y)) = (usize::try_from(c.0), usize::try_from(c.1)) else {
        return Outcome::None;
    };
    let Some(cell) = world.cell(x, y) else {
        return Outcome::None;
    };
    let material = cell.material;
    let creature = world
        .creature_at(c.0, c.1)
        .and_then(|id| world.creature(id));
    Outcome::Inspected(match (creature, world.plant_at(x, y)) {
        (Some(found), _) => Inspection::Creature {
            id: found.id(),
            species: found.species(),
            x,
            y,
            material,
        },
        (None, plant) => match (world.device_at(x, y), plant) {
            (Some(id), _) => Inspection::Device { id, x, y },
            (None, Some(id)) => Inspection::Plant { id, x, y, material },
            (None, None) => Inspection::Cell { x, y, material },
        },
    })
}

/// The cells of a line from `a` to `b` inclusive (Bresenham): 8-connected,
/// so discs stamped along it overlap.
fn line(a: (i64, i64), b: (i64, i64)) -> impl Iterator<Item = (i64, i64)> {
    let (dx, dy) = ((b.0 - a.0).abs(), -(b.1 - a.1).abs());
    let (sx, sy) = ((b.0 - a.0).signum(), (b.1 - a.1).signum());
    let mut p = a;
    let mut err = dx + dy;
    let mut done = false;
    core::iter::from_fn(move || {
        if done {
            return None;
        }
        let out = p;
        if p == b {
            done = true;
        } else {
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                p.0 += sx;
            }
            if e2 <= dx {
                err += dx;
                p.1 += sy;
            }
        }
        Some(out)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MaterialTable, WorldConfig};

    fn world() -> World {
        let config = WorldConfig {
            width: 160,
            height: 90,
            seed: 1,
            ..WorldConfig::default()
        };
        World::new(config, MaterialTable::builtin().unwrap()).unwrap()
    }

    fn id(w: &World, name: &str) -> MaterialId {
        w.materials().id(name).unwrap()
    }

    fn count(w: &World, m: MaterialId) -> usize {
        w.cells().iter().filter(|c| c.material == m).count()
    }

    fn at(w: &World, x: usize, y: usize) -> MaterialId {
        w.cell(x, y).unwrap().material
    }

    #[test]
    fn line_is_connected_and_inclusive() {
        let pts: Vec<_> = line((0, 0), (7, -3)).collect();
        assert_eq!(pts.first(), Some(&(0, 0)));
        assert_eq!(pts.last(), Some(&(7, -3)));
        assert_eq!(pts.len(), 8);
        for w in pts.windows(2) {
            assert!((w[1].0 - w[0].0).abs() <= 1 && (w[1].1 - w[0].1).abs() <= 1);
        }
        assert_eq!(line((2, 2), (2, 2)).count(), 1);
    }

    #[test]
    fn pour_fills_a_disc_of_the_selected_material() {
        let mut w = world();
        let sand = id(&w, "sand");
        let mut t = Tools::new(sand);
        t.resize(-1); // radius 2: 13 + 2·2 cells within r² + r
        let out = t.press(&mut w, Button::Primary, (80, 40));
        let n = count(&w, sand);
        assert_eq!(out, Outcome::Painted(n));
        assert_eq!(n, 21);
        assert_eq!(at(&w, 80, 40), sand);
        assert_eq!(at(&w, 82, 40), sand);
        assert_ne!(at(&w, 83, 40), sand);
    }

    #[test]
    fn pour_never_overwrites_solids() {
        let mut w = world();
        let (sand, rock) = (id(&w, "sand"), id(&w, "rock"));
        w.set_cell(80, 40, rock).unwrap();
        let mut t = Tools::new(sand);
        t.press(&mut w, Button::Primary, (80, 40));
        assert_eq!(at(&w, 80, 40), rock);
    }

    #[test]
    fn fast_drag_leaves_no_gaps() {
        let mut w = world();
        let sand = id(&w, "sand");
        let mut t = Tools::new(sand);
        t.resize(-i32::try_from(DEFAULT_RADIUS).unwrap()); // one cell
        // Two pointer samples 100 cells apart, as a fast flick gives.
        t.press(&mut w, Button::Primary, (20, 30));
        let out = t.moved(&mut w, (120, 50));
        t.release(&mut w, Button::Primary);
        for p in line((20, 30), (120, 50)) {
            let (x, y) = (usize::try_from(p.0).unwrap(), usize::try_from(p.1).unwrap());
            assert_eq!(at(&w, x, y), sand, "gap at {p:?}");
        }
        assert_eq!(out, Outcome::Painted(100));
        // Released: further moves do nothing.
        assert_eq!(t.moved(&mut w, (10, 10)), Outcome::None);
        assert_eq!(count(&w, sand), 101);
    }

    #[test]
    fn fast_drag_with_a_wide_brush_covers_a_solid_band() {
        let mut w = world();
        let sand = id(&w, "sand");
        let mut t = Tools::new(sand);
        t.press(&mut w, Button::Primary, (10, 40));
        t.moved(&mut w, (150, 40));
        for x in 10..=150 {
            for y in 37..=43 {
                assert_eq!(at(&w, x, y), sand, "gap at ({x}, {y})");
            }
        }
    }

    #[test]
    fn fast_drag_at_max_radius_covers_a_solid_band() {
        let config = WorldConfig {
            width: 320,
            height: 120,
            ..WorldConfig::default()
        };
        let mut w = World::new(config, MaterialTable::builtin().unwrap()).unwrap();
        let sand = id(&w, "sand");
        let mut t = Tools::new(sand);
        t.resize(1000);
        t.press(&mut w, Button::Primary, (30, 60));
        t.moved(&mut w, (290, 70));
        // Within r − 2 of the segment is all sand (the line's cells are
        // within a cell of the true segment).
        for x in 30..=290 {
            let cy = 60 + (x - 30) * 10 / 260;
            for y in cy - 22..=cy + 22 {
                let (ux, uy) = (usize::try_from(x).unwrap(), usize::try_from(y).unwrap());
                assert_eq!(at(&w, ux, uy), sand, "gap at ({x}, {y})");
            }
        }
    }

    #[test]
    fn scoop_erases_to_air_above_the_surface_and_water_below() {
        let config = WorldConfig {
            width: 160,
            height: 90,
            water_level: 0.5,
            ..WorldConfig::default()
        };
        let mut w = World::new(config, MaterialTable::builtin().unwrap()).unwrap();
        let (sand, air, water) = (id(&w, "sand"), id(&w, "air"), id(&w, "water"));
        let s = w.surface_row();
        let s_i = i64::try_from(s).unwrap();
        w.fill_rect(
            crate::Rect {
                x0: 70,
                y0: s - 5,
                x1: 90,
                y1: s + 5,
            },
            sand,
            Brush::Replace,
        )
        .unwrap();
        let mut t = Tools::new(sand);
        t.select(PaletteItem::Tool(Tool::Scoop));
        let out = t.press(&mut w, Button::Primary, (80, s_i));
        assert!(matches!(out, Outcome::Painted(n) if n > 0));
        assert_eq!(at(&w, 80, s - 1), air);
        assert_eq!(at(&w, 80, s), water);
        assert_eq!(at(&w, 80, s + 2), water);
        assert_eq!(at(&w, 80, s - 5), sand);
    }

    #[test]
    fn right_drag_scoops_whatever_tool_is_selected() {
        let mut w = world();
        let (rock, water) = (id(&w, "rock"), id(&w, "water"));
        let y = w.surface_row() + 10;
        for x in 40..60 {
            w.set_cell(x, y, rock).unwrap();
        }
        let mut t = Tools::new(rock);
        let yi = i64::try_from(y).unwrap();
        t.press(&mut w, Button::Secondary, (40, yi));
        t.moved(&mut w, (59, yi));
        assert_eq!(t.preview(&w), Some((59, yi, DEFAULT_RADIUS)));
        t.release(&mut w, Button::Secondary);
        assert!((40..60).all(|x| at(&w, x, y) == water));
        // Releasing the other button doesn't end a stroke it didn't start,
        // and a second button can't hijack it.
        t.press(&mut w, Button::Secondary, (40, yi));
        t.release(&mut w, Button::Primary);
        assert!(t.stroke.is_some());
        assert_eq!(t.press(&mut w, Button::Primary, (40, yi)), Outcome::None);
        t.release(&mut w, Button::Primary);
        assert!(t.stroke.is_some());
    }

    #[test]
    fn holding_still_keeps_pouring() {
        let mut w = world();
        let sand = id(&w, "sand");
        let mut t = Tools::new(sand);
        t.press(&mut w, Button::Primary, (80, 20));
        w.tick();
        w.tick();
        // The poured sand has started to fall, leaving fluid to refill.
        assert!(t.hold(&mut w) > 0);
        t.release(&mut w, Button::Primary);
        assert_eq!(t.hold(&mut w), 0);
    }

    #[test]
    fn net_changes_no_cells() {
        let mut w = world();
        let sand = id(&w, "sand");
        let before = w.state_hash();
        let mut t = Tools::new(sand);
        t.select(PaletteItem::Tool(Tool::Net));
        assert_eq!(t.press(&mut w, Button::Primary, (80, 60)), Outcome::None);
        assert_eq!(t.moved(&mut w, (90, 60)), Outcome::None);
        t.release(&mut w, Button::Primary);
        assert_eq!(t.preview(&w), None);
        assert_eq!(w.state_hash(), before);
    }

    #[test]
    fn inspect_reports_the_cell_and_changes_nothing() {
        let mut w = world();
        let (sand, water) = (id(&w, "sand"), id(&w, "water"));
        w.set_cell(80, 60, sand).unwrap();
        let before = w.state_hash();
        let mut t = Tools::new(sand);
        t.select(PaletteItem::Tool(Tool::Inspect));
        assert_eq!(
            t.press(&mut w, Button::Primary, (80, 60)),
            Outcome::Inspected(Inspection::Cell {
                x: 80,
                y: 60,
                material: sand
            })
        );
        assert_eq!(
            t.moved(&mut w, (81, 60)),
            Outcome::Inspected(Inspection::Cell {
                x: 81,
                y: 60,
                material: water
            })
        );
        assert_eq!(t.press(&mut w, Button::Primary, (-1, 60)), Outcome::None);
        assert_eq!(w.state_hash(), before);
    }

    #[test]
    fn palette_selects_tools_and_materials() {
        let w = world();
        let items = Tools::palette(&w);
        let pourable = w
            .materials()
            .iter()
            .filter(|m| m.palette.is_some_and(|p| p.holds() == Holds::Materials));
        let offered = w.species().iter().filter(|s| s.palette.is_some()).count();
        assert_eq!(
            items.len(),
            pourable.count() + w.plant_species().len() + Placeable::ALL.len() + 5 + offered
        );
        let gravel = id(&w, "gravel");
        let mut t = Tools::new(id(&w, "sand"));
        t.select(PaletteItem::Tool(Tool::Inspect));
        assert!(t.is_selected(PaletteItem::Tool(Tool::Inspect)));
        t.select(PaletteItem::Material(gravel));
        assert_eq!((t.tool(), t.material()), (Tool::Pour, gravel));
        assert!(t.is_selected(PaletteItem::Material(gravel)));
        assert_eq!(items.iter().filter(|&&i| t.is_selected(i)).count(), 1);
    }

    /// Every sub-category's items by name, `"top/sub"` (or `"top"`) first.
    fn tree_names(w: &World) -> Vec<(String, Vec<String>)> {
        Tools::palette_tree(w)
            .into_iter()
            .flat_map(|tab| tab.subs)
            .map(|sub| {
                let names = sub
                    .items
                    .iter()
                    .map(|&i| match i {
                        PaletteItem::Material(m) => w.materials().get(m).unwrap().name.clone(),
                        PaletteItem::Tool(Tool::AddCreature(s)) => {
                            w.species().get(s).unwrap().name.clone()
                        }
                        PaletteItem::Tool(Tool::Plant(s)) => {
                            w.plant_species().get(s).unwrap().name.clone()
                        }
                        PaletteItem::Tool(t) => format!("{t:?}"),
                    })
                    .collect();
                (sub.path.name(), names)
            })
            .collect()
    }

    #[test]
    #[allow(clippy::too_many_lines)] // full palette tree table
    fn palette_tree_comes_from_the_assets() {
        let w = world();
        let tree = Tools::palette_tree(&w);
        let tops: Vec<_> = tree.iter().map(|t| t.label).collect();
        assert_eq!(
            tops,
            [
                "Tools",
                "Build",
                "Water",
                "Food",
                "Care",
                "Plants",
                "Fish",
                "Invertebrates",
                "Amphibians"
            ],
            "every top category is filled"
        );
        let names = tree_names(&w);
        let got: Vec<(&str, Vec<&str>)> = names
            .iter()
            .map(|(p, n)| (p.as_str(), n.iter().map(String::as_str).collect()))
            .collect();
        let want: Vec<(&str, Vec<&str>)> = vec![
            ("tools", vec!["Scoop", "Net", "Inspect", "Glue", "Magnify"]),
            (
                "build/substrate",
                vec!["sand", "gravel", "aquasoil", "clown_vomit_gravel"],
            ),
            (
                "build/hardscape",
                vec!["rock", "dragon_stone", "seiryu_stone", "driftwood"],
            ),
            (
                "build/devices",
                vec![
                    "Place(Bubbler)",
                    "Place(InternalFilter)",
                    "Place(SpongeFilter)",
                    "Place(HobFilter)",
                    "Place(UnderGravelFilter)",
                    "Place(UnderGravelPanel)",
                ],
            ),
            (
                "build/ornaments",
                vec![
                    "Place(VolcanoBubbler)",
                    "Place(StoneCastle)",
                    "Place(PastelCastle)",
                ],
            ),
            ("water", vec!["water", "air_bubble", "co2_bubble"]),
            (
                "food/fish",
                vec![
                    "flake_food",
                    "sinking_pellet",
                    "bloodworm",
                    "algae_wafer",
                    "brine_shrimp",
                ],
            ),
            ("food/live", vec!["live_bloodworm", "live_brine_shrimp"]),
            (
                "care",
                vec!["root_tab", "liquid_fertiliser", "waste", "detritus"],
            ),
            (
                "plants/stem",
                vec!["hygrophila", "rotala", "ludwigia", "cabomba", "bacopa"],
            ),
            (
                "plants/rosette",
                vec!["amazon_sword", "vallisneria", "cryptocoryne", "tiger_lotus"],
            ),
            ("plants/carpet", vec!["dwarf_hairgrass", "monte_carlo"]),
            ("plants/floating", vec!["duckweed", "frogbit"]),
            (
                "plants/moss",
                vec![
                    "java_moss",
                    "christmas_moss",
                    "anubias",
                    "java_fern",
                    "marimo",
                ],
            ),
            (
                "fish/schooling",
                vec!["neon_tetra", "guppy", "harlequin_rasbora"],
            ),
            (
                "fish/centrepiece",
                vec![
                    "betta",
                    "honey_gourami",
                    "angelfish",
                    "pea_puffer",
                    "flowerhorn",
                ],
            ),
            ("fish/coldwater", vec!["common_goldfish", "fancy_goldfish"]),
            (
                "fish/cleanup",
                vec![
                    "corydoras",
                    "otocinclus",
                    "bristlenose_pleco",
                    "kuhli_loach",
                ],
            ),
            (
                "invertebrates/shrimp",
                vec!["cherry_shrimp", "amano_shrimp", "thai_micro_crab"],
            ),
            (
                "invertebrates/snails",
                vec!["nerite", "mystery_snail", "ramshorn_snail"],
            ),
            (
                "amphibians",
                vec!["african_dwarf_frog", "axolotl", "common_frog"],
            ),
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn palette_tree_offers_every_asset_once_in_order() {
        let w = world();
        let tree = Tools::palette_tree(&w);
        let names = tree_names(&w);
        // Each sub-category is in its top category, in tree order.
        for tab in &tree {
            assert!(tab.subs.iter().all(|s| s.path.top_name() == tab.name));
            assert!(tab.subs.windows(2).all(|p| p[0].path < p[1].path));
        }
        // Every stocked species and every plant species has a slot; the
        // cheat-only crabs and blinker (which come from their cheats)
        // don't.
        let all = Tools::palette(&w);
        for s in w.species().iter() {
            let item = PaletteItem::Tool(Tool::AddCreature(s.id));
            assert_eq!(all.contains(&item), !s.cheat_only, "{}", s.name);
        }
        assert_eq!(w.species().iter().filter(|s| s.cheat_only).count(), 3);
        for s in w.plant_species().iter() {
            assert!(
                all.contains(&PaletteItem::Tool(Tool::Plant(s.id))),
                "{}",
                s.name
            );
        }
        // Number keys pick the open sub-category's first ten slots, so
        // every slot is keyboard-reachable.
        for (path, items) in &names {
            assert!(items.len() <= 10, "{path} has {} slots", items.len());
        }
        // Plant tissue, air and device cells aren't poured.
        for name in [
            "air",
            "stem",
            "leaf",
            "root",
            "dead_plant",
            "bubbler",
            "seed",
        ] {
            let m = id(&w, name);
            assert!(!all.contains(&PaletteItem::Material(m)), "{name}");
        }
    }

    fn stock(t: &mut Tools, w: &mut World, x: i64, morph: u8) -> CreatureId {
        let outcome = t.press(w, Button::Primary, (x, 40));
        t.release(w, Button::Primary);
        let Outcome::Added(id) = outcome else {
            panic!("expected a creature, got {outcome:?}");
        };
        assert_eq!(w.creature(id).unwrap().alleles(), [morph; 2]);
        id
    }

    #[test]
    fn single_morph_creature_slot_places_without_a_pick() {
        let mut w = world();
        let mut t = Tools::new(id(&w, "sand"));
        assert_eq!(t.selected_creature(), None);
        assert_eq!(t.next_creature_morph(&w), None);
        assert!(!t.set_creature_morph(&w, None));

        let neon = w.species().id("neon_tetra").unwrap();
        t.select(PaletteItem::Tool(Tool::AddCreature(neon)));
        assert_eq!(t.selected_creature(), Some(neon));
        assert_eq!(t.creature_morph(&w), None);
        assert_eq!(t.next_creature_morph(&w), None);
        assert!(!t.set_creature_morph(&w, Some(0)));
        assert!(!t.set_creature_morph(&w, None));
        stock(&mut t, &mut w, 40, 0);
    }

    #[test]
    fn noncycling_creature_slot_places_the_default_or_its_pick() {
        let mut w = world();
        let mut t = Tools::new(id(&w, "sand"));
        let shrimp = w.species().id("cherry_shrimp").unwrap();
        let guppy = w.species().id("guppy").unwrap();
        t.select(PaletteItem::Tool(Tool::AddCreature(shrimp)));
        assert_eq!(t.creature_morph(&w), None);
        let default = w.species().get(shrimp).unwrap().palette_morph;
        stock(&mut t, &mut w, 40, default);
        assert_eq!(t.next_creature_morph(&w), Some(Some(1)));
        assert!(t.set_creature_morph(&w, Some(3)));
        assert_eq!(t.next_creature_morph(&w), Some(Some(0)));
        assert!(t.set_creature_morph(&w, Some(2)));
        assert!(!t.set_creature_morph(&w, Some(4)));

        t.select(PaletteItem::Tool(Tool::AddCreature(guppy)));
        assert_eq!(t.creature_morph(&w), None);
        assert!(t.set_creature_morph(&w, Some(1)));
        t.select(PaletteItem::Tool(Tool::AddCreature(shrimp)));
        assert_eq!(t.creature_morph(&w), Some(2));
        stock(&mut t, &mut w, 60, 2);

        let mut fresh = Tools::new(id(&w, "sand"));
        fresh.keep_creature_picks(&t);
        assert_eq!(fresh.morph_for(&w, shrimp), Some(2));
        assert_eq!(fresh.morph_for(&w, guppy), Some(1));
    }

    #[test]
    fn cycling_creature_slot_resumes_automatic_stocking_after_a_pick() {
        let mut w = world();
        let mut t = Tools::new(id(&w, "sand"));
        let axolotl = w.species().id("axolotl").unwrap();
        t.select(PaletteItem::Tool(Tool::AddCreature(axolotl)));
        assert_eq!(t.creature_morph(&w), None);
        stock(&mut t, &mut w, 20, 0);
        stock(&mut t, &mut w, 40, 1);

        assert!(t.set_creature_morph(&w, Some(1)));
        for x in [60, 80] {
            let id = stock(&mut t, &mut w, x, 1);
            assert_eq!(
                w.creature_card(id).unwrap().morph.as_deref(),
                Some("leucistic")
            );
        }
        assert!(t.set_creature_morph(&w, None));
        assert_eq!(t.creature_morph(&w), None);
        stock(&mut t, &mut w, 100, 1);

        for morph in [Some(0), Some(1), Some(2), None] {
            assert_eq!(t.next_creature_morph(&w), Some(morph));
        }
        assert_eq!(t.creature_morph(&w), None);
        stock(&mut t, &mut w, 120, 2);
    }

    #[test]
    fn a_plant_slot_pours_its_species_as_the_propagule_picked_for_it() {
        let mut w = world();
        let plant = |w: &World, name| w.plant_species().id(name).unwrap();
        let (hyg, moss, rotala) = (
            plant(&w, "hygrophila"),
            plant(&w, "java_moss"),
            plant(&w, "rotala"),
        );
        let mut t = Tools::new(id(&w, "sand"));
        // No plant slot selected: nothing to pick.
        assert_eq!(t.plant_propagule(&w), None);
        assert!(!t.set_plant_propagule(&w, Propagule::Seeds));
        assert_eq!(t.next_plant_propagule(&w), None);
        t.select(PaletteItem::Tool(Tool::Plant(hyg)));
        assert_eq!(t.selected_plant(), Some(hyg));
        assert!(t.preview(&w).is_none(), "no pointer yet");
        assert_eq!(t.plant_propagule(&w), Some(Propagule::Cuttings));
        assert!(t.set_plant_propagule(&w, Propagule::Seeds));
        // Picks are per species: moss only grows from fragments, rotala
        // from cuttings, and hygrophila keeps its seeds.
        t.select(PaletteItem::Tool(Tool::Plant(moss)));
        assert_eq!(t.plant_propagule(&w), Some(Propagule::Fragments));
        assert!(!t.set_plant_propagule(&w, Propagule::Seeds));
        assert_eq!(t.next_plant_propagule(&w), Some(Propagule::Fragments));
        t.select(PaletteItem::Tool(Tool::Plant(rotala)));
        assert_eq!(t.plant_propagule(&w), Some(Propagule::Cuttings));
        t.select(PaletteItem::Tool(Tool::Plant(hyg)));
        assert_eq!(t.plant_propagule(&w), Some(Propagule::Seeds));
        assert_eq!(t.propagule_for(&w, rotala), Some(Propagule::Cuttings));
        // P cycles the species' own methods, wrapping.
        let cycle: Vec<_> = (0..3)
            .map(|_| t.next_plant_propagule(&w).unwrap())
            .collect();
        assert_eq!(
            cycle,
            [Propagule::Cuttings, Propagule::Fragments, Propagule::Seeds]
        );
        // A press pours the slot's species as its pick: one seed.
        t.resize(-i32::try_from(DEFAULT_RADIUS).unwrap());
        let seed = id(&w, "seed");
        assert_eq!(
            t.press(&mut w, Button::Primary, (80, 30)),
            Outcome::Painted(1)
        );
        t.release(&mut w, Button::Primary);
        assert_eq!(count(&w, seed), 1);
        // The picks survive into a host's new tools.
        let mut fresh = Tools::new(id(&w, "sand"));
        fresh.keep_plant_picks(&t);
        fresh.select(PaletteItem::Tool(Tool::Plant(hyg)));
        assert_eq!(fresh.plant_propagule(&w), Some(Propagule::Seeds));
    }

    #[test]
    fn place_stamps_one_device_per_click() {
        let mut w = world();
        let device = id(&w, "device");
        let cells = w
            .device_kinds()
            .get(crate::DeviceKind::InternalFilter)
            .cells;
        let mut t = Tools::new(id(&w, "sand"));
        assert!(!t.can_flip(&w), "pour tool cannot flip");
        t.select(PaletteItem::Tool(Tool::Scoop));
        assert!(!t.can_flip(&w), "scoop cannot flip");
        t.select(PaletteItem::Tool(Tool::Inspect));
        assert!(!t.can_flip(&w), "inspect cannot flip");
        t.select(PaletteItem::Tool(Tool::Magnify));
        assert!(!t.can_flip(&w), "magnify cannot flip");
        t.select(PaletteItem::Tool(Tool::Place(Placeable::Bubbler)));
        assert!(!t.can_flip(&w), "bubbler does not flip");
        t.select(PaletteItem::Tool(Tool::Place(Placeable::SpongeFilter)));
        assert!(!t.can_flip(&w), "bed-mounted sponge filter does not flip");
        t.select(PaletteItem::Tool(Tool::Place(Placeable::HobFilter)));
        assert!(t.can_flip(&w), "rim-mounted HOB filter flips");
        t.select(PaletteItem::Tool(Tool::Place(Placeable::UnderGravelFilter)));
        assert!(t.can_flip(&w), "floor-mounted under-gravel filter flips");
        t.select(PaletteItem::Tool(Tool::Place(Placeable::UnderGravelPanel)));
        assert!(
            !t.can_flip(&w),
            "the symmetric under-gravel panel does not flip"
        );
        t.select(PaletteItem::Tool(Tool::Place(Placeable::InternalFilter)));
        assert!(t.can_flip(&w), "side-glass internal filter flips");
        assert_eq!(t.preview(&w), None);
        // The ghost follows the pointer: on the nearer (left) glass, or the
        // other one when flipped.
        t.moved(&mut w, (20, 40));
        let (ghost, fits) = t.ghost(&w).unwrap();
        assert!(fits && !ghost.facing_left && ghost.x == 0);
        assert!(t.flip());
        assert!(t.ghost(&w).unwrap().0.facing_left);
        assert!(!t.flip());
        assert_eq!(
            t.press(&mut w, Button::Primary, (20, 40)),
            Outcome::Painted(cells)
        );
        assert_eq!(t.moved(&mut w, (60, 40)), Outcome::None);
        assert_eq!(t.hold(&mut w), 0);
        t.release(&mut w, Button::Primary);
        assert_eq!(count(&w, device), cells);
        assert_eq!(at(&w, 5, 40), device);
        // Inspect names it; scooping one cell takes the whole device.
        t.select(PaletteItem::Tool(Tool::Inspect));
        let found = t.press(&mut w, Button::Primary, (5, 40));
        assert!(matches!(
            found,
            Outcome::Inspected(Inspection::Device { x: 5, y: 40, .. })
        ));
        t.release(&mut w, Button::Primary);
        t.select(PaletteItem::Tool(Tool::Scoop));
        t.resize(-3);
        t.press(&mut w, Button::Primary, (5, 40));
        t.release(&mut w, Button::Primary);
        assert_eq!(count(&w, device), 0);
        assert_eq!(w.devices().count(), 0);
    }

    #[test]
    fn brush_size_is_clamped() {
        let mut t = Tools::new(MaterialId(1));
        assert!(t.resize(2));
        assert_eq!(t.radius(), DEFAULT_RADIUS + 2);
        t.resize(1000);
        assert_eq!(t.radius(), MAX_RADIUS);
        assert!(!t.resize(1));
        t.resize(-1000);
        assert_eq!(t.radius(), MIN_RADIUS);
    }

    #[test]
    fn add_creature_reports_population_cap_and_no_room() {
        let mut w = world();
        let neon = w.species().id("neon_tetra").unwrap();
        let sand = id(&w, "sand");
        let mut t = Tools::new(sand);
        t.select(PaletteItem::Tool(Tool::AddCreature(neon)));

        // Deep inside a solid block there is no open water within PLACE_RADIUS.
        w.fill_rect(
            crate::Rect {
                x0: 0,
                y0: 55,
                x1: 128,
                y1: 96,
            },
            sand,
            Brush::Replace,
        )
        .unwrap();
        assert_eq!(
            t.press(&mut w, Button::Primary, (64, 80)),
            Outcome::AddRefused {
                species: neon,
                reason: AddRefused::NoRoom,
            }
        );
        t.release(&mut w, Button::Primary);

        // Filling the tank to its cap reports PopulationCap (not live food).
        assert_eq!(w.set_creature_cap(2), 2);
        for x in [40, 80] {
            assert!(matches!(
                t.press(&mut w, Button::Primary, (x, 40)),
                Outcome::Added(_)
            ));
            t.release(&mut w, Button::Primary);
        }
        assert_eq!(
            t.press(&mut w, Button::Primary, (60, 40)),
            Outcome::AddRefused {
                species: neon,
                reason: AddRefused::PopulationCap {
                    cap: 2,
                    live_food: false,
                },
            }
        );
        t.release(&mut w, Button::Primary);

        // Raising the cap lets another creature be added immediately.
        assert_eq!(w.set_creature_cap(3), 3);
        assert!(matches!(
            t.press(&mut w, Button::Primary, (60, 40)),
            Outcome::Added(_)
        ));
        t.release(&mut w, Button::Primary);
    }

    #[test]
    fn add_live_food_reports_live_food_cap() {
        let mut w = world();
        let worm = w.species().id("live_bloodworm").unwrap();
        let mut t = Tools::new(id(&w, "sand"));
        t.select(PaletteItem::Tool(Tool::AddCreature(worm)));

        // Pinches add up to MAX_LIVE_FOOD, then refuse with live_food: true.
        while w.live_food_count() < crate::MAX_LIVE_FOOD {
            assert!(matches!(
                t.press(&mut w, Button::Primary, (64, 40)),
                Outcome::Added(_)
            ));
            t.release(&mut w, Button::Primary);
        }
        assert_eq!(
            t.press(&mut w, Button::Primary, (64, 40)),
            Outcome::AddRefused {
                species: worm,
                reason: AddRefused::PopulationCap {
                    cap: crate::MAX_LIVE_FOOD,
                    live_food: true,
                },
            }
        );
        t.release(&mut w, Button::Primary);
    }

    #[test]
    fn magnify_tracks_pointer_cycles_on_click_and_steps_zoom() {
        let mut w = world();
        let sand = id(&w, "sand");
        let before = w.state_hash();
        let mut t = Tools::new(sand);
        assert_eq!(t.zoom(), DEFAULT_ZOOM);
        assert_eq!(t.magnifier(), None);

        t.select(PaletteItem::Tool(Tool::Magnify));
        // No pointer yet: no lens, no brush preview.
        assert_eq!(t.magnifier(), None);
        assert_eq!(t.preview(&w), None);

        // Hovering shows the magnifier at the default zoom and changes nothing.
        assert_eq!(t.moved(&mut w, (40, 30)), Outcome::None);
        assert_eq!(t.magnifier(), Some((40, 30, 2)));
        assert_eq!(t.preview(&w), None);

        // A click (press + release on the same cell) cycles zoom through ZOOM_LEVELS.
        for &want in &ZOOM_LEVELS[1..] {
            assert_eq!(t.press(&mut w, Button::Primary, (40, 30)), Outcome::None);
            assert_eq!(t.release(&mut w, Button::Primary), Outcome::Zoomed(want));
            assert_eq!(t.zoom(), want);
            assert_eq!(t.magnifier(), Some((40, 30, want)));
        }
        // Wraps back to the first level.
        assert_eq!(t.press(&mut w, Button::Primary, (40, 30)), Outcome::None);
        assert_eq!(
            t.release(&mut w, Button::Primary),
            Outcome::Zoomed(ZOOM_LEVELS[0])
        );

        // Dragging to another cell pans without cycling zoom on release.
        assert_eq!(t.press(&mut w, Button::Primary, (40, 30)), Outcome::None);
        assert_eq!(t.moved(&mut w, (55, 35)), Outcome::None);
        assert_eq!(t.magnifier(), Some((55, 35, ZOOM_LEVELS[0])));
        assert_eq!(t.release(&mut w, Button::Primary), Outcome::None);
        assert_eq!(t.zoom(), ZOOM_LEVELS[0]);

        // step_zoom clamps at both ends; set_zoom snaps to the nearest level.
        assert!(!t.step_zoom(-1));
        assert!(t.step_zoom(1));
        assert_eq!(t.zoom(), 4);
        assert!(t.step_zoom(99));
        assert_eq!(t.zoom(), 8);
        assert!(!t.step_zoom(1));
        assert!(t.set_zoom(5));
        assert_eq!(t.zoom(), 4);
        assert!(t.set_zoom(6));
        assert_eq!(t.zoom(), 6);
        assert!(!t.set_zoom(6));

        // Nothing in the world changed so far.
        assert_eq!(w.state_hash(), before);

        // Secondary button scoops as usual, hiding the magnifier during the stroke.
        w.set_cell(60, 40, sand).unwrap();
        assert!(matches!(
            t.press(&mut w, Button::Secondary, (60, 40)),
            Outcome::Painted(_)
        ));
        assert_eq!(t.magnifier(), None);
        assert!(t.preview(&w).is_some());
        t.release(&mut w, Button::Secondary);
        assert_eq!(t.magnifier(), Some((60, 40, 6)));

        // Leaving hides the lens.
        t.leave(&mut w);
        assert_eq!(t.magnifier(), None);
    }
}
