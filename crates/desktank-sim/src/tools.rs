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
//! of its species per click. The one plants slot (T6b) pours the species
//! picked with [`Tools::set_plant_species`], as the propagule (seeds,
//! cuttings or fragments) picked with [`Tools::set_plant_propagule`].
//!
//! The palette is grouped ([`PaletteGroup`], [`Tools::palette_groups`]):
//! hosts show a group selector and the open group's items, so every slot
//! stays a short reach (and a number key) away.

pub use crate::CreatureId;
use crate::{
    BodyId, Brush, MaterialId, Placeable, PlantId, PlantSpeciesId, Propagule, SpeciesId, World,
};

/// Smallest brush radius in cells (a single cell).
pub const MIN_RADIUS: u32 = 0;
/// Largest brush radius in cells.
pub const MAX_RADIUS: u32 = 24;
/// Brush radius a new [`Tools`] starts with.
pub const DEFAULT_RADIUS: u32 = 3;

/// A palette group: hosts show one group's items at a time, with a
/// selector for the groups (SPEC §2.5).
///
/// The material groups come from data: each pourable material names its
/// group in its TOML (`palette = "substrate"`), and materials without one
/// aren't offered. The tools, devices and creatures groups are filled from
/// code ([`Tool`], [`Placeable::ALL`], the species table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaletteGroup {
    /// Scoop, net, inspect and glue.
    Tools,
    /// Sand, gravel, soil, rock and driftwood.
    Substrate,
    /// Water and bubbles.
    Water,
    /// Fish food and plant fertilisers.
    Food,
    /// The plants slot (seeds and cuttings of a picked species) and algae.
    Plants,
    /// Waste and detritus.
    Waste,
    /// Placeable devices: bubbler, filter outlet.
    Devices,
    /// One slot per species.
    Creatures,
}

impl PaletteGroup {
    /// Every group, in palette order.
    pub const ALL: [Self; 8] = [
        Self::Tools,
        Self::Substrate,
        Self::Water,
        Self::Food,
        Self::Plants,
        Self::Waste,
        Self::Devices,
        Self::Creatures,
    ];

    /// The name materials use for it (`palette = "…"`).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Tools => "tools",
            Self::Substrate => "substrate",
            Self::Water => "water",
            Self::Food => "food",
            Self::Plants => "plants",
            Self::Waste => "waste",
            Self::Devices => "devices",
            Self::Creatures => "creatures",
        }
    }

    /// Display label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Tools => "Tools",
            Self::Substrate => "Substrate",
            Self::Water => "Water",
            Self::Food => "Food",
            Self::Plants => "Plants",
            Self::Waste => "Waste",
            Self::Devices => "Devices",
            Self::Creatures => "Creatures",
        }
    }

    /// Whether materials may name it: the tools, devices and creatures
    /// groups are filled from code.
    #[must_use]
    pub fn holds_materials(self) -> bool {
        !matches!(self, Self::Tools | Self::Devices | Self::Creatures)
    }
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
    /// Place a device, once per click.
    Place(Placeable),
    /// Add a creature of this species, once per click.
    AddCreature(SpeciesId),
    /// Pour the picked plant species ([`Tools::plant_species`]) as the
    /// picked propagule ([`Tools::plant_propagule`]).
    Plant,
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
    /// Pour a plant species' seeds or cuttings.
    Plant,
    Scoop,
    /// Glue (`Some(true)`) or free bodies; `None` until the stroke first
    /// touches a body, which it toggles, setting the way for the rest.
    Glue(Option<bool>),
    /// Carrying a creature, held by the cell offset from the pointer to its
    /// body centre (or nothing: the hit test missed).
    Net(Option<(CreatureId, (i64, i64))>),
    Inspect,
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
    /// The species the plants slot pours (0: the default species).
    plant: PlantSpeciesId,
    /// The propagule picked for it (`None`: the species' default). Kept
    /// across species changes; one a species doesn't grow from falls back
    /// to that species' default ([`Tools::plant_propagule`]).
    propagule: Option<Propagule>,
    radius: u32,
    pointer: Option<(i64, i64)>,
    stroke: Option<Stroke>,
}

