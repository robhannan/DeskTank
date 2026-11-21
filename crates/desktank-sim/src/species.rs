//! Creature species, loaded from TOML (SPEC §2.4, D8): size, depth zone,
//! surface needs, speed, diet and placeholder pixel art (D11).
//!
//! Each species is one file in `assets/species/`. Its sprite is pixel art
//! written as rows of characters, one per texel, with a colour key; the
//! renderer packs every species' frames into one atlas texture. Sprites
//! are drawn at [`TEXELS_PER_CELL`] texels per cell side, so their sides
//! must be even, and the sprite sets the creature's body: the cells it
//! covers and collides with (half its texel size). Every sprite also gives
//! `fine_frames`: the same poses drawn at four texels per cell side (twice
//! the width and height), which the renderer uses at display scales
//! divisible by four. A species may also glow at night (`[glow]`): an
//! emissive colour the renderer adds after the daylight tint.
//!
//! Units (SPEC §2.1): speeds are in cells per second of world time, depths
//! in cells or as a fraction of the water column (0 the surface, 1 the
//! floor).

use std::collections::BTreeMap;

use serde::Deserialize;
use thiserror::Error;

use crate::material::{MaterialId, MaterialTable};
use crate::world::DEPTH_BANDS;

/// Index of a species in the [`SpeciesTable`]; fixed in each species' TOML
/// file so saves stay stable when species are added.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SpeciesId(pub u8);

/// Sprite texels per cell side: creatures are drawn at twice the grid's
/// resolution, so a 14×6-texel sprite has a 7×3-cell body.
pub const TEXELS_PER_CELL: usize = 2;
/// Largest sprite side, in texels (32 cells, 16 cm; the fine frames are
/// twice it).
pub const MAX_SPRITE_SIDE: usize = 64;
/// Cells per centimetre of real-world size: every species is drawn at this
/// scale, so a 3.5 cm neon tetra is 7 cells long and sits beside a 2.5 cm
/// cherry shrimp (5 cells) as it would in a tank.
pub const CELLS_PER_CM: f32 = 2.0;
/// Smallest and largest territory radius a species may give, in cm.
const MIN_TERRITORY_CM: f32 = 2.0;
const MAX_TERRITORY_CM: f32 = 50.0;
/// Largest palette icon side, in pixels (a palette slot's face).
pub const MAX_ICON_SIDE: usize = 8;
/// Fastest allowed speed, in cells per second: under one cell per tick, so
/// a body never skips past a cell it would collide with.
pub const MAX_SPEED: f32 = 16.0;
/// Farthest a species reaches to eat, in cells past its body.
pub const MAX_REACH: u8 = 4;
/// Longest lifespan a species may give, in real days (20 years).
pub const MAX_LIFESPAN_DAYS: u32 = 7300;
/// The range of [`Species::air_tolerance_seconds`]: half a minute to two
/// hours.
const AIR_TOLERANCE_SECONDS: std::ops::RangeInclusive<u32> = 30..=7200;
/// Most colour morphs a species may have (SPEC §2.4, D13).
pub const MAX_MORPHS: usize = 8;
/// Most young in one brood or clutch.
pub const MAX_BROOD: u8 = 16;
/// Shortest `swim_every` a species may give, in seconds (over twice a
/// swim, so a bottom dweller spends most of its time on the bottom).
pub const MIN_SWIM_EVERY: f32 = 10.0;
/// Shortest `burrow_every` a species may give, in seconds (twice a burrow).
pub const MIN_BURROW_EVERY: f32 = 32.0;

/// The part of the tank a species lives in (SPEC §2.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Zone {
    /// Just under the surface.
    Surface,
    /// Open water.
    Mid,
    /// Near the floor.
    Bottom,
    /// Submerged faces of hardscape, plants and tank walls.
    Surfaces,
}

/// How a species gets on with others (SPEC §2.4). Schoolers follow their own kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Temperament {
    /// Swims in a group of its own kind.
    Schooling,
    /// Gets on with everyone.
    Peaceful,
    /// Defends a patch of the tank.
    Territorial,
}

/// How a species breeds (SPEC §2.4, T9 part 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BreedingMode {
    /// The female carries her young (or, for a shrimp, her eggs under her
    /// tail) for `days` and releases them as fry.
    Livebearer,
    /// The female lays a clutch of eggs (the `egg` material) beside her,
    /// which hatch into fry after `days`.
    Eggs,
}

/// A species' breeding, validated. Species without one don't breed in a
/// freshwater tank (a nerite's eggs and an amano's larvae need brackish
/// water).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Breeding {
    /// Livebearer or egg layer.
    pub mode: BreedingMode,
    /// Days a female carries her young, or a clutch takes to hatch.
    pub days: u32,
    /// Fewest and most young in a brood or clutch (1..=[`MAX_BROOD`]).
    pub brood: [u8; 2],
    /// Days a female rests after a brood or clutch before she breeds again.
    pub rest_days: u32,
}

/// A species' night glow (`[glow]` in its TOML): an emissive colour the
/// renderer adds after the daylight tint, scaled by how dark the tank is,
/// so it shows at night, faintly at dusk and not at all by day.
#[derive(Clone, Debug, PartialEq)]
pub struct Glow {
    /// The glow colour (sRGB).
    pub colour: [u8; 3],
    /// How strongly it glows at full darkness, in `(0, 1]`.
    pub strength: f32,
    /// Whether it pulses slowly (between about 82% and 100%).
    pub pulse: bool,
    /// Sprite colours (from the keys in `eyes`) that glow as eye whites:
    /// brighter than the body.
    pub eyes: Vec<[u8; 3]>,
    /// Sprite colours (from the keys in `unlit`) that don't glow, such as
    /// pupils.
    pub unlit: Vec<[u8; 3]>,
}

/// One look of a species: a colour morph (SPEC §2.4, D13), drawn for one
/// sex. Its frames and fine frames are the sprite's (or the female's),
/// recoloured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Look {
    /// The morph's index in [`Species::morphs`].
    pub morph: u8,
    /// Whether this is the female's look (only species drawn differently
    /// by sex have one).
    pub female: bool,
    /// Animation frames, as [`Sprite::frames`].
    pub frames: Vec<Vec<[u8; 4]>>,
    /// The same poses at twice the size, as [`Sprite::fine_frames`].
    pub fine_frames: Vec<Vec<[u8; 4]>>,
}

/// Pixel art for a species, facing right: `frames` of `width × height`
/// RGBA texels (sRGB; alpha 0 or 255), row-major from the top left. Both
/// sides are even ([`TEXELS_PER_CELL`] texels per cell). `fine_frames` are
/// the same poses at twice the width and height.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sprite {
    /// Width in texels.
    pub width: usize,
    /// Height in texels.
    pub height: usize,
    /// Animation frames.
    pub frames: Vec<Vec<[u8; 4]>>,
    /// The same poses at twice the width and height (four texels per cell
    /// side), one for each of `frames`.
    pub fine_frames: Vec<Vec<[u8; 4]>>,
    /// The palette icon: hand-drawn at one pixel per cell, at most
    /// [`MAX_ICON_SIDE`] a side, in the same colours.
    pub icon: Icon,
}

/// A species' palette icon: `width × height` RGBA pixels (alpha 0 or 255),
/// row-major from the top left.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Icon {
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// The pixels.
    pub pixels: Vec<[u8; 4]>,
}

