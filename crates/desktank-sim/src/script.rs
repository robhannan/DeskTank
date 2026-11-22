//! Deterministic input scripts: timed pours replayed against a world, for
//! `desktank capture`, render tests and determinism tests (SPEC §7).
//!
//! A script is TOML. Every key is optional:
//!
//! ```toml
//! ticks = 200        # ticks to run before the first frame (default 0)
//! frame_every = 2    # ticks between frames when capturing several (default 1)
//!
//! [world]
//! width = 160        # grid size in cells (default 160×90)
//! height = 90
//! water_level = 0.96 # fraction of the height (default 0.96)
//! starter = true     # lay out the starter scene first (default false)
//!
//! [[pour]]
//! tick = 0           # first tick to pour on (default 0)
//! repeat = 40        # pour on this many consecutive ticks (default 1)
//! x = 80             # disc centre, in cells from the top left
//! y = 10
//! dx = 1             # move the centre this many cells per repeat (default 0)
//! radius = 3         # disc radius in cells (default 2)
//! material = "sand"  # a material name from assets/materials
//! brush = "pour"     # "pour" fills only fluid cells, "replace" overwrites (default "pour")
//! glued = true       # a rigid material's body is glued in place (default false)
//! # plant = "rotala" # instead of a material: pour this plant species
//!                    # (assets/plants), as its palette slot does
//! # propagule = "seeds" # with `plant`: "seeds", "cuttings", "fragments" or "runners", one the
//!                    # species lists (default: cuttings if it lists them, else its first)
//!
//! [[place]]
//! tick = 0           # tick to place on (default 0)
//! x = 20             # footprint centre, in cells from the top left
//! y = 60
//! device = "bubbler" # "bubbler" or "internal_filter" ("filter_outlet" names it too; see `Placeable`)
//!
//! [[water_change]]
//! tick = 600         # tick to change the water on (see `World::water_change`)
//!
//! [[creature]]
//! tick = 0           # tick to add them on (default 0)
//! species = "neon_tetra" # a species name from assets/species
//! x = 40             # body centre, in cells from the top left (the nearest free spot)
//! y = 30
//! count = 6          # how many (default 1)
//! dx = 3             # move the centre this many cells per creature (default 0)
//! age_days = 10.0    # age in days of its life (default: just mature); a
//!                    # young one is as grown as a well-fed one would be
//! sex = "female"     # "female" or "male" (default: whichever its species
//!                    # has fewer of, a male on a tie)
//! morph = "blue"     # a colour morph of the species, both alleles (default:
//!                    # the palette's)
//! ```
//!
//! Water changes, then placements, then pours, then creatures due on tick `t` are applied just
//! before the world's `t`-th tick (counting from 0), so they show from that tick on. Each
//! `[[pour]]` entry is one stroke: the rigid cells of all its repeats form
//! one body, held still until its last repeat (D17).

use serde::Deserialize;
use thiserror::Error;

use crate::{BodyId, Brush, Placeable, Propagule, World, WorldConfig, WorldError};

/// Most creatures one `[[creature]]` entry may add.
#[allow(clippy::cast_possible_truncation)] // 128
const MAX_COUNT: u32 = crate::MAX_CREATURE_CAP as u32;