impl Tools {
    /// Pour `material` with the default brush.
    #[must_use]
    pub fn new(material: MaterialId) -> Self {
        Self {
            tool: Tool::Pour,
            material,
            plant: PlantSpeciesId(0),
            propagule: None,
            radius: DEFAULT_RADIUS,
            pointer: None,
            stroke: None,
        }
    }

    /// The palette for `world`, grouped: the non-empty groups in
    /// [`PaletteGroup::ALL`] order with their items. Tools are scoop, net,
    /// inspect and glue; each material group lists the materials naming it,
    /// by ID (the plants group starts with the plants slot); then the
    /// placeables whose material `world` has, and its
    /// species by ID.
    #[must_use]
    pub fn palette_groups(world: &World) -> Vec<(PaletteGroup, Vec<PaletteItem>)> {
        let table = world.materials();
        PaletteGroup::ALL
            .into_iter()
            .map(|group| {
                let items: Vec<_> = match group {
                    PaletteGroup::Tools => [Tool::Scoop, Tool::Net, Tool::Inspect, Tool::Glue]
                        .map(PaletteItem::Tool)
                        .to_vec(),
                    PaletteGroup::Devices => Placeable::ALL
                        .into_iter()
                        .filter(|p| table.id(p.material_name()).is_some())
                        .map(|p| PaletteItem::Tool(Tool::Place(p)))
                        .collect(),
                    PaletteGroup::Creatures => world
                        .species()
                        .iter()
                        .map(|s| PaletteItem::Tool(Tool::AddCreature(s.id)))
                        .collect(),
                    // The plants slot (if `world` has plant species and
                    // their seeds and cuttings) leads its group.
                    PaletteGroup::Plants => {
                        let slot = (!world.plant_species().is_empty()
                            && ["seed", "cutting"].iter().all(|n| table.id(n).is_some()))
                        .then_some(PaletteItem::Tool(Tool::Plant));
                        slot.into_iter()
                            .chain(
                                table
                                    .iter()
                                    .filter(|m| m.palette == Some(group))
                                    .map(|m| PaletteItem::Material(m.id)),
                            )
                            .collect()
                    }
                    _ => table
                        .iter()
                        .filter(|m| m.palette == Some(group))
                        .map(|m| PaletteItem::Material(m.id))
                        .collect(),
                };
                (group, items)
            })
            .filter(|(_, items)| !items.is_empty())
            .collect()
    }