/// One species, validated.
#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::struct_excessive_bools)] // Independent species traits from the catalogue.
pub struct Species {
    /// Stable ID.
    pub id: SpeciesId,
    /// Unique name, e.g. `"neon_tetra"`.
    pub name: String,
    /// The zone it lives in.
    pub zone: Zone,
    /// The band of the water column it keeps to, as fractions from the
    /// surface (0) to the floor (1); `depth[0] < depth[1]`.
    pub depth: [f32; 2],
    /// Front and back depth bands it keeps to, inclusive (D21): `0` is the
    /// front glass, [`DEPTH_BANDS`] − 1 the back. Every band by default.
    pub bands: [u8; 2],
    /// Water depth it needs, in cells (SPEC §2.1).
    pub min_depth: usize,
    /// Whether it needs to reach air at the surface to breathe.
    pub breathes_air: bool,
    /// For an air-breather, world seconds it lasts once a breath is due
    /// and it can't reach air (0 for the rest): it builds up stress
    /// towards [`crate::STRESS_LIMIT`] over this time and dies of
    /// suffocation at the end. Short for obligate air-breathers (frogs),
    /// long for labyrinth fish, whose gills still take some oxygen.
    pub air_tolerance_seconds: u32,
    /// Temperament.
    pub temperament: Temperament,
    /// Cruising speed, cells per second.
    pub speed: f32,
    /// Top speed (chasing food), cells per second.
    pub max_speed: f32,
    /// Materials it eats.
    pub diet: Vec<MaterialId>,
    /// Real-world body length in cm, nose to tail without antennae or an
    /// outsized fin tail such as a male guppy's (a crab's carapace), drawn
    /// at [`CELLS_PER_CM`]: the sprite is at least this long and adds only
    /// what lies outside it, such as antennae, fins or a crab's legs.
    pub length_cm: f32,
    /// How far past its body it takes food, in cells (1..=[`MAX_REACH`]):
    /// 1 for a mouth at the body's edge, more for a filter feeder's fans.
    pub reach: u8,
    /// Seconds between a bottom walker's hops off the substrate (shrimp),
    /// or `None` if it never hops.
    pub hop_every: Option<f32>,
    /// Seconds between a bottom dweller's chances to swim (`None`: it can't
    /// swim, like a snail). A bottom walker or surface crawler that can
    /// swim but would rather not (shrimp, plecos, loaches) lifts off at
    /// one chance in two, swims a short way and settles again.
    pub swim_every: Option<f32>,
    /// Seconds between a bottom walker's chances to burrow into loose
    /// substrate (`None`: it never burrows; the kuhli loach does).
    pub burrow_every: Option<f32>,
    /// Real-world lifespan in days (1..=[`MAX_LIFESPAN_DAYS`]), on the life
    /// clock (SPEC §2.4, T9: a day per `LIFE_DAY_SECONDS` of world time).
    /// Each creature's own lifespan is within ±10% of it.
    pub lifespan_days: u32,
    /// Real-world age at maturity in days (below the lifespan): a well-fed
    /// creature is an adult from this age.
    pub maturity_days: u32,
    /// Seconds between droppings of waste (its bioload), or `None` for a
    /// species that leaves none (every stocked species so far).
    pub waste_every: Option<f32>,
    /// Whether it also eats corpses within reach, a bite at a time.
    pub eats_corpses: bool,
    /// The carpet plant species (by name) a bottom walker plants at a home
    /// it picks on the substrate, then keeps near; `None` for no home.
    pub home_carpet: Option<String>,
    /// Added only by its own cheat (SPEC §2.5): left out of the palette,
    /// the `spawn` cheat and stocking, and it never breeds.
    pub cheat_only: bool,
    /// Live food (bloodworms; SPEC §2.4 *Live food*): offered from the
    /// Food palette group a pinch at a time, never stocked, bred or
    /// counted against the population cap. It is eaten whole by any
    /// species at least as long whose diet holds [`Species::food_as`],
    /// and dies into a cell of that material.
    pub live_food: bool,
    /// The food material a live food counts as and dies into (`None` for
    /// other species, or a live food whose material the table lacks).
    pub food_as: Option<MaterialId>,
    /// Longest prey it eats, in cm (`None`: it eats no creatures). A
    /// hungry adult takes any smaller creature it touches: fry, shrimp
    /// (SPEC §2.4, T9 part 2).
    pub prey_cm: Option<f32>,
    /// Radius in cm of the territory a `territorial` adult claims where it
    /// settles and drives other fish out of (`None`: it keeps none; only
    /// territorial species may set it).
    pub territory_cm: Option<f32>,
    /// How it breeds, or `None` if it doesn't in a freshwater tank.
    pub breeding: Option<Breeding>,
    /// Its night glow, or `None` (every species but the blinker).
    pub glow: Option<Glow>,
    /// Its colour morphs by name, in allele order (append only: saves
    /// store the index). A species with no `[genetics]` has one, `"wild"`.
    pub morphs: Vec<String>,
    /// Index of the wildtype, which mixed lines revert to.
    pub wildtype: u8,
    /// Index of the morph the palette adds: the sprite's own colours.
    pub palette_morph: u8,
    /// Stock successive palette additions in different inherited colours.
    pub cycle_palette: bool,
    /// Every look: each morph for a male (or either sex), then for a
    /// female if the species is drawn differently by sex. See
    /// [`Species::look`].
    pub looks: Vec<Look>,
    /// Its pixel art; half its size in texels is the body size in cells.
    pub sprite: Sprite,
    /// Its main colour (the commonest in the first frame), for swatches.
    pub colour: [u8; 3],
}

impl Species {
    /// Whether it is a pet the palette, `spawn` and stocking offer: neither
    /// cheat-only nor live food.
    #[must_use]
    pub fn stocked(&self) -> bool {
        !self.cheat_only && !self.live_food
    }

    /// Body size in cells, `(width, height)`: the sprite's size in texels
    /// over [`TEXELS_PER_CELL`].
    #[must_use]
    pub fn size(&self) -> (usize, usize) {
        (
            self.sprite.width / TEXELS_PER_CELL,
            self.sprite.height / TEXELS_PER_CELL,
        )
    }

    /// Whether it eats `material`.
    #[must_use]
    pub fn eats(&self, material: MaterialId) -> bool {
        self.diet.contains(&material)
    }

    /// Droppings per minute of world time (its bioload): 0 for a species
    /// that leaves no waste.
    #[must_use]
    pub fn bioload(&self) -> f32 {
        self.waste_every.map_or(0.0, |every| 60.0 / every)
    }

    /// Whether females are drawn differently from males.
    #[must_use]
    pub fn dimorphic(&self) -> bool {
        self.looks.len() > self.morphs.len()
    }

    /// Index in [`Species::looks`] of `morph` drawn for a female or a male
    /// (the male's look for a species drawn alike; morph 0 for an unknown
    /// morph).
    #[must_use]
    pub fn look(&self, morph: u8, female: bool) -> usize {
        let morph = if usize::from(morph) < self.morphs.len() {
            usize::from(morph)
        } else {
            0
        };
        if female && self.dimorphic() {
            self.morphs.len() + morph
        } else {
            morph
        }
    }

    /// The index of the morph with this name.
    #[must_use]
    pub fn morph(&self, name: &str) -> Option<u8> {
        self.morphs
            .iter()
            .position(|m| m == name)
            .and_then(|i| u8::try_from(i).ok())
    }
}

