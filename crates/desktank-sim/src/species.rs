//! Creature species, loaded from TOML (SPEC §2.4, D8): size, depth zone,
//! surface needs, speed, diet and placeholder pixel art (D11).
//!
//! Each species is one file in `assets/species/`. Its sprite is pixel art
//! written as rows of characters, one per texel, with a colour key; the
//! renderer packs every species' frames into one atlas texture. Sprites
//! are drawn at [`TEXELS_PER_CELL`] texels per cell side, so their sides
//! must be even, and the sprite sets the creature's body: the cells it
//! covers and collides with (half its texel size).
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
/// Largest palette icon side, in pixels (a palette slot's face).
pub const MAX_ICON_SIDE: usize = 8;
/// Fastest allowed speed, in cells per second: under one cell per tick, so
/// a body never skips past a cell it would collide with.
pub const MAX_SPEED: f32 = 16.0;
/// Farthest a species reaches to eat, in cells past its body.
pub const MAX_REACH: u8 = 4;

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
/// sides are even ([`TEXELS_PER_CELL`] texels per cell).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sprite {
    /// Width in texels.
    pub width: usize,
    /// Height in texels.
    pub height: usize,
    /// Animation frames.
    pub frames: Vec<Vec<[u8; 4]>>,
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
    /// How far past its body it takes food, in cells (1..=[`MAX_REACH`]):
    /// 1 for a mouth at the body's edge, more for a filter feeder's fans.
    pub reach: u8,
    /// Seconds between a bottom walker's hops off the substrate (shrimp),
    /// or `None` if it never hops.
    pub hop_every: Option<f32>,
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
    #[serde(default = "default_reach")]
    reach: u8,
    #[serde(default)]
    hop_every: Option<f32>,
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
        reach: d.reach,
        hop_every: d.hop_every,
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
    let mut frames = Vec::with_capacity(def.frames.len());
    for rows in &def.frames {
        if rows.len() != height {
            return Err("every sprite frame must be the same size");
        }
        frames.push(texels(rows, width, &key)?);
    }
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
    "##;

    #[test]
    #[allow(clippy::many_single_char_names)]
    fn builtin_species_load() {
        let m = materials();
        let t = SpeciesTable::builtin_for(&m).unwrap();
        assert_eq!(t.len(), 7);
        for name in [
            "neon_tetra",
            "corydoras",
            "betta",
            "nerite",
            "cherry_shrimp",
            "amano_shrimp",
            "thai_micro_crab",
        ] {
            let s = t.get(t.id(name).unwrap()).unwrap();
            assert!(!s.diet.is_empty(), "{name} eats something");
            assert!(s.sprite.frames.len() >= 2, "{name} animates");
            let icon = &s.sprite.icon;
            assert!(icon.pixels.iter().any(|p| p[3] > 0), "{name} has an icon");
            let (w, h) = s.size();
            assert!(w <= 8 && h <= 8, "{name} fits a palette slot");
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
        assert!(
            crab.size().0 % 2 == 1 && crab.size().1 % 2 == 1,
            "odd crawler sides"
        );
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
            bad("min_depth = 10", "min_depth = 10\nreach = 0"),
            Err(SpeciesError::Invalid { .. })
        ));
        assert!(matches!(
            bad("min_depth = 10", "min_depth = 10\nreach = 5"),
            Err(SpeciesError::Invalid { .. })
        ));
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
        let ok = walker("hop_every = 6.0\nreach = 4").unwrap();
        let s = ok.get(SpeciesId(9)).unwrap();
        assert_eq!((s.hop_every, s.reach), (Some(6.0), 4));
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