    /// The palette for `world` as one list: [`Tools::palette_groups`]'
    /// items, group after group.
    #[must_use]
    pub fn palette(world: &World) -> Vec<PaletteItem> {
        Self::palette_groups(world)
            .into_iter()
            .flat_map(|(_, items)| items)
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

    /// The plant species the plants slot pours, as `world` resolves it
    /// (the default species until one is picked).
    #[must_use]
    pub fn plant_species(&self, world: &World) -> Option<PlantSpeciesId> {
        world
            .plant_species()
            .for_state(u16::from(self.plant.0))
            .map(|s| s.id)
    }

    /// Picks the plant species the plants slot pours. Doesn't change the
    /// selected tool. The picked propagule stays if the species grows from
    /// it; otherwise the species' default is poured.
    pub fn set_plant_species(&mut self, species: PlantSpeciesId) {
        self.plant = species;
    }

    /// The propagule the plants slot pours, as `world` resolves it: the
    /// picked one if the picked species grows from it, else the species'
    /// [`crate::PlantSpecies::default_propagule`].
    #[must_use]
    pub fn plant_propagule(&self, world: &World) -> Option<Propagule> {
        let s = world.plant_species().for_state(u16::from(self.plant.0))?;
        Some(
            self.propagule
                .filter(|&p| s.grows_from(p))
                .unwrap_or_else(|| s.default_propagule()),
        )
    }

    /// Picks the propagule the plants slot pours, if the picked species
    /// grows from it (as `world` resolves the species). Returns whether it
    /// was taken. Doesn't change the selected tool.
    pub fn set_plant_propagule(&mut self, world: &World, propagule: Propagule) -> bool {
        let ok = world
            .plant_species()
            .for_state(u16::from(self.plant.0))
            .is_some_and(|s| s.grows_from(propagule));
        if ok {
            self.propagule = Some(propagule);
        }
        ok
    }

    /// Takes `other`'s plant species and propagule picks as they are (the
    /// stored propagule pick too, even one its species doesn't grow from),
    /// for a host replacing its tank. Species ids are stable across
    /// worlds; one this world lacks resolves to the default species.
    pub fn keep_plant_picks(&mut self, other: &Self) {
        self.plant = other.plant;
        self.propagule = other.propagule;
    }

    /// Picks the next propagule the picked species grows from (wrapping),
    /// for hosts without a picker of their own. Returns it.
    pub fn next_plant_propagule(&mut self, world: &World) -> Option<Propagule> {
        let s = world.plant_species().for_state(u16::from(self.plant.0))?;
        let current = self.plant_propagule(world)?;
        let next = s
            .propagules()
            .find(|&p| p > current)
            .or_else(|| s.propagules().next())?;
        self.propagule = Some(next);
        Some(next)
    }

    /// Picks the next plant species after the current one (wrapping), for
    /// hosts without a picker of their own. Returns it.
    pub fn next_plant_species(&mut self, world: &World) -> Option<PlantSpeciesId> {
        let table = world.plant_species();
        let current = self.plant_species(world)?;
        let next = table
            .iter()
            .map(|s| s.id)
            .find(|&id| id > current)
            .or_else(|| table.iter().next().map(|s| s.id))?;
        self.plant = next;
        Some(next)
    }

    /// Brush radius in cells.
    #[must_use]
    pub fn radius(&self) -> u32 {
        self.radius
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
    /// cells: for pour, scoop and glue, or any stroke that uses the brush.
    #[must_use]
    pub fn preview(&self) -> Option<(i64, i64, u32)> {
        let painting = match self.stroke {
            Some(s) => matches!(
                s.action,
                Action::Pour | Action::Plant | Action::Scoop | Action::Glue(_)
            ),
            None => matches!(
                self.tool,
                Tool::Pour | Tool::Plant | Tool::Scoop | Tool::Glue
            ),
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
            (Button::Primary, Tool::Plant) => Action::Plant,
            (Button::Primary, Tool::Glue) => Action::Glue(None),
            (Button::Primary, Tool::Net) => {
                Action::Net(world.creature_at(cell.0, cell.1).and_then(|id| {
                    let (x, y) = world.creature(id)?.cell();
                    Some((id, (x - cell.0, y - cell.1)))
                }))
            }
            (Button::Primary, Tool::Inspect) => Action::Inspect,
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
                Tool::Place(p) => Outcome::Painted(world.place(p, cell.0, cell.1).unwrap_or(0)),
                // No room (solid there) or a full tank adds nothing.
                Tool::AddCreature(s) => world
                    .add_creature(s, cell.0, cell.1)
                    .map_or(Outcome::None, Outcome::Added),
                _ => Outcome::None,
            },
            Action::Net(Some((id, _))) => {
                world.hold_creature(id);
                Outcome::Caught(id)
            }
            Action::Net(None) => Outcome::None,
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
            Action::Net(None) | Action::Inspect | Action::Place => Outcome::None,
        }
    }

    /// `button` was released. Ends the stroke it started. A netted creature
    /// is let go where it is, or lifted out of the tank if the pointer is
    /// above the water or outside the grid.
    pub fn release(&mut self, world: &mut World, button: Button) -> Outcome {
        let Some(stroke) = self.stroke.filter(|s| s.button == button) else {
            return Outcome::None;
        };
        self.stroke = None;
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

    /// One brush disc at `c` for the stroke in progress; returns how many
    /// cells changed. A pour adds rigid cells to the stroke's body.
    fn stamp(&mut self, world: &mut World, c: (i64, i64)) -> usize {
        let plant = self.plant_species(world).zip(self.plant_propagule(world));
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
/// water below, cell by cell.
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
            let to = if y < surface { air } else { water };
            if world.cell(x, y).is_some_and(|cell| cell.material != to)
                && world.set_cell(x, y, to).is_ok()
            {
                changed += 1;
            }
        }
    }
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
        (None, Some(id)) => Inspection::Plant { id, x, y, material },
        (None, None) => Inspection::Cell { x, y, material },
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
        assert_eq!(t.preview(), Some((59, yi, DEFAULT_RADIUS)));
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
        assert_eq!(t.preview(), None);
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
        let pourable = w.materials().iter().filter(|m| m.palette.is_some());
        assert_eq!(
            items.len(),
            pourable.count() + 1 + 2 + 4 + w.species().len()
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

    #[test]
    fn palette_groups_come_from_the_materials() {
        let w = world();
        let groups = Tools::palette_groups(&w);
        let order: Vec<_> = groups.iter().map(|(g, _)| *g).collect();
        assert_eq!(order, PaletteGroup::ALL, "every built-in group is filled");
        let names = |g: PaletteGroup| -> Vec<String> {
            let (_, items) = groups.iter().find(|(x, _)| *x == g).unwrap();
            items
                .iter()
                .map(|&i| match i {
                    PaletteItem::Material(m) => w.materials().get(m).unwrap().name.clone(),
                    PaletteItem::Tool(t) => format!("{t:?}"),
                })
                .collect()
        };
        assert_eq!(
            names(PaletteGroup::Tools),
            ["Scoop", "Net", "Inspect", "Glue"]
        );
        assert_eq!(
            names(PaletteGroup::Substrate),
            ["sand", "gravel", "rock", "aquasoil", "driftwood"]
        );
        assert_eq!(
            names(PaletteGroup::Water),
            ["water", "air_bubble", "co2_bubble"]
        );
        assert_eq!(names(PaletteGroup::Plants), ["Plant", "algae"]);
        assert_eq!(names(PaletteGroup::Waste), ["waste", "detritus"]);
        assert_eq!(
            names(PaletteGroup::Devices),
            ["Place(Bubbler)", "Place(FilterOutlet)"]
        );
        // Every species, nerites included, has a slot in Creatures.
        let creatures: Vec<_> = w
            .species()
            .iter()
            .map(|s| PaletteItem::Tool(Tool::AddCreature(s.id)))
            .collect();
        assert!(w.species().iter().any(|s| s.name == "nerite"));
        assert_eq!(
            groups
                .iter()
                .find(|(g, _)| *g == PaletteGroup::Creatures)
                .unwrap()
                .1,
            creatures
        );
        // Number keys pick within the open group: every slot is reachable.
        assert!(groups.iter().all(|(_, items)| items.len() <= 10));
        // Plant tissue, air and device cells aren't poured.
        for name in ["air", "stem", "leaf", "root", "dead_plant", "bubbler"] {
            let m = id(&w, name);
            assert!(
                !Tools::palette(&w).contains(&PaletteItem::Material(m)),
                "{name}"
            );
        }
    }

    #[test]
    fn place_stamps_one_device_per_click() {
        let mut w = world();
        let outlet = id(&w, "filter_outlet");
        let mut t = Tools::new(id(&w, "sand"));
        t.select(PaletteItem::Tool(Tool::Place(Placeable::FilterOutlet)));
        assert_eq!(t.preview(), None);
        assert_eq!(
            t.press(&mut w, Button::Primary, (20, 40)),
            Outcome::Painted(9)
        );
        assert_eq!(t.moved(&mut w, (60, 40)), Outcome::None);
        assert_eq!(t.hold(&mut w), 0);
        t.release(&mut w, Button::Primary);
        assert_eq!(count(&w, outlet), 9);
        assert_eq!(at(&w, 20, 40), outlet);
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
}
