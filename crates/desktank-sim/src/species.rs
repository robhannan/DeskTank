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
    sprite: SpriteDef,
}

fn default_reach() -> u8 {
    1
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpriteDef {
    colours: BTreeMap<String, String>,
    frames: Vec<Vec<String>>,
    #[serde(default)]
    fine_frames: Vec<Vec<String>>,
    icon: Vec<String>,
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
    let colour = main_colour(&sprite);
    Ok(Species {
        id: SpeciesId(d.id),
        name: d.name.clone(),
        zone: d.zone,
        depth: d.depth,
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
        sprite,
        colour,
    })
}

/// Turns character rows into RGBA frames.
fn sprite(def: &SpriteDef) -> Result<Sprite, &'static str> {
    let mut key = BTreeMap::new();
    for (k, v) in &def.colours {
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