/// Error reading or applying a script.
#[derive(Debug, Error)]
pub enum ScriptError {
    /// `frame_every` is zero.
    #[error("frame_every must be at least 1")]
    ZeroFrameEvery,
    /// The TOML didn't parse or has unknown keys.
    #[error("bad script: {0}")]
    Parse(#[from] toml::de::Error),
    /// A pour names a material the table doesn't have.
    #[error("unknown material {0:?}")]
    UnknownMaterial(String),
    /// A placement names a device that doesn't exist.
    #[error("unknown device {0:?}")]
    UnknownDevice(String),
    /// A `[[creature]]` entry names a species the table doesn't have.
    #[error("unknown species {0:?}")]
    UnknownSpecies(String),
    /// A pour names a plant species the table doesn't have.
    #[error("unknown plant species {0:?}")]
    UnknownPlantSpecies(String),
    /// A pour names a propagule its plant species doesn't grow from.
    #[error("plant species {plant:?} doesn't grow from {propagule:?}")]
    BadPropagule {
        /// The plant species name.
        plant: String,
        /// The propagule name.
        propagule: &'static str,
    },
    /// A pour names a propagule but no plant.
    #[error("propagule {0:?} needs a plant")]
    PropaguleWithoutPlant(&'static str),
    /// A `[[creature]]` entry names a morph its species doesn't have.
    #[error("species {species:?} has no morph {morph:?}")]
    UnknownMorph {
        /// The species name.
        species: String,
        /// The morph name.
        morph: String,
    },
    /// A `[[creature]]` entry asks for more creatures than a tank holds.
    #[error("count {0} is more than a tank holds")]
    TooMany(u32),
    /// The world rejected an edit or its configuration.
    #[error(transparent)]
    World(#[from] WorldError),
}

/// A parsed input script.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Script {
    /// Ticks to run before the first frame.
    #[serde(default)]
    pub ticks: u64,
    /// Ticks between frames when capturing several.
    #[serde(default = "one")]
    pub frame_every: u64,
    /// The world to build.
    #[serde(default)]
    pub world: WorldSpec,
    /// Timed pours.
    #[serde(default, rename = "pour")]
    pub pours: Vec<Pour>,
    /// Timed device placements.
    #[serde(default, rename = "place")]
    pub places: Vec<Place>,
    /// Timed water changes.
    #[serde(default, rename = "water_change")]
    pub water_changes: Vec<WaterChange>,
    /// Timed creature additions.
    #[serde(default, rename = "creature")]
    pub creatures: Vec<AddCreatures>,
}

impl Default for Script {
    fn default() -> Self {
        Self {
            ticks: 0,
            frame_every: 1,
            world: WorldSpec::default(),
            pours: Vec::new(),
            places: Vec::new(),
            water_changes: Vec::new(),
            creatures: Vec::new(),
        }
    }
}

/// The `[world]` table.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct WorldSpec {
    /// Width in cells.
    pub width: usize,
    /// Height in cells.
    pub height: usize,
    /// Water level as a fraction of the height.
    pub water_level: f32,
    /// Lay out the host's starter scene before the script runs.
    pub starter: bool,
}

impl Default for WorldSpec {
    fn default() -> Self {
        Self {
            width: 160,
            height: 90,
            water_level: WorldConfig::default().water_level,
            starter: false,
        }
    }
}

/// One `[[pour]]` entry.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Pour {
    /// First tick to pour on.
    #[serde(default)]
    pub tick: u64,
    /// Number of consecutive ticks to pour on.
    #[serde(default = "one")]
    pub repeat: u64,
    /// Disc centre column.
    pub x: i64,
    /// Disc centre row.
    pub y: i64,
    /// Column step per repeat.
    #[serde(default)]
    pub dx: i64,
    /// Disc radius in cells.
    #[serde(default = "two")]
    pub radius: u32,
    /// Material name (unused when `plant` is set).
    #[serde(default)]
    pub material: String,
    /// Plant species name: pour its seeds, cuttings, fragments or runners instead.
    #[serde(default)]
    pub plant: Option<String>,
    /// With `plant`: the propagule to pour (default: the species').
    #[serde(default)]
    pub propagule: Option<Propagule>,
    /// How the disc treats the cells it covers.
    #[serde(default)]
    pub brush: BrushName,
    /// Glue the body a rigid pour makes.
    #[serde(default)]
    pub glued: bool,
}

/// One `[[place]]` entry.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Place {
    /// Tick to place on.
    #[serde(default)]
    pub tick: u64,
    /// Footprint centre column.
    pub x: i64,
    /// Footprint centre row.
    pub y: i64,
    /// Device: a [`Placeable`] name.
    pub device: String,
}