/// Error loading or validating species.
#[derive(Debug, Error)]
pub enum SpeciesError {
    /// A TOML source didn't parse or didn't match the schema.
    #[error("species source {index}: {source}")]
    Parse {
        /// Index of the source in the list passed in.
        index: usize,
        /// The parse error.
        #[source]
        source: toml::de::Error,
    },
    /// Two species share an ID.
    #[error("duplicate species id {0}")]
    DuplicateId(u8),
    /// Two species share a name.
    #[error("duplicate species name {0:?}")]
    DuplicateName(String),
    /// A property is out of range.
    #[error("species {name:?}: {reason}")]
    Invalid {
        /// Species name.
        name: String,
        /// What is wrong.
        reason: &'static str,
    },
    /// The diet names a material that doesn't exist.
    #[error("species {name:?}: unknown food {food:?}")]
    UnknownFood {
        /// Species name.
        name: String,
        /// The missing material.
        food: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpeciesDef {
    id: u8,
    name: String,
    zone: Zone,
    depth: [f32; 2],
    #[serde(default = "every_band")]
    bands: [u8; 2],
    min_depth: usize,
    #[serde(default)]
    breathes_air: bool,
    #[serde(default)]
    air_tolerance_seconds: Option<u32>,
    temperament: Temperament,
    speed: f32,
    max_speed: f32,
    diet: Vec<String>,
    length_cm: f32,
    #[serde(default = "default_reach")]
    reach: u8,
    #[serde(default)]
    hop_every: Option<f32>,
    #[serde(default)]
    swim_every: Option<f32>,
    #[serde(default)]
    burrow_every: Option<f32>,
    lifespan_days: u32,
    maturity_days: u32,
    #[serde(default)]
    waste_every: Option<f32>,
    #[serde(default)]
    eats_corpses: bool,
    #[serde(default)]
    home_carpet: Option<String>,
    #[serde(default)]
    cheat_only: bool,
    /// The food material a live food counts as (makes it live food).
    #[serde(default)]
    live_food: Option<String>,
    prey_cm: Option<f32>,
    #[serde(default)]
    territory_cm: Option<f32>,
    #[serde(default)]
    breeding: Option<BreedingDef>,
    #[serde(default)]
    genetics: Option<GeneticsDef>,
    #[serde(default)]
    glow: Option<GlowDef>,
    sprite: SpriteDef,
}

fn every_band() -> [u8; 2] {
    [0, DEPTH_BANDS - 1]
}

fn default_reach() -> u8 {
    1
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BreedingDef {
    mode: BreedingMode,
    days: u32,
    brood: [u8; 2],
    rest_days: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GlowDef {
    colour: String,
    strength: f32,
    #[serde(default)]
    pulse: bool,
    /// Sprite colour keys that glow as eye whites.
    #[serde(default)]
    eyes: Vec<String>,
    /// Sprite colour keys that don't glow.
    #[serde(default)]
    unlit: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GeneticsDef {
    #[serde(default)]
    cycle_palette: bool,
    /// The wildtype morph's name.
    wildtype: String,
    /// The name of the morph the sprite's colours draw (the palette's).
    sprite: String,
    morph: Vec<MorphDef>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MorphDef {
    name: String,
    /// Colours replacing the sprite's, by key.
    #[serde(default)]
    colours: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpriteDef {
    colours: BTreeMap<String, String>,
    frames: Vec<Vec<String>>,
    #[serde(default)]
    fine_frames: Vec<Vec<String>>,
    icon: Vec<String>,
    /// Frames drawn for a female instead (the same size and count), for
    /// species whose sexes look different.
    #[serde(default)]
    female_frames: Option<Vec<Vec<String>>>,
    /// The female's poses at twice the size, one for each of
    /// `female_frames` (required with them).
    #[serde(default)]
    female_fine_frames: Option<Vec<Vec<String>>>,
    /// Colours replacing the sprite's (after the morph's) on a female.
    #[serde(default)]
    female: Option<BTreeMap<String, String>>,
}

/// The species embedded in the binary (T7a placeholders, D11).
const BUILTIN: &[&str] = &[
    include_str!("../../../assets/species/neon_tetra.toml"),
    include_str!("../../../assets/species/corydoras.toml"),
    include_str!("../../../assets/species/betta.toml"),
    include_str!("../../../assets/species/nerite.toml"),
    include_str!("../../../assets/species/cherry_shrimp.toml"),
    include_str!("../../../assets/species/amano_shrimp.toml"),
    include_str!("../../../assets/species/thai_micro_crab.toml"),
    include_str!("../../../assets/species/honey_gourami.toml"),
    include_str!("../../../assets/species/guppy.toml"),
    include_str!("../../../assets/species/harlequin_rasbora.toml"),
    include_str!("../../../assets/species/otocinclus.toml"),
    include_str!("../../../assets/species/medical_crab.toml"),
    include_str!("../../../assets/species/home_owning_crab.toml"),
    include_str!("../../../assets/species/blinker.toml"),
    include_str!("../../../assets/species/common_goldfish.toml"),
    include_str!("../../../assets/species/fancy_goldfish.toml"),
    include_str!("../../../assets/species/angelfish.toml"),
    include_str!("../../../assets/species/pea_puffer.toml"),
    include_str!("../../../assets/species/flowerhorn.toml"),
    include_str!("../../../assets/species/bristlenose_pleco.toml"),
    include_str!("../../../assets/species/kuhli_loach.toml"),
    include_str!("../../../assets/species/mystery_snail.toml"),
    include_str!("../../../assets/species/ramshorn_snail.toml"),
    include_str!("../../../assets/species/african_dwarf_frog.toml"),
    include_str!("../../../assets/species/axolotl.toml"),
    include_str!("../../../assets/species/common_frog.toml"),
    include_str!("../../../assets/species/live_bloodworm.toml"),
];

/// All species, ordered by ID.
#[derive(Clone, Debug, PartialEq)]
pub struct SpeciesTable {
    species: Vec<Species>,
}

impl SpeciesTable {
    /// The species embedded from `assets/species/*.toml`, with diets looked
    /// up in `materials`. Foods the table lacks (a test's own material set)
    /// are left out of the diets.
    ///
    /// # Errors
    ///
    /// Returns an error if an embedded file is invalid (a build defect,
    /// covered by tests).
    pub fn builtin_for(materials: &MaterialTable) -> Result<Self, SpeciesError> {
        Self::load(BUILTIN, materials, true)
    }

    /// Parses one TOML document per species and validates the set.
    ///
    /// # Errors
    ///
    /// Returns an error if a source doesn't parse, a property is invalid,
    /// IDs or names clash, or a diet names an unknown material.
    pub fn from_toml_sources(
        sources: &[&str],
        materials: &MaterialTable,
    ) -> Result<Self, SpeciesError> {
        Self::load(sources, materials, false)
    }

    fn load(
        sources: &[&str],
        materials: &MaterialTable,
        skip_unknown_food: bool,
    ) -> Result<Self, SpeciesError> {
        let mut defs = Vec::with_capacity(sources.len());
        for (index, src) in sources.iter().enumerate() {
            let def: SpeciesDef =
                toml::from_str(src).map_err(|source| SpeciesError::Parse { index, source })?;
            defs.push(def);
        }
        defs.sort_by_key(|d| d.id);
        for pair in defs.windows(2) {
            if pair[0].id == pair[1].id {
                return Err(SpeciesError::DuplicateId(pair[0].id));
            }
        }
        for (i, d) in defs.iter().enumerate() {
            if defs[..i].iter().any(|o| o.name == d.name) {
                return Err(SpeciesError::DuplicateName(d.name.clone()));
            }
        }
        let species = defs
            .iter()
            .map(|d| validate(d, materials, skip_unknown_food))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { species })
    }

    /// The species with this ID, if any.
    #[must_use]
    pub fn get(&self, id: SpeciesId) -> Option<&Species> {
        self.species
            .binary_search_by_key(&id, |s| s.id)
            .ok()
            .map(|i| &self.species[i])
    }

    /// The ID of the species with this name, if any.
    #[must_use]
    pub fn id(&self, name: &str) -> Option<SpeciesId> {
        self.species.iter().find(|s| s.name == name).map(|s| s.id)
    }

    /// All species, ordered by ID.
    pub fn iter(&self) -> impl Iterator<Item = &Species> {
        self.species.iter()
    }

    /// Number of species.
    #[must_use]
    pub fn len(&self) -> usize {
        self.species.len()
    }

    /// Whether there are no species.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.species.is_empty()
    }
}

#[allow(clippy::too_many_lines)] // one check per property
fn validate(
    d: &SpeciesDef,
    materials: &MaterialTable,
    skip_unknown_food: bool,
) -> Result<Species, SpeciesError> {
    let invalid = |reason| SpeciesError::Invalid {
        name: d.name.clone(),
        reason,
    };
    let [top, bottom] = d.depth;
    if !(0.0..=1.0).contains(&top) || !(0.0..=1.0).contains(&bottom) || top >= bottom {
        return Err(invalid(
            "depth must be [top, bottom] within 0..=1 with top < bottom",
        ));
    }
    if d.bands[0] > d.bands[1] || d.bands[1] >= DEPTH_BANDS {
        return Err(invalid(
            "bands must be [front, back] within 0..DEPTH_BANDS (8) with front <= back",
        ));
    }
    if !(d.speed.is_finite() && d.speed > 0.0 && d.speed <= d.max_speed) {
        return Err(invalid("speed must be > 0 and at most max_speed"));
    }
    if d.max_speed > MAX_SPEED {
        return Err(invalid("max_speed must be at most 16 cells per second"));
    }
    if !(1..=MAX_REACH).contains(&d.reach) {
        return Err(invalid("reach must be 1..=MAX_REACH (4) cells"));
    }
    if let Some(every) = d.hop_every {
        if d.zone != Zone::Bottom {
            return Err(invalid("only bottom walkers hop"));
        }
        if !(every.is_finite() && (1.0..=600.0).contains(&every)) {
            return Err(invalid("hop_every must be 1..=600 seconds"));
        }
    }
    if let Some(every) = d.swim_every {
        if !matches!(d.zone, Zone::Bottom | Zone::Surfaces) {
            return Err(invalid(
                "only bottom walkers and surface crawlers swim now and then",
            ));
        }
        if !(every.is_finite() && (MIN_SWIM_EVERY..=600.0).contains(&every)) {
            return Err(invalid("swim_every must be 10..=600 seconds"));
        }
    }
    if let Some(every) = d.burrow_every {
        if d.zone != Zone::Bottom {
            return Err(invalid("only bottom walkers burrow"));
        }
        if !(every.is_finite() && (MIN_BURROW_EVERY..=600.0).contains(&every)) {
            return Err(invalid("burrow_every must be 32..=600 seconds"));
        }
    }
    if !(1..=MAX_LIFESPAN_DAYS).contains(&d.lifespan_days)
        || d.maturity_days == 0
        || d.maturity_days >= d.lifespan_days
    {
        return Err(invalid(
            "lifespan_days must be 1..=7300 and maturity_days 1..lifespan_days",
        ));
    }
    if let Some(every) = d.waste_every
        && !(every.is_finite() && (1.0..=600.0).contains(&every))
    {
        return Err(invalid("waste_every must be 1..=600 seconds"));
    }
    match (d.breathes_air, d.air_tolerance_seconds) {
        (true, Some(t)) if AIR_TOLERANCE_SECONDS.contains(&t) => {}
        (false, None) => {}
        _ => {
            return Err(invalid(
                "an air-breather needs air_tolerance_seconds (30..=7200), and only an air-breather",
            ));
        }
    }
    if d.home_carpet.is_some() && d.zone != Zone::Bottom {
        return Err(invalid("only bottom walkers keep a home"));
    }
    let mut diet = Vec::with_capacity(d.diet.len());
    for food in &d.diet {
        match materials.id(food) {
            Some(id) => diet.push(id),
            None if skip_unknown_food => {}
            None => {
                return Err(SpeciesError::UnknownFood {
                    name: d.name.clone(),
                    food: food.clone(),
                });
            }
        }
    }
    let food_as = match &d.live_food {
        None => None,
        Some(food) => {
            if d.cheat_only
                || d.breeding.is_some()
                || d.genetics.is_some()
                || d.prey_cm.is_some()
                || d.territory_cm.is_some()
                || !d.diet.is_empty()
            {
                return Err(invalid(
                    "live food has an empty diet, no breeding, genetics, prey or territory, \
                     and isn't cheat-only",
                ));
            }
            match materials.id(food) {
                Some(id) => Some(id),
                None if skip_unknown_food => None,
                None => {
                    return Err(SpeciesError::UnknownFood {
                        name: d.name.clone(),
                        food: food.clone(),
                    });
                }
            }
        }
    };
    let sprite = sprite(&d.sprite).map_err(invalid)?;
    // Sprite sides are at most 64 texels: exact in f32.
    #[allow(clippy::cast_precision_loss)]
    let drawn_cm = sprite.width.max(sprite.height) as f32 / TEXELS_PER_CELL as f32 / CELLS_PER_CM;
    if !(d.length_cm.is_finite() && d.length_cm > 0.0 && d.length_cm <= drawn_cm) {
        return Err(invalid(
            "length_cm must be > 0 and fit the sprite at CELLS_PER_CM",
        ));
    }
    if let Some(prey) = d.prey_cm
        && !(prey.is_finite() && prey > 0.0 && prey < d.length_cm)
    {
        return Err(invalid("prey_cm must be > 0 and below length_cm"));
    }
    if let Some(radius) = d.territory_cm
        && !(radius.is_finite()
            && (MIN_TERRITORY_CM..=MAX_TERRITORY_CM).contains(&radius)
            && d.temperament == Temperament::Territorial)
    {
        return Err(invalid(
            "territory_cm must be 2..=50 and only for a territorial species",
        ));
    }
    let breeding = d
        .breeding
        .as_ref()
        .map(breeding)
        .transpose()
        .map_err(invalid)?;
    let glow = d
        .glow
        .as_ref()
        .map(|g| glow(g, &d.sprite.colours))
        .transpose()
        .map_err(invalid)?;
    let (morphs, wildtype, palette_morph, looks) = looks(d, &sprite).map_err(invalid)?;
    let colour = main_colour(&sprite);
    Ok(Species {
        id: SpeciesId(d.id),
        name: d.name.clone(),
        zone: d.zone,
        depth: d.depth,
        bands: d.bands,
        min_depth: d.min_depth,
        breathes_air: d.breathes_air,
        air_tolerance_seconds: d.air_tolerance_seconds.unwrap_or(0),
        temperament: d.temperament,
        speed: d.speed,
        max_speed: d.max_speed,
        diet,
        length_cm: d.length_cm,
        reach: d.reach,
        hop_every: d.hop_every,
        swim_every: d.swim_every,
        burrow_every: d.burrow_every,
        lifespan_days: d.lifespan_days,
        maturity_days: d.maturity_days,
        waste_every: d.waste_every,
        eats_corpses: d.eats_corpses,
        home_carpet: d.home_carpet.clone(),
        cheat_only: d.cheat_only,
        live_food: d.live_food.is_some(),
        food_as,
        prey_cm: d.prey_cm,
        territory_cm: d.territory_cm,
        breeding,
        glow,
        morphs,
        wildtype,
        palette_morph,
        cycle_palette: d.genetics.as_ref().is_some_and(|g| g.cycle_palette),
        looks,
        sprite,
        colour,
    })
}

fn breeding(b: &BreedingDef) -> Result<Breeding, &'static str> {
    let [low, high] = b.brood;
    if !(1..=MAX_BROOD).contains(&low) || !(low..=MAX_BROOD).contains(&high) {
        return Err("breeding.brood must be [fewest, most] within 1..=16");
    }
    if !(1..=365).contains(&b.days) || !(1..=365).contains(&b.rest_days) {
        return Err("breeding.days and rest_days must be 1..=365");
    }
    Ok(Breeding {
        mode: b.mode,
        days: b.days,
        brood: b.brood,
        rest_days: b.rest_days,
    })
}

fn glow(g: &GlowDef, colours: &BTreeMap<String, String>) -> Result<Glow, &'static str> {
    let colour =
        crate::material::parse_colour(&g.colour).ok_or("glow.colour must be \"#rrggbb\"")?;
    if !(g.strength.is_finite() && g.strength > 0.0 && g.strength <= 1.0) {
        return Err("glow.strength must be > 0 and at most 1");
    }
    let key = colour_key(colours)?;
    let resolve = |keys: &[String]| {
        keys.iter()
            .map(|k| {
                let mut chars = k.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => key.get(&c).map(|c| [c[0], c[1], c[2]]),
                    _ => None,
                }
            })
            .collect::<Option<Vec<_>>>()
            .ok_or("glow.eyes and glow.unlit must be sprite colour keys")
    };
    Ok(Glow {
        colour,
        strength: g.strength,
        pulse: g.pulse,
        eyes: resolve(&g.eyes)?,
        unlit: resolve(&g.unlit)?,
    })
}

/// The morph names, the wildtype's and the sprite's indices, and every
/// look (each morph for a male, then for a female if the sprite gives
/// female colours).
type Looks = (Vec<String>, u8, u8, Vec<Look>);

fn looks(d: &SpeciesDef, sprite: &Sprite) -> Result<Looks, &'static str> {
    let wild = MorphDef {
        name: "wild".to_owned(),
        colours: BTreeMap::new(),
    };
    let (defs, wildtype, palette) = match &d.genetics {
        Some(g) => {
            let find = |name: &str| g.morph.iter().position(|m| m.name == name);
            let wildtype = find(&g.wildtype).ok_or("genetics.wildtype must name a morph")?;
            let palette = find(&g.sprite).ok_or("genetics.sprite must name a morph")?;
            if !g.morph[palette].colours.is_empty() {
                return Err("the sprite's morph takes the sprite's colours");
            }
            (g.morph.iter().collect::<Vec<_>>(), wildtype, palette)
        }
        None => (vec![&wild], 0, 0),
    };
    if defs.is_empty() || defs.len() > MAX_MORPHS {
        return Err("a species has 1..=8 morphs");
    }
    for (i, m) in defs.iter().enumerate() {
        if m.name.is_empty() || defs[..i].iter().any(|o| o.name == m.name) {
            return Err("morph names must be unique and not empty");
        }
    }
    let base = colour_key(&d.sprite.colours)?;
    let recolour = |key: &mut BTreeMap<char, [u8; 4]>,
                    over: &BTreeMap<String, String>|
     -> Result<(), &'static str> {
        for (c, rgba) in colour_key(over)? {
            let slot = key
                .get_mut(&c)
                .ok_or("morph and female colours must replace a sprite colour")?;
            *slot = rgba;
        }
        Ok(())
    };
    if let Some(f) = &d.sprite.female_frames
        && (f.len() != d.sprite.frames.len() || f.iter().any(|rows| rows.len() != sprite.height))
    {
        return Err("female_frames must match the frames' count and size");
    }
    match (&d.sprite.female_frames, &d.sprite.female_fine_frames) {
        (Some(_), Some(f))
            if f.len() == d.sprite.frames.len()
                && f.iter().all(|rows| rows.len() == sprite.height * 2) => {}
        (None, None) => {}
        _ => return Err("female_frames need female_fine_frames: every pose at twice the size"),
    }
    let sexes: &[bool] = if d.sprite.female.is_some() || d.sprite.female_frames.is_some() {
        &[false, true]
    } else {
        &[false]
    };
    let mut looks = Vec::with_capacity(defs.len() * sexes.len());
    for &female in sexes {
        for (i, m) in defs.iter().enumerate() {
            let mut key = base.clone();
            recolour(&mut key, &m.colours)?;
            if female && let Some(f) = &d.sprite.female {
                recolour(&mut key, f)?;
            }
            let (source, fine) = match (
                &d.sprite.female_frames,
                &d.sprite.female_fine_frames,
                female,
            ) {
                (Some(f), Some(fine), true) => (f, fine),
                _ => (&d.sprite.frames, &d.sprite.fine_frames),
            };
            let decode = |frames: &[Vec<String>], width| {
                frames
                    .iter()
                    .map(|rows| texels(rows, width, &key))
                    .collect::<Result<Vec<_>, _>>()
            };
            looks.push(Look {
                // At most MAX_MORPHS.
                morph: u8::try_from(i).unwrap_or(0),
                female,
                frames: decode(source, sprite.width)?,
                fine_frames: decode(fine, sprite.width * 2)?,
            });
        }
    }
    let index = |i: usize| u8::try_from(i).unwrap_or(0);
    Ok((
        defs.iter().map(|m| m.name.clone()).collect(),
        index(wildtype),
        index(palette),
        looks,
    ))
}

