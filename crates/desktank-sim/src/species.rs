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
//! divisible by four.
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
/// Largest sprite side, in texels (16 cells).
pub const MAX_SPRITE_SIDE: usize = 32;
/// Cells per centimetre of real-world size: every species is drawn at this
/// scale, so a 3.5 cm neon tetra is 7 cells long and sits beside a 2.5 cm
/// cherry shrimp (5 cells) as it would in a tank.
pub const CELLS_PER_CM: f32 = 2.0;
/// Largest palette icon side, in pixels (a palette slot's face).
pub const MAX_ICON_SIDE: usize = 8;
/// Fastest allowed speed, in cells per second: under one cell per tick, so
/// a body never skips past a cell it would collide with.
pub const MAX_SPEED: f32 = 16.0;
/// Farthest a species reaches to eat, in cells past its body.
pub const MAX_REACH: u8 = 4;
/// Longest lifespan a species may give, in real days (20 years).
pub const MAX_LIFESPAN_DAYS: u32 = 7300;
/// Most colour morphs a species may have (SPEC §2.4, D13).
pub const MAX_MORPHS: usize = 8;
/// Most young in one brood or clutch.
pub const MAX_BROOD: u8 = 16;

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
    /// Real-world lifespan in days (1..=[`MAX_LIFESPAN_DAYS`]), on the life
    /// clock (SPEC §2.4, T9: a day per `LIFE_DAY_SECONDS` of world time).
    /// Each creature's own lifespan is within ±10% of it.
    pub lifespan_days: u32,
    /// Real-world age at maturity in days (below the lifespan): a well-fed
    /// creature is an adult from this age.
    pub maturity_days: u32,
    /// Longest prey it eats, in cm (`None`: it eats no creatures). A
    /// hungry adult takes any smaller creature it touches: fry, shrimp
    /// (SPEC §2.4, T9 part 2).
    pub prey_cm: Option<f32>,
    /// How it breeds, or `None` if it doesn't in a freshwater tank.
    pub breeding: Option<Breeding>,
    /// Its colour morphs by name, in allele order (append only: saves
    /// store the index). A species with no `[genetics]` has one, `"wild"`.
    pub morphs: Vec<String>,
    /// Index of the wildtype, which mixed lines revert to.
    pub wildtype: u8,
    /// Index of the morph the palette adds: the sprite's own colours.
    pub palette_morph: u8,
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
    temperament: Temperament,
    speed: f32,
    max_speed: f32,
    diet: Vec<String>,
    length_cm: f32,
    #[serde(default = "default_reach")]
    reach: u8,
    #[serde(default)]
    hop_every: Option<f32>,
    lifespan_days: u32,
    maturity_days: u32,
    #[serde(default)]
    prey_cm: Option<f32>,
    #[serde(default)]
    breeding: Option<BreedingDef>,
    #[serde(default)]
    genetics: Option<GeneticsDef>,
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
struct GeneticsDef {
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
    if !(1..=MAX_LIFESPAN_DAYS).contains(&d.lifespan_days)
        || d.maturity_days == 0
        || d.maturity_days >= d.lifespan_days
    {
        return Err(invalid(
            "lifespan_days must be 1..=7300 and maturity_days 1..lifespan_days",
        ));
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
    let sprite = sprite(&d.sprite).map_err(invalid)?;
    // Sprite sides are at most 32 texels: exact in f32.
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
    let breeding = d
        .breeding
        .as_ref()
        .map(breeding)
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
        temperament: d.temperament,
        speed: d.speed,
        max_speed: d.max_speed,
        diet,
        length_cm: d.length_cm,
        reach: d.reach,
        hop_every: d.hop_every,
        lifespan_days: d.lifespan_days,
        maturity_days: d.maturity_days,
        prey_cm: d.prey_cm,
        breeding,
        morphs,
        wildtype,
        palette_morph,
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
        return Err("sprite sides must be 2..=32 texels");
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
        assert_eq!(t.len(), 11);
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

    /// The fish at [`CELLS_PER_CM`]: a male guppy's body is the shortest
    /// (its fan tail drawn past it), then the neon and otocinclus, with the
    /// harlequin and honey gourami the longest and deepest.
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
        // Every species is drawn at the shared scale.
        for s in t.iter() {
            assert!(
                s.length_cm <= drawn(s) && drawn(s) - s.length_cm <= 1.5,
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