/// One `[[water_change]]` entry.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WaterChange {
    /// Tick to change the water on.
    pub tick: u64,
}

/// One `[[creature]]` entry.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AddCreatures {
    /// Tick to add them on.
    #[serde(default)]
    pub tick: u64,
    /// Species name.
    pub species: String,
    /// Body centre column of the first.
    pub x: i64,
    /// Body centre row.
    pub y: i64,
    /// How many.
    #[serde(default = "one_u32")]
    pub count: u32,
    /// Column step per creature.
    #[serde(default)]
    pub dx: i64,
    /// Age in days of a creature's life (`None`: just mature).
    #[serde(default)]
    pub age_days: Option<f64>,
    /// Its sex (`None`: whichever its species has fewer of).
    #[serde(default)]
    pub sex: Option<Sex>,
    /// A colour morph name, given both alleles (`None`: the palette's).
    #[serde(default)]
    pub morph: Option<String>,
}

/// A creature's sex as written in a script.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Sex {
    /// Female.
    Female,
    /// Male.
    Male,
}

/// A brush as written in a script.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BrushName {
    /// [`Brush::Pour`].
    #[default]
    Pour,
    /// [`Brush::Replace`].
    Replace,
}

fn one() -> u64 {
    1
}

fn one_u32() -> u32 {
    1
}

fn two() -> u32 {
    2
}

impl Script {
    /// Parses a script.
    ///
    /// # Errors
    ///
    /// Returns an error if the TOML is malformed, has unknown keys, sets
    /// `frame_every` to zero, or a pour names a propagule without a plant.
    pub fn parse(source: &str) -> Result<Self, ScriptError> {
        let script: Self = toml::from_str(source)?;
        if script.frame_every == 0 {
            return Err(ScriptError::ZeroFrameEvery);
        }
        if let Some(p) = script
            .places
            .iter()
            .find(|p| Placeable::from_name(&p.device).is_none())
        {
            return Err(ScriptError::UnknownDevice(p.device.clone()));
        }
        if let Some(q) = script
            .pours
            .iter()
            .filter(|p| p.plant.is_none())
            .find_map(|p| p.propagule)
        {
            return Err(ScriptError::PropaguleWithoutPlant(q.name()));
        }
        if let Some(c) = script.creatures.iter().find(|c| c.count > MAX_COUNT) {
            return Err(ScriptError::TooMany(c.count));
        }
        Ok(script)
    }

    /// The world configuration for `seed`.
    #[must_use]
    pub fn world_config(&self, seed: u64) -> WorldConfig {
        WorldConfig {
            width: self.world.width,
            height: self.world.height,
            seed,
            water_level: self.world.water_level,
        }
    }

    /// A player that replays this script against one world.
    #[must_use]
    pub fn player(&self) -> Player<'_> {
        Player {
            script: self,
            bodies: vec![None; self.pours.len()],
        }
    }

    /// Ticks the world from its current tick up to `until`, applying pours
    /// on the way (with a fresh [`Player`]).
    ///
    /// # Errors
    ///
    /// Returns an error if a pour names an unknown material.
    pub fn run(&self, world: &mut World, until: u64) -> Result<(), ScriptError> {
        self.player().run(world, until)
    }
}

/// Replays a [`Script`] against one world, remembering the body each rigid
/// pour is filling.
#[derive(Clone, Debug)]
pub struct Player<'a> {
    script: &'a Script,
    bodies: Vec<Option<BodyId>>,
}