/// A colour key: one character to an opaque colour.
fn colour_key(colours: &BTreeMap<String, String>) -> Result<BTreeMap<char, [u8; 4]>, &'static str> {
    let mut key = BTreeMap::new();
    for (k, v) in colours {
        let mut chars = k.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            return Err("sprite colour keys must be one character");
        };
        if c == '.' {
            return Err("'.' is transparent and can't be a colour key");
        }
        let rgb = crate::material::parse_colour(v).ok_or("sprite colours must be \"#rrggbb\"")?;
        key.insert(c, [rgb[0], rgb[1], rgb[2], 255]);
    }
    Ok(key)
}

/// Turns character rows into RGBA frames.
fn sprite(def: &SpriteDef) -> Result<Sprite, &'static str> {
    let key = colour_key(&def.colours)?;
    let first = def
        .frames
        .first()
        .ok_or("a sprite needs at least one frame")?;
    let height = first.len();
    let width = first.first().map_or(0, |r| r.chars().count());
    if !(2..=MAX_SPRITE_SIDE).contains(&width) || !(2..=MAX_SPRITE_SIDE).contains(&height) {
        return Err("sprite sides must be 2..=64 texels");
    }
    if width % TEXELS_PER_CELL != 0 || height % TEXELS_PER_CELL != 0 {
        return Err("sprite sides must be even (two texels per cell)");
    }
    let decode = |frames: &[Vec<String>], width, height| {
        frames
            .iter()
            .map(|rows| {
                if rows.len() != height {
                    return Err("every frame must be as tall as the first (fine frames twice it)");
                }
                texels(rows, width, &key)
            })
            .collect::<Result<Vec<_>, _>>()
    };
    if def.fine_frames.is_empty() {
        return Err("a sprite needs fine_frames: every pose again at twice the width and height");
    }
    if def.fine_frames.len() != def.frames.len() {
        return Err("fine_frames must have one pose for each of frames");
    }
    let frames = decode(&def.frames, width, height)?;
    let fine_frames = decode(&def.fine_frames, width * 2, height * 2)?;
    let icon_height = def.icon.len();
    let icon_width = def.icon.first().map_or(0, |r| r.chars().count());
    if !(1..=MAX_ICON_SIDE).contains(&icon_width) || !(1..=MAX_ICON_SIDE).contains(&icon_height) {
        return Err("icon sides must be 1..=8 pixels");
    }
    let icon = Icon {
        width: icon_width,
        height: icon_height,
        pixels: texels(&def.icon, icon_width, &key)?,
    };
    Ok(Sprite {
        width,
        height,
        frames,
        fine_frames,
        icon,
    })
}

/// Rows of `width` keyed characters as RGBA texels.
fn texels(
    rows: &[String],
    width: usize,
    key: &BTreeMap<char, [u8; 4]>,
) -> Result<Vec<[u8; 4]>, &'static str> {
    let mut out = Vec::with_capacity(width * rows.len());
    for row in rows {
        if row.chars().count() != width {
            return Err("every sprite row must be the same width");
        }
        for c in row.chars() {
            out.push(match c {
                '.' => [0; 4],
                c => *key
                    .get(&c)
                    .ok_or("sprite uses a character with no colour")?,
            });
        }
    }
    Ok(out)
}