impl Player<'_> {
    /// Applies the water changes, placements, pours and creatures due on
    /// `tick` (call before the world's tick).
    ///
    /// # Errors
    ///
    /// Returns an error if a pour names an unknown material, a placement an
    /// unknown device or a creature an unknown species, or a creature has
    /// no room (or the tank is full).
    pub fn apply(&mut self, world: &mut World, tick: u64) -> Result<(), ScriptError> {
        for _ in self.script.water_changes.iter().filter(|c| c.tick == tick) {
            world.water_change();
        }
        for p in self.script.places.iter().filter(|p| p.tick == tick) {
            let device = Placeable::from_name(&p.device)
                .ok_or_else(|| ScriptError::UnknownDevice(p.device.clone()))?;
            world.place(device, p.x, p.y)?;
        }
        for (p, body) in self.script.pours.iter().zip(&mut self.bodies) {
            let Some(i) = tick.checked_sub(p.tick).filter(|&i| i < p.repeat) else {
                continue;
            };
            let x =
                p.x.saturating_add(p.dx.saturating_mul(i64::try_from(i).unwrap_or(i64::MAX)));
            // Clamped far outside any grid, so `paint`'s clipping maths can't
            // overflow; a disc there misses the grid either way.
            let far = 4 * i64::try_from(crate::MAX_SIDE).unwrap_or(i64::MAX / 8);
            let (x, y) = (x.clamp(-far, far), p.y.clamp(-far, far));
            if let Some(name) = &p.plant {
                let s = world
                    .plant_species()
                    .iter()
                    .find(|s| s.name == *name)
                    .ok_or_else(|| ScriptError::UnknownPlantSpecies(name.clone()))?;
                let propagule = p.propagule.unwrap_or_else(|| s.default_propagule());
                if !s.grows_from(propagule) {
                    return Err(ScriptError::BadPropagule {
                        plant: name.clone(),
                        propagule: propagule.name(),
                    });
                }
                let species = s.id;
                world.paint_plant(x, y, p.radius, species, propagule)?;
                continue;
            }
            let material = world
                .materials()
                .id(&p.material)
                .ok_or_else(|| ScriptError::UnknownMaterial(p.material.clone()))?;
            let brush = match p.brush {
                BrushName::Pour => Brush::Pour,
                BrushName::Replace => Brush::Replace,
            };
            let (_, joined) = world.paint_into(x, y, p.radius, material, brush, *body)?;
            *body = joined;
            if p.glued
                && let Some(id) = joined
            {
                world.set_anchored(id, true);
            }
        }
        for c in self.script.creatures.iter().filter(|c| c.tick == tick) {
            let species = world
                .species()
                .id(&c.species)
                .ok_or_else(|| ScriptError::UnknownSpecies(c.species.clone()))?;
            // Clamped to the longest lifespan, so the cast is exact.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let age = c.age_days.map(|d| {
                (d.clamp(0.0, f64::from(crate::species::MAX_LIFESPAN_DAYS))
                    * f64::from(crate::LIFE_DAY_TICKS))
                .round() as u32
            });
            let alleles = match &c.morph {
                Some(name) => {
                    let m = world
                        .species()
                        .get(species)
                        .and_then(|s| s.morph(name))
                        .ok_or_else(|| ScriptError::UnknownMorph {
                            species: c.species.clone(),
                            morph: name.clone(),
                        })?;
                    Some([m, m])
                }
                None => None,
            };
            let traits = crate::Traits {
                age,
                female: c.sex.map(|s| s == Sex::Female),
                alleles,
            };
            for i in 0..i64::from(c.count) {
                let x = c.x.saturating_add(c.dx.saturating_mul(i));
                world.add_creature_with_traits(species, x, c.y, traits)?;
            }
        }
        Ok(())
    }

    /// Ticks the world from its current tick up to `until`, applying pours
    /// on the way.
    ///
    /// # Errors
    ///
    /// Returns an error if a pour names an unknown material.
    pub fn run(&mut self, world: &mut World, until: u64) -> Result<(), ScriptError> {
        while world.tick_count() < until {
            self.apply(world, world.tick_count())?;
            world.tick();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MaterialTable;

    const SCRIPT: &str = r#"
        ticks = 60
        [world]
        width = 64
        height = 48
        [[pour]]
        tick = 2
        repeat = 10
        x = 20
        y = 8
        dx = 2
        material = "sand"
    "#;

    fn world(script: &Script, seed: u64) -> World {
        World::new(script.world_config(seed), MaterialTable::builtin().unwrap()).unwrap()
    }

    #[test]
    fn parses_defaults() {
        let s = Script::parse("").unwrap();
        assert_eq!((s.ticks, s.frame_every), (0, 1));
        assert_eq!((s.world.width, s.world.height), (160, 90));
        let s = Script::parse(SCRIPT).unwrap();
        assert_eq!(s.pours[0].radius, 2);
        assert_eq!(s.pours[0].brush, BrushName::Pour);
    }

    #[test]
    fn rejects_unknown_keys_and_materials() {
        assert!(Script::parse("tickz = 3").is_err());
        let s = Script::parse("[[pour]]\nx = 1\ny = 1\nmaterial = \"lava\"").unwrap();
        let mut w = world(&s, 1);
        assert!(matches!(
            s.run(&mut w, 1),
            Err(ScriptError::UnknownMaterial(_))
        ));
    }

    #[test]
    fn pours_a_plant_propagule_the_species_lists() {
        let pour = |extra: &str| {
            format!("[[pour]]\nx = 32\ny = 8\nradius = 0\nplant = \"hygrophila\"\n{extra}")
        };
        let s = Script::parse(&pour("propagule = \"seeds\"")).unwrap();
        let mut w = world(&s, 1);
        s.run(&mut w, 1).unwrap();
        let seed = w.materials().id("seed").unwrap();
        assert_eq!(w.material_counts()[usize::from(seed.0)], 1);
        // Without one, cuttings (hygrophila's default).
        let s = Script::parse(&pour("")).unwrap();
        let mut w = world(&s, 1);
        s.run(&mut w, 1).unwrap();
        let cutting = w.materials().id("cutting").unwrap();
        assert_eq!(w.material_counts()[usize::from(cutting.0)], 1);
        // Rotala only grows from cuttings.
        let s = Script::parse("[[pour]]\nx = 32\ny = 8\nplant = \"rotala\"\npropagule = \"seeds\"")
            .unwrap();
        let mut w = world(&s, 1);
        assert!(matches!(
            s.run(&mut w, 1),
            Err(ScriptError::BadPropagule { .. })
        ));
        // A propagule needs a plant, and a known name.
        assert!(matches!(
            Script::parse("[[pour]]\nx = 1\ny = 1\nmaterial = \"sand\"\npropagule = \"seeds\""),
            Err(ScriptError::PropaguleWithoutPlant("seeds"))
        ));
        assert!(Script::parse(&pour("propagule = \"spores\"")).is_err());
    }

    #[test]
    fn replays_deterministically() {
        let s = Script::parse(SCRIPT).unwrap();
        let (mut a, mut b) = (world(&s, 3), world(&s, 3));
        s.run(&mut a, s.ticks).unwrap();
        s.run(&mut b, 20).unwrap();
        s.run(&mut b, s.ticks).unwrap();
        assert_eq!(a.state_hash(), b.state_hash());
        let sand = a.materials().id("sand").unwrap();
        assert!(a.material_counts()[usize::from(sand.0)] > 50);
    }

    #[test]
    fn rejects_zero_frame_every_and_survives_extreme_coordinates() {
        assert!(matches!(
            Script::parse("frame_every = 0"),
            Err(ScriptError::ZeroFrameEvery)
        ));
        assert_eq!(Script::default().frame_every, 1);
        let s = Script::parse(&format!(
            "[[pour]]\nx = {}\ny = {}\nmaterial = \"sand\"\n[[pour]]\nx = {}\ny = 0\ndx = {}\nrepeat = 3\nmaterial = \"sand\"",
            i64::MAX,
            i64::MIN,
            i64::MIN,
            i64::MAX
        ))
        .unwrap();
        let mut w = world(&s, 1);
        s.run(&mut w, 3).unwrap();
    }

    #[test]
    fn places_devices() {
        let s = Script::parse("[[place]]\ntick = 1\nx = 10\ny = 5\ndevice = \"bubbler\"").unwrap();
        let mut w = world(&s, 1);
        let bubbler = w.materials().id("bubbler").unwrap();
        s.run(&mut w, 1).unwrap();
        assert_eq!(w.material_counts()[usize::from(bubbler.0)], 0);
        s.run(&mut w, 2).unwrap();
        assert_eq!(w.material_counts()[usize::from(bubbler.0)], 8);
        // Rejected up front, not mid-run.
        assert!(matches!(
            Script::parse("[[place]]\ntick = 2\nx = 1\ny = 1\ndevice = \"heater\""),
            Err(ScriptError::UnknownDevice(_))
        ));
    }

    #[test]
    fn changes_the_water() {
        let s = Script::parse(
            "[[pour]]\nx = 30\ny = 40\nradius = 3\nmaterial = \"detritus\"\n[[water_change]]\ntick = 5",
        )
        .unwrap();
        let mut w = world(&s, 1);
        let detritus = w.materials().id("detritus").unwrap();
        s.run(&mut w, 5).unwrap();
        assert!(w.material_counts()[usize::from(detritus.0)] > 0);
        s.run(&mut w, 6).unwrap();
        assert_eq!(w.material_counts()[usize::from(detritus.0)], 0);
    }

    #[test]
    fn adds_creatures() {
        let s = Script::parse(
            "[[creature]]\ntick = 2\nspecies = \"neon_tetra\"\nx = 20\ny = 40\ncount = 3\ndx = 10",
        )
        .unwrap();
        let mut w = world(&s, 1);
        s.run(&mut w, 2).unwrap();
        assert_eq!(w.creatures().len(), 0);
        s.run(&mut w, 3).unwrap();
        assert_eq!(w.creatures().len(), 3);
        let bad = Script::parse("[[creature]]\nspecies = \"shark\"\nx = 1\ny = 40").unwrap();
        assert!(matches!(
            bad.run(&mut world(&bad, 1), 1),
            Err(ScriptError::UnknownSpecies(_))
        ));
        assert!(matches!(
            Script::parse("[[creature]]\nspecies = \"betta\"\nx = 1\ny = 1\ncount = 999"),
            Err(ScriptError::TooMany(999))
        ));
        // Sex and morph (T9 part 2).
        let s = Script::parse(
            "[[creature]]\nspecies = \"cherry_shrimp\"\nx = 20\ny = 40\ncount = 2\ndx = 10\n\
             sex = \"female\"\nmorph = \"blue\"",
        )
        .unwrap();
        let mut w = world(&s, 1);
        s.run(&mut w, 1).unwrap();
        let blue = w
            .species()
            .get(w.creatures()[0].species())
            .unwrap()
            .morph("blue");
        for c in w.creatures() {
            assert!(c.female());
            assert_eq!(Some(w.creature_morph(c)), blue);
        }
        let bad = Script::parse(
            "[[creature]]\nspecies = \"cherry_shrimp\"\nx = 20\ny = 40\nmorph = \"tartan\"",
        )
        .unwrap();
        assert!(matches!(
            bad.run(&mut world(&bad, 1), 1),
            Err(ScriptError::UnknownMorph { .. })
        ));
    }

    #[test]
    fn pours_only_within_their_window() {
        let s = Script::parse(SCRIPT).unwrap();
        let mut w = world(&s, 3);
        s.run(&mut w, 2).unwrap();
        let sand = w.materials().id("sand").unwrap();
        assert_eq!(w.material_counts()[usize::from(sand.0)], 0);
    }
}