/// The commonest opaque colour of the first frame (the first one wins a
/// tie).
fn main_colour(sprite: &Sprite) -> [u8; 3] {
    let mut counts: Vec<([u8; 4], usize)> = Vec::new();
    for t in sprite.frames.first().into_iter().flatten() {
        if t[3] == 0 {
            continue;
        }
        match counts.iter_mut().find(|(c, _)| c == t) {
            Some((_, n)) => *n += 1,
            None => counts.push((*t, 1)),
        }
    }
    let mut best = ([0x80, 0x80, 0x80, 255], 0);
    for &(c, n) in &counts {
        if n > best.1 {
            best = (c, n);
        }
    }
    [best.0[0], best.0[1], best.0[2]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn materials() -> MaterialTable {
        MaterialTable::builtin().unwrap()
    }

    const OK: &str = r##"
        id = 9
        name = "test_fish"
        zone = "mid"
        depth = [0.2, 0.6]
        min_depth = 10
        temperament = "peaceful"
        length_cm = 1.0
        lifespan_days = 100
        maturity_days = 10
        speed = 2.0
        max_speed = 4.0
        diet = ["flake_food"]
        [sprite]
        colours = { a = "#102030", b = "#405060" }
        frames = [
            ["aabbaa", "aabbaa", ".aa...", ".aa..."],
            ["aabbaa", "aabbaa", "aa..aa", "aa..aa"],
        ]
        icon = ["aab", "a.a"]
        fine_frames = [
            ["aaaabbbbaaaa", "aaaabbbbaaaa", "aaaabbbbaaaa", "aaaabbbbaaaa",
             ".aaaa.......", ".aaaa.......", ".aaaa.......", ".aaaa......."],
            ["aaaabbbbaaaa", "aaaabbbbaaaa", "aaaabbbbaaaa", "aaaabbbbaaaa",
             "aaaa....aaaa", "aaaa....aaaa", "aaaa....aaaa", "aaaa....aaaa"],
        ]
    "##;

    #[test]
    #[allow(clippy::many_single_char_names)]
    fn builtin_species_load() {
        let m = materials();
        let t = SpeciesTable::builtin_for(&m).unwrap();
        assert_eq!(t.len(), 27);
        for name in [
            "neon_tetra",
            "corydoras",
            "betta",
            "nerite",
            "cherry_shrimp",
            "amano_shrimp",
            "thai_micro_crab",
            "honey_gourami",
            "guppy",
            "harlequin_rasbora",
            "otocinclus",
        ] {
            let s = t.get(t.id(name).unwrap()).unwrap();
            assert!(!s.diet.is_empty(), "{name} eats something");
            assert!(s.sprite.frames.len() >= 2, "{name} animates");
            let icon = &s.sprite.icon;
            assert!(icon.pixels.iter().any(|p| p[3] > 0), "{name} has an icon");
            let (w, h) = s.size();
            assert!(w <= 16 && h <= 16, "{name} is at most 16 cells a side");
        }
        // Between them they eat every T5 food.
        for food in ["flake_food", "sinking_pellet", "bloodworm", "algae_wafer"] {
            let id = m.id(food).unwrap();
            assert!(t.iter().any(|s| s.eats(id)), "nobody eats {food}");
        }
        // Live bloodworms are live food counting as frozen bloodworms, eaten
        // by every species that eats those, and not stocked.
        let worm = t.get(t.id("live_bloodworm").unwrap()).unwrap();
        assert!(worm.live_food && !worm.stocked() && !worm.cheat_only);
        assert_eq!(worm.food_as, m.id("bloodworm"));
        assert!(worm.breeding.is_none() && worm.diet.is_empty());
        // Live food lives on the bed: it never rises for air or suffocates.
        assert!(!worm.breathes_air && worm.air_tolerance_seconds == 0);
        assert_eq!(worm.zone, Zone::Bottom);
        assert!((1.0..=2.0).contains(&worm.length_cm));
        assert_eq!(t.iter().filter(|s| s.live_food).count(), 1);
        let betta = t.get(t.id("betta").unwrap()).unwrap();
        assert!(betta.breathes_air);
        assert_eq!(betta.zone, Zone::Surface);
        // Shrimp walk the bottom and hop; the crab crawls and filter-feeds.
        let algae = m.id("algae").unwrap();
        for name in ["cherry_shrimp", "amano_shrimp"] {
            let s = t.get(t.id(name).unwrap()).unwrap();
            assert_eq!(s.zone, Zone::Bottom);
            assert!(s.hop_every.is_some(), "{name} hops");
            assert!(s.eats(algae), "{name} grazes algae");
        }
        let crab = t.get(t.id("thai_micro_crab").unwrap()).unwrap();
        assert_eq!(
            (crab.zone, crab.reach, crab.size()),
            (Zone::Surfaces, 2, (3, 3))
        );
        // Every crawler's body is odd on both sides, so it stays on whole
        // cells after a quarter turn.
        for s in t.iter().filter(|s| s.zone == Zone::Surfaces) {
            let (w, h) = s.size();
            assert!(w % 2 == 1 && h % 2 == 1, "{} has even sides", s.name);
        }
        // The honey gourami breathes air like the betta, slower and over a
        // deeper band; the harlequin schools and the otocinclus crawls and
        // grazes.
        let gourami = t.get(t.id("honey_gourami").unwrap()).unwrap();
        assert!(gourami.breathes_air);
        assert_eq!(gourami.zone, Zone::Surface);
        assert!(gourami.speed < betta.speed && gourami.depth[1] > betta.depth[1]);
        // Frogs need air far sooner than the labyrinth fish.
        for frog in ["african_dwarf_frog", "common_frog"] {
            let frog = t.get(t.id(frog).unwrap()).unwrap();
            assert!(frog.air_tolerance_seconds * 4 <= gourami.air_tolerance_seconds);
            assert!(frog.air_tolerance_seconds * 4 <= betta.air_tolerance_seconds);
        }
        let guppy = t.get(t.id("guppy").unwrap()).unwrap();
        assert_ne!(guppy.temperament, Temperament::Schooling);
        let rasbora = t.get(t.id("harlequin_rasbora").unwrap()).unwrap();
        assert_eq!(
            (rasbora.zone, rasbora.temperament),
            (Zone::Mid, Temperament::Schooling)
        );
        let oto = t.get(t.id("otocinclus").unwrap()).unwrap();
        assert_eq!((oto.zone, oto.size()), (Zone::Surfaces, (7, 3)));
        assert!(oto.eats(algae));
    }

    /// The cheat-only crabs (SPEC §2.5): peaceful bottom walkers that never
    /// breed, eat everything edible that isn't alive and leave far more
    /// waste than any stocked species; the home owner plants a carpet.
    #[test]
    fn the_cheat_only_crabs() {
        let m = materials();
        let t = SpeciesTable::builtin_for(&m).unwrap();
        let get = |name| t.get(t.id(name).unwrap()).unwrap();
        let crabs = [get("medical_crab"), get("home_owning_crab")];
        // Everything edible that isn't alive (and not waste, which would
        // cancel their bioload).
        let foods = [
            "flake_food",
            "sinking_pellet",
            "bloodworm",
            "algae_wafer",
            "algae",
            "detritus",
            "dead_plant",
        ];
        let never = ["leaf", "stem", "root", "runner", "seed", "cutting", "waste"];
        let stocked_bioload = t
            .iter()
            .filter(|s| !s.cheat_only)
            .map(Species::bioload)
            .fold(0.0, f32::max);
        for crab in crabs {
            assert!(crab.cheat_only && crab.breeding.is_none(), "{}", crab.name);
            assert_eq!(
                (crab.zone, crab.temperament, crab.size()),
                (Zone::Bottom, Temperament::Peaceful, (6, 7))
            );
            for food in foods {
                assert!(crab.eats(m.id(food).unwrap()), "{} eats {food}", crab.name);
            }
            for food in never {
                assert!(!crab.eats(m.id(food).unwrap()), "{} eats {food}", crab.name);
            }
            assert!(crab.eats_corpses);
            assert!(crab.bioload() >= 10.0 * stocked_bioload.max(1.0));
        }
        assert_eq!(crabs[0].home_carpet, None);
        assert_eq!(crabs[1].home_carpet.as_deref(), Some("dwarf_hairgrass"));
    }

    /// The cheat-only blinker (SPEC §2.5): a 15 cm goldfish-like mid-water
    /// swimmer, the only species bigger than 16 cells and the only one
    /// that glows. It eats fry and the smallest invertebrates but no adult
    /// of the small schooling fish, and leaves the most waste.
    #[test]
    fn the_cheat_only_blinker() {
        let m = materials();
        let t = SpeciesTable::builtin_for(&m).unwrap();
        let get = |name| t.get(t.id(name).unwrap()).unwrap();
        let blinker = get("blinker");
        assert!(blinker.cheat_only && blinker.breeding.is_none());
        assert_eq!(
            (blinker.zone, blinker.temperament, blinker.size()),
            (Zone::Mid, Temperament::Peaceful, (30, 22))
        );
        assert_eq!((blinker.sprite.width, blinker.sprite.height), (60, 44));
        assert!(blinker.sprite.width <= MAX_SPRITE_SIDE);
        assert!((blinker.length_cm - 15.0).abs() < f32::EPSILON);
        assert_eq!(blinker.sprite.frames.len(), 4);
        // It needs more water than it is tall.
        assert!(blinker.min_depth >= 2 * blinker.size().1);
        for food in [
            "flake_food",
            "sinking_pellet",
            "bloodworm",
            "algae_wafer",
            "algae",
            "detritus",
            "dead_plant",
        ] {
            assert!(blinker.eats(m.id(food).unwrap()), "eats {food}");
        }
        for food in ["leaf", "stem", "root", "waste"] {
            assert!(!blinker.eats(m.id(food).unwrap()), "eats {food}");
        }
        assert!(blinker.eats_corpses);
        // Prey: the micro crab, but no adult schooler or cherry shrimp.
        let prey = blinker.prey_cm.unwrap();
        assert!(get("thai_micro_crab").length_cm <= prey);
        for name in ["neon_tetra", "harlequin_rasbora", "cherry_shrimp", "guppy"] {
            assert!(get(name).length_cm > prey, "eats adult {name}");
        }
        // The heaviest bioload of all.
        assert!(
            t.iter()
                .filter(|s| s.id != blinker.id)
                .all(|s| s.bioload() < blinker.bioload())
        );
        // It alone glows: green, pulsing, eye whites bright, pupils dark.
        let glow = blinker.glow.as_ref().unwrap();
        assert_eq!(glow.colour, [0x7c, 0xff, 0x4f]);
        assert!((glow.strength - 0.35).abs() < f32::EPSILON && glow.pulse);
        assert_eq!(glow.eyes, [[0xe9, 0xf5, 0xfc], [0xa9, 0xca, 0xdd]]);
        assert_eq!(glow.unlit, [[0x14, 0x14, 0x14]]);
        assert_eq!(t.iter().filter(|s| s.glow.is_some()).count(), 1);
        // Its outline is a dark shade of its orange, not near-black.
        let outline = [0x8a, 0x42, 0x16];
        assert!(blinker.sprite.frames[0].iter().any(|t| t[..3] == outline));
    }

    #[test]
    fn glow_is_optional_and_checked() {
        let m = materials();
        let with = |glow: &str| OK.replacen("[sprite]", &format!("{glow}\n[sprite]"), 1);
        let plain = SpeciesTable::from_toml_sources(&[OK], &m).unwrap();
        assert_eq!(plain.iter().next().unwrap().glow, None);
        let lit = with(
            r##"[glow]
            colour = "#7cff4f"
            strength = 0.5
            eyes = ["b"]
            unlit = ["a"]"##,
        );
        let t = SpeciesTable::from_toml_sources(&[&lit], &m).unwrap();
        let glow = t.iter().next().unwrap().glow.clone().unwrap();
        assert_eq!(
            glow,
            Glow {
                colour: [0x7c, 0xff, 0x4f],
                strength: 0.5,
                pulse: false,
                eyes: vec![[0x40, 0x50, 0x60]],
                unlit: vec![[0x10, 0x20, 0x30]],
            }
        );
        for bad in [
            r#"colour = "green"
            strength = 0.5"#,
            r##"colour = "#7cff4f"
            strength = 0.0"##,
            r##"colour = "#7cff4f"
            strength = 1.5"##,
            r##"colour = "#7cff4f"
            strength = nan"##,
            r##"colour = "#7cff4f"
            strength = 0.5
            eyes = ["z"]"##,
            r##"colour = "#7cff4f"
            strength = 0.5
            unlit = ["ab"]"##,
            r##"colour = "#7cff4f"
            strength = 0.5
            halo = 3"##,
            "strength = 0.5",
        ] {
            let src = with(&format!("[glow]\n{bad}"));
            assert!(
                SpeciesTable::from_toml_sources(&[&src], &m).is_err(),
                "accepted {bad}"
            );
        }
    }

    /// Sprites up to [`MAX_SPRITE_SIDE`] texels a side load (the blinker is
    /// 60), and one texel pair more is refused.
    #[test]
    fn sprites_may_be_up_to_the_largest_side() {
        let m = materials();
        let sized = |side: usize| {
            let row = format!("\"{}\"", "a".repeat(side));
            let fine = format!("\"{}\"", "a".repeat(side * 2));
            let rows = |r: &str, n: usize| vec![r; n].join(", ");
            format!(
                r##"
                id = 9
                name = "big"
                zone = "mid"
                depth = [0.2, 0.6]
                min_depth = 10
                temperament = "peaceful"
                length_cm = 1.0
                lifespan_days = 100
                maturity_days = 10
                speed = 2.0
                max_speed = 4.0
                diet = ["flake_food"]
                [sprite]
                colours = {{ a = "#102030" }}
                frames = [[{}]]
                icon = ["a"]
                fine_frames = [[{}]]
                "##,
                rows(&row, 2),
                rows(&fine, 4)
            )
        };
        let ok = SpeciesTable::from_toml_sources(&[&sized(MAX_SPRITE_SIDE)], &m).unwrap();
        assert_eq!(ok.iter().next().unwrap().size(), (MAX_SPRITE_SIDE / 2, 1));
        let err = SpeciesTable::from_toml_sources(&[&sized(MAX_SPRITE_SIDE + 2)], &m).unwrap_err();
        assert!(err.to_string().contains("2..=64"), "{err}");
    }

    /// The fish at [`CELLS_PER_CM`]: a male guppy's body is the shortest
    /// (its fan tail drawn past it), then the neon and otocinclus, the
    /// harlequin and honey gourami, and the betta the longest and deepest.
    #[test]
    fn fish_are_sized_to_scale() {
        let t = SpeciesTable::builtin_for(&materials()).unwrap();
        let get = |name| t.get(t.id(name).unwrap()).unwrap();
        // Sprite width in cm, fins included.
        #[allow(clippy::cast_precision_loss)]
        let drawn = |s: &Species| s.size().0 as f32 / CELLS_PER_CM;
        let (guppy, tetra, oto, rasbora, gourami) = (
            get("guppy"),
            get("neon_tetra"),
            get("otocinclus"),
            get("harlequin_rasbora"),
            get("honey_gourami"),
        );
        assert!(guppy.length_cm < tetra.length_cm);
        assert!(tetra.length_cm <= oto.length_cm && oto.length_cm < rasbora.length_cm);
        // Real-world ranges.
        assert!((2.5..=3.5).contains(&guppy.length_cm));
        assert!((3.5..=4.0).contains(&oto.length_cm));
        assert!((4.0..=4.5).contains(&rasbora.length_cm));
        assert!((4.0..=5.0).contains(&gourami.length_cm));
        // The guppy's fan tail adds 1–1.5 cm; the others are drawn to
        // length.
        assert!((1.0..=1.5).contains(&(drawn(guppy) - guppy.length_cm)));
        for s in [oto, rasbora, gourami] {
            assert!((drawn(s) - s.length_cm).abs() < 0.01, "{}", s.name);
        }
        assert!(rasbora.size().1 > tetra.size().1, "harlequins are deeper");
        assert!(gourami.size().1 > rasbora.size().1, "gouramis are deeper");
        // The betta: a 6–6.5 cm body with
        // flowing fins drawn up to 2 cm past it and tall finnage.
        let betta = get("betta");
        assert!((6.0..=6.5).contains(&betta.length_cm));
        assert!((0.5..=2.0).contains(&(drawn(betta) - betta.length_cm)));
        for s in [guppy, tetra, oto, rasbora, gourami] {
            assert!(betta.length_cm > s.length_cm, "{}", s.name);
            assert!(betta.size().1 > s.size().1, "{}", s.name);
        }
        // It needs water twice as deep as it is tall.
        assert!(betta.min_depth >= 2 * betta.size().1);
    }

    /// Real-world sizes at [`CELLS_PER_CM`]: the Thai micro crab is the
    /// smallest invertebrate, then the cherry, then the amano, each in its
    /// range, beside a neon tetra about a female cherry's size.
    #[test]
    fn invertebrates_are_sized_to_scale() {
        let t = SpeciesTable::builtin_for(&materials()).unwrap();
        let get = |name| t.get(t.id(name).unwrap()).unwrap();
        // Sprite width in cm, antennae and legs included.
        #[allow(clippy::cast_precision_loss)]
        let drawn = |s: &Species| s.size().0 as f32 / CELLS_PER_CM;
        let (crab, cherry, amano) = (
            get("thai_micro_crab"),
            get("cherry_shrimp"),
            get("amano_shrimp"),
        );
        let tetra = get("neon_tetra");
        assert!(crab.length_cm < cherry.length_cm && cherry.length_cm < amano.length_cm);
        assert!(drawn(crab) < drawn(cherry) && drawn(cherry) < drawn(amano));
        // Body lengths (the crab's carapace).
        assert!((0.5..=1.0).contains(&crab.length_cm));
        assert!((1.5..=3.5).contains(&cherry.length_cm));
        assert!((3.0..=5.0).contains(&amano.length_cm));
        assert!((3.0..=4.0).contains(&tetra.length_cm));
        // The crab's legs span 1.5–2.5 cm, two thirds of a cherry or so.
        assert!((1.5..=2.5).contains(&drawn(crab)));
        assert!(drawn(crab) < drawn(cherry));
        // Shrimp antennae add at most 1 cm past the body; the amano's are
        // the longer.
        for s in [cherry, amano] {
            let extra = drawn(s) - s.length_cm;
            assert!(
                (0.0..=1.0).contains(&extra),
                "{} antennae {extra} cm",
                s.name
            );
        }
        assert!(drawn(amano) - amano.length_cm > drawn(cherry) - cherry.length_cm);
        // Every species is drawn at the shared scale; only the betta's
        // flowing fins reach 2 cm past its body.
        for s in t.iter() {
            let fins = if s.name == "betta" { 2.0 } else { 1.5 };
            assert!(
                s.length_cm <= drawn(s) && drawn(s) - s.length_cm <= fins,
                "{}",
                s.name
            );
        }
    }

    /// T9 part 2: who breeds and how, who eats whom, and the colour morphs
    /// and female looks.
    #[test]
    fn breeding_predators_and_morphs() {
        let t = SpeciesTable::builtin_for(&materials()).unwrap();
        let get = |name| t.get(t.id(name).unwrap()).unwrap();
        for name in ["guppy", "cherry_shrimp"] {
            let b = get(name).breeding.unwrap();
            assert_eq!(b.mode, BreedingMode::Livebearer, "{name}");
        }
        for name in [
            "neon_tetra",
            "corydoras",
            "betta",
            "honey_gourami",
            "harlequin_rasbora",
            "otocinclus",
        ] {
            assert_eq!(
                get(name).breeding.unwrap().mode,
                BreedingMode::Eggs,
                "{name}"
            );
        }
        for name in ["nerite", "amano_shrimp", "thai_micro_crab"] {
            assert!(get(name).breeding.is_none(), "{name}");
        }
        // Predators: the betta takes a cherry shrimp but not an amano;
        // guppies and gouramis only fry, shrimplets and crabs.
        let prey = |name| get(name).prey_cm.unwrap_or(0.0);
        assert!(prey("betta") >= get("cherry_shrimp").length_cm);
        assert!(prey("betta") < get("amano_shrimp").length_cm);
        for name in ["guppy", "honey_gourami"] {
            assert!(prey(name) < get("cherry_shrimp").length_cm, "{name}");
            assert!(prey(name) >= get("thai_micro_crab").length_cm, "{name}");
        }
        assert_eq!(prey("neon_tetra"), 0.0);
        // Cherry shrimp: the palette's red, reverting to a brown wildtype,
        // and blue and yellow; drawn alike by sex.
        let cherry = get("cherry_shrimp");
        assert_eq!(cherry.morphs, ["red", "wild", "blue", "yellow"]);
        assert_eq!((cherry.palette_morph, cherry.wildtype), (0, 1));
        assert!(!cherry.dimorphic());
        assert_eq!(cherry.looks.len(), 4);
        assert_eq!(cherry.looks[0].frames, cherry.sprite.frames);
        assert_eq!(cherry.looks[0].fine_frames, cherry.sprite.fine_frames);
        let brown = &cherry.looks[usize::from(cherry.wildtype)];
        assert_ne!(brown.frames, cherry.sprite.frames);
        assert_ne!(brown.fine_frames, cherry.sprite.fine_frames);
        assert_eq!(cherry.look(2, true), 2);
        // Guppies: females have their own frames, in every strain.
        let guppy = get("guppy");
        assert_eq!(guppy.morphs, ["wild", "red", "blue"]);
        assert!(guppy.dimorphic());
        assert_eq!(guppy.looks.len(), 6);
        assert_eq!((guppy.look(1, false), guppy.look(1, true)), (1, 4));
        let (male, female) = (&guppy.looks[1], &guppy.looks[4]);
        assert_eq!((male.morph, male.female), (1, false));
        assert_eq!((female.morph, female.female), (1, true));
        assert_ne!(male.frames, female.frames);
        assert_eq!(female.frames.len(), guppy.sprite.frames.len());
        // Species without genetics have the one wild look.
        let tetra = get("neon_tetra");
        assert_eq!((tetra.morphs.len(), tetra.looks.len()), (1, 1));
        assert_eq!(tetra.looks[0].frames, tetra.sprite.frames);
        assert_eq!(tetra.look(5, true), 0);
    }

    #[test]
    fn only_an_air_breather_has_an_air_tolerance() {
        let m = materials();
        let with = |extra: &str| {
            SpeciesTable::from_toml_sources(
                &[&OK.replace("diet = [", &format!("{extra}\ndiet = ["))],
                &m,
            )
        };
        let frog = with("breathes_air = true\nair_tolerance_seconds = 180").unwrap();
        assert_eq!(frog.get(SpeciesId(9)).unwrap().air_tolerance_seconds, 180);
        assert_eq!(
            with("")
                .unwrap()
                .get(SpeciesId(9))
                .unwrap()
                .air_tolerance_seconds,
            0
        );
        for bad in [
            "breathes_air = true",
            "breathes_air = true\nair_tolerance_seconds = 10",
            "breathes_air = true\nair_tolerance_seconds = 9000",
            "air_tolerance_seconds = 180",
        ] {
            assert!(
                matches!(with(bad), Err(SpeciesError::Invalid { .. })),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_territory_is_only_for_a_territorial_species() {
        let m = materials();
        // 2–50 cm across.
        let territory = |temperament: &str, cm: &str| {
            SpeciesTable::from_toml_sources(
                &[&OK.replace(
                    "temperament = \"peaceful\"",
                    &format!("temperament = \"{temperament}\"\nterritory_cm = {cm}"),
                )],
                &m,
            )
        };
        let held = territory("territorial", "15.0").unwrap();
        assert_eq!(held.get(SpeciesId(9)).unwrap().territory_cm, Some(15.0));
        for (temperament, cm) in [
            ("peaceful", "15.0"),
            ("territorial", "1.0"),
            ("territorial", "51.0"),
            ("territorial", "nan"),
        ] {
            assert!(
                matches!(
                    territory(temperament, cm),
                    Err(SpeciesError::Invalid { .. })
                ),
                "{temperament} {cm}"
            );
        }
    }

    #[test]
    fn rejects_bad_breeding_and_genetics() {
        let m = materials();
        let with = |extra: &str| {
            let src = OK.replace("[sprite]", &format!("{extra}\n[sprite]"));
            SpeciesTable::from_toml_sources(&[&src], &m)
        };
        let breeding = |days: u32, brood: &str| {
            format!("[breeding]\nmode = \"eggs\"\ndays = {days}\nbrood = {brood}\nrest_days = 5\n")
        };
        assert!(with(&breeding(2, "[2, 4]")).is_ok());
        for bad in [
            breeding(0, "[2, 4]"),
            breeding(2, "[0, 4]"),
            breeding(2, "[5, 4]"),
            breeding(2, "[2, 17]"),
        ] {
            assert!(
                matches!(with(&bad), Err(SpeciesError::Invalid { .. })),
                "{bad}"
            );
        }
        let genetics = |wild: &str, sprite: &str, colours: &str| {
            format!(
                "[genetics]\nwildtype = \"{wild}\"\nsprite = \"{sprite}\"\n\
                 [[genetics.morph]]\nname = \"plain\"\n\
                 [[genetics.morph]]\nname = \"dark\"\ncolours = {colours}\n"
            )
        };
        let ok = with(&genetics("dark", "plain", "{ a = \"#000000\" }")).unwrap();
        let s = ok.get(SpeciesId(9)).unwrap();
        assert_eq!((s.wildtype, s.palette_morph, s.looks.len()), (1, 0, 2));
        assert_eq!(s.looks[1].frames[0][0], [0, 0, 0, 255]);
        for bad in [
            genetics("feral", "plain", "{ a = \"#000000\" }"),
            genetics("dark", "fancy", "{ a = \"#000000\" }"),
            // The sprite's morph is the sprite's colours.
            genetics("plain", "dark", "{ a = \"#000000\" }"),
            // Only the sprite's colour keys can change.
            genetics("dark", "plain", "{ z = \"#000000\" }"),
        ] {
            assert!(
                matches!(with(&bad), Err(SpeciesError::Invalid { .. })),
                "{bad}"
            );
        }
        // Prey shorter than the predator.
        assert!(matches!(
            SpeciesTable::from_toml_sources(
                &[&OK.replace("length_cm = 1.0", "length_cm = 1.0\nprey_cm = 1.0")],
                &m
            ),
            Err(SpeciesError::Invalid { .. })
        ));
        // Female frames match the frames' count and size, and come with
        // fine frames at twice the size.
        let one = "[\"aaaaaa\", \"aaaaaa\", \"aaaaaa\", \"aaaaaa\"]";
        let row = "\"aaaaaaaaaaaa\"";
        let fine_one = format!("[{}]", [row; 8].join(", "));
        let fine_two = format!("[{fine_one}, {fine_one}]");
        let female_with = |frames: &str, fine: Option<&str>| {
            let fine = fine.map_or(String::new(), |f| format!("female_fine_frames = {f}\n"));
            let src = OK.replace(
                "icon = ",
                &format!("female_frames = {frames}\n{fine}icon = "),
            );
            SpeciesTable::from_toml_sources(&[&src], &m)
        };
        let female = |frames: &str| female_with(frames, Some(&fine_two));
        let t = female(&format!("[{one}, {one}]")).unwrap();
        let s = t.get(SpeciesId(9)).unwrap();
        assert!(s.dimorphic());
        assert_eq!(s.looks[1].frames[0][12], [0x10, 0x20, 0x30, 255]);
        assert_eq!(s.looks[1].fine_frames[0][12], [0x10, 0x20, 0x30, 255]);
        assert_eq!(s.looks[1].fine_frames[0].len(), 12 * 8);
        assert!(matches!(
            female(&format!("[{one}]")),
            Err(SpeciesError::Invalid { .. })
        ));
        assert!(matches!(
            female("[[\"aaaaaa\"], [\"aaaaaa\"]]"),
            Err(SpeciesError::Invalid { .. })
        ));
        // Without fine frames, with too few, or at the base size.
        for fine in [
            None,
            Some(format!("[{fine_one}]")),
            Some(format!("[{one}, {one}]")),
        ] {
            assert!(
                matches!(
                    female_with(&format!("[{one}, {one}]"), fine.as_deref()),
                    Err(SpeciesError::Invalid { .. })
                ),
                "{fine:?}"
            );
        }
    }

    #[test]
    fn parses_sprites_and_colours() {
        let t = SpeciesTable::from_toml_sources(&[OK], &materials()).unwrap();
        let s = t.get(SpeciesId(9)).unwrap();
        assert_eq!((s.sprite.width, s.sprite.height), (6, 4));
        assert_eq!(s.size(), (3, 2));
        assert_eq!(s.sprite.frames[0][0], [0x10, 0x20, 0x30, 255]);
        assert_eq!(s.sprite.frames[0][12], [0; 4]);
        assert_eq!(s.colour, [0x10, 0x20, 0x30]);
        assert_eq!((s.sprite.icon.width, s.sprite.icon.height), (3, 2));
        assert_eq!(s.sprite.icon.pixels[2], [0x40, 0x50, 0x60, 255]);
        assert_eq!(s.sprite.icon.pixels[4], [0; 4]);
    }

    /// A morph or female colour set that recolours the sprite must also
    /// recolour the keys only its fine art uses, or a blue shrimp drawn
    /// finely would keep the red sprite's shading. Listed keys are colours
    /// every strain shares (the guppy's pale belly and dorsal sheen).
    #[test]
    fn recolours_cover_the_keys_only_fine_art_uses() {
        let keys = |frames: &[Vec<String>]| -> BTreeSet<char> {
            frames
                .iter()
                .flatten()
                .flat_map(|r| r.chars())
                .filter(|&c| c != '.')
                .collect()
        };
        for src in BUILTIN {
            let d: SpeciesDef = toml::from_str(src).unwrap();
            let mut fine_only = &keys(&d.sprite.fine_frames) - &keys(&d.sprite.frames);
            if let (Some(f), Some(fine)) = (&d.sprite.female_frames, &d.sprite.female_fine_frames) {
                fine_only.extend(&keys(fine) - &keys(f));
            }
            let sets = d
                .genetics
                .iter()
                .flat_map(|g| g.morph.iter().map(|m| &m.colours))
                .chain(d.sprite.female.as_ref());
            for colours in sets.filter(|c| !c.is_empty()) {
                let over: BTreeSet<char> =
                    colours.keys().filter_map(|k| k.chars().next()).collect();
                let shared: &[char] = if d.name == "guppy" { &['a', 'm'] } else { &[] };
                let missing: Vec<_> = fine_only
                    .difference(&over)
                    .filter(|c| !shared.contains(c))
                    .collect();
                assert!(missing.is_empty(), "{}: {missing:?} not recoloured", d.name);
            }
        }
    }

    #[test]
    fn fine_art_is_required_and_must_match_the_base_poses_at_double_size() {
        let materials = materials();
        let table = SpeciesTable::builtin_for(&materials).unwrap();
        for s in table.iter() {
            let art = &s.sprite;
            assert_eq!(art.fine_frames.len(), art.frames.len(), "{}", s.name);
            let side = art.width * art.height * 4;
            assert!(
                art.fine_frames.iter().all(|f| f.len() == side),
                "{}",
                s.name
            );
        }
        let parse = |src: &str| SpeciesTable::from_toml_sources(&[src], &materials);
        let (head, _) = OK.split_once("fine_frames").unwrap();
        assert!(matches!(
            parse(head),
            Err(SpeciesError::Invalid { reason, .. }) if reason.contains("fine_frames")
        ));
        // Two base poses in the 6×4 test frame need two 12×8 fine poses.
        let row = "aaaaaaaaaaaa";
        let pose =
            |rows: usize, row: &str| format!("[{}]", vec![format!("{row:?}"); rows].join(", "));
        let good = pose(8, row);
        assert!(parse(&format!("{head}fine_frames = [{good}, {good}]")).is_ok());
        let one = format!("[{good}]");
        let short = format!("[{0}, {0}]", pose(7, row));
        let narrow = format!("[{0}, {0}]", pose(8, &row[1..]));
        for invalid in ["[]", one.as_str(), short.as_str(), narrow.as_str()] {
            assert!(
                matches!(
                    parse(&format!("{head}fine_frames = {invalid}")),
                    Err(SpeciesError::Invalid { .. })
                ),
                "{invalid}"
            );
        }
    }

    #[test]
    fn depth_bands_default_to_every_band_and_are_checked() {
        let m = materials();
        let t = SpeciesTable::from_toml_sources(&[OK], &m).unwrap();
        assert_eq!(t.get(SpeciesId(9)).unwrap().bands, [0, DEPTH_BANDS - 1]);
        let with = |bands: &str| {
            let src = OK.replace(
                "min_depth = 10",
                &format!("min_depth = 10\nbands = {bands}"),
            );
            SpeciesTable::from_toml_sources(&[&src], &m)
        };
        assert_eq!(
            with("[2, 5]").unwrap().get(SpeciesId(9)).unwrap().bands,
            [2, 5]
        );
        assert_eq!(
            with("[3, 3]").unwrap().get(SpeciesId(9)).unwrap().bands,
            [3, 3]
        );
        for bad in ["[5, 2]", "[0, 8]", "[9, 9]"] {
            assert!(
                matches!(with(bad), Err(SpeciesError::Invalid { .. })),
                "bands = {bad}"
            );
        }
        // Every built-in species uses every band.
        for s in SpeciesTable::builtin_for(&m).unwrap().iter() {
            assert_eq!(s.bands, [0, DEPTH_BANDS - 1], "{}", s.name);
        }
    }

    #[test]
    fn rejects_bad_swims_and_burrows() {
        let m = materials();
        let bad = |from: &str, to: &str| {
            let src = OK.replace(from, to);
            assert_ne!(src, OK, "{from:?} not in the template");
            SpeciesTable::from_toml_sources(&[&src], &m)
        };
        let walker = |extra: &str| {
            let src = OK
                .replace("zone = \"mid\"", "zone = \"bottom\"")
                .replace("min_depth = 10", &format!("min_depth = 10\n{extra}"));
            SpeciesTable::from_toml_sources(&[&src], &m)
        };
        // Only bottom dwellers swim now and then, every 10..=600 s, and only
        // walkers burrow, every 32..=600 s.
        assert!(matches!(
            bad("min_depth = 10", "min_depth = 10\nswim_every = 20.0"),
            Err(SpeciesError::Invalid { .. })
        ));
        assert!(walker("swim_every = 20.0\nburrow_every = 40.0").is_ok());
        let crawler = OK
            .replace("zone = \"mid\"", "zone = \"surfaces\"")
            .replace("min_depth = 10", "min_depth = 10\nswim_every = 20.0");
        assert!(SpeciesTable::from_toml_sources(&[&crawler], &m).is_ok());
        assert!(matches!(
            SpeciesTable::from_toml_sources(&[&crawler.replace("swim_every", "burrow_every")], &m),
            Err(SpeciesError::Invalid { .. })
        ));
        for extra in [
            "swim_every = 9.0",
            "swim_every = 601.0",
            "swim_every = nan",
            "burrow_every = 31.0",
            "burrow_every = inf",
        ] {
            assert!(
                matches!(walker(extra), Err(SpeciesError::Invalid { .. })),
                "{extra}"
            );
        }
    }

    #[test]
    fn rejects_bad_lengths_reaches_and_hops() {
        let m = materials();
        let bad = |from: &str, to: &str| {
            let src = OK.replace(from, to);
            assert_ne!(src, OK, "{from:?} not in the template");
            SpeciesTable::from_toml_sources(&[&src], &m)
        };
        assert!(matches!(
            bad("min_depth = 10", "min_depth = 10\nreach = 0"),
            Err(SpeciesError::Invalid { .. })
        ));
        assert!(matches!(
            bad("min_depth = 10", "min_depth = 10\nreach = 5"),
            Err(SpeciesError::Invalid { .. })
        ));
        // A length the sprite can't hold at CELLS_PER_CM (3 cells: 1.5 cm).
        for length in ["length_cm = 2.0", "length_cm = 0.0", "length_cm = nan"] {
            assert!(
                matches!(
                    bad("length_cm = 1.0", length),
                    Err(SpeciesError::Invalid { .. })
                ),
                "{length}"
            );
        }
        // Only bottom walkers hop, every 1..=600 s.
        assert!(matches!(
            bad("min_depth = 10", "min_depth = 10\nhop_every = 5.0"),
            Err(SpeciesError::Invalid { .. })
        ));
        let walker = |extra: &str| {
            let src = OK
                .replace("zone = \"mid\"", "zone = \"bottom\"")
                .replace("min_depth = 10", &format!("min_depth = 10\n{extra}"));
            SpeciesTable::from_toml_sources(&[&src], &m)
        };
        for every in ["0.5", "601.0", "nan", "inf"] {
            assert!(
                matches!(
                    walker(&format!("hop_every = {every}")),
                    Err(SpeciesError::Invalid { .. })
                ),
                "hop_every = {every}"
            );
        }
        // Maturity comes before the end of a lifespan of at most 20 years.
        for (from, to) in [
            ("lifespan_days = 100", "lifespan_days = 0"),
            ("lifespan_days = 100", "lifespan_days = 7301"),
            ("maturity_days = 10", "maturity_days = 0"),
            ("maturity_days = 10", "maturity_days = 100"),
        ] {
            assert!(
                matches!(bad(from, to), Err(SpeciesError::Invalid { .. })),
                "{to}"
            );
        }
        // Waste every 1..=600 s; only walkers keep a home.
        for every in ["0.5", "601.0", "nan"] {
            assert!(
                matches!(
                    bad(
                        "min_depth = 10",
                        &format!("min_depth = 10\nwaste_every = {every}")
                    ),
                    Err(SpeciesError::Invalid { .. })
                ),
                "waste_every = {every}"
            );
        }
        assert!(matches!(
            bad(
                "min_depth = 10",
                "min_depth = 10\nhome_carpet = \"monte_carlo\""
            ),
            Err(SpeciesError::Invalid { .. })
        ));
        assert!(walker("home_carpet = \"monte_carlo\"\nwaste_every = 2.0").is_ok());
        let ok = walker("hop_every = 6.0\nreach = 4").unwrap();
        let s = ok.get(SpeciesId(9)).unwrap();
        assert_eq!((s.hop_every, s.reach), (Some(6.0), 4));
    }

    #[test]
    fn rejects_bad_definitions() {
        let m = materials();
        let bad = |from: &str, to: &str| {
            let src = OK.replace(from, to);
            assert_ne!(src, OK, "{from:?} not in the template");
            SpeciesTable::from_toml_sources(&[&src], &m)
        };
        assert!(matches!(
            bad("[0.2, 0.6]", "[0.6, 0.2]"),
            Err(SpeciesError::Invalid { .. })
        ));
        assert!(matches!(
            bad("max_speed = 4.0", "max_speed = 40.0"),
            Err(SpeciesError::Invalid { .. })
        ));
        assert!(matches!(
            bad("[\"flake_food\"]", "[\"cake\"]"),
            Err(SpeciesError::UnknownFood { .. })
        ));
        // Ragged rows, an unkeyed character, odd sides.
        assert!(matches!(
            bad("\"aa..aa\", \"aa..aa\"]", "\"aa..aa\", \"aa..a\"]"),
            Err(SpeciesError::Invalid { .. })
        ));
        assert!(matches!(
            bad("\"aa..aa\", \"aa..aa\"]", "\"aa..aa\", \"aa..az\"]"),
            Err(SpeciesError::Invalid { .. })
        ));
        // Icons: at most 8 a side, every row the same width, keyed colours.
        for icon in [
            "[\"aaaaaaaaa\"]",
            "[\"a\", \"a\", \"a\", \"a\", \"a\", \"a\", \"a\", \"a\", \"a\"]",
            "[]",
            "[\"aa\", \"a\"]",
            "[\"az\"]",
        ] {
            assert!(
                matches!(
                    bad("icon = [\"aab\", \"a.a\"]", &format!("icon = {icon}")),
                    Err(SpeciesError::Invalid { .. })
                ),
                "{icon}"
            );
        }
        let odd_width = OK
            .replace("aabbaa", "aabbaaa")
            .replace(".aa...", ".aa....")
            .replace("aa..aa", "aa..aaa");
        assert!(matches!(
            SpeciesTable::from_toml_sources(&[&odd_width], &m),
            Err(SpeciesError::Invalid { reason, .. }) if reason.contains("even")
        ));
        let odd_height = OK
            .replace("\".aa...\"]", "\".aa...\", \"aaaaaa\"]")
            .replace("\"aa..aa\"]", "\"aa..aa\", \"aaaaaa\"]");
        assert!(matches!(
            SpeciesTable::from_toml_sources(&[&odd_height], &m),
            Err(SpeciesError::Invalid { reason, .. }) if reason.contains("even")
        ));
        assert!(matches!(
            bad("zone = \"mid\"", "zone = \"sky\""),
            Err(SpeciesError::Parse { .. })
        ));
        assert!(matches!(
            SpeciesTable::from_toml_sources(&[OK, OK], &m),
            Err(SpeciesError::DuplicateId(9))
        ));
    }
}
