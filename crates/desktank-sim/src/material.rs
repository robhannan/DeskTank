//! Materials: movement classes and properties, loaded from TOML (SPEC §2.2).
//!
//! Units (SPEC §2.1): density is relative to water (1.0), angles are in
//! degrees and times are in seconds of world time.

use serde::Deserialize;
use thiserror::Error;

use crate::palette::{Holds, PalettePath, PalettePathError};

/// Index of a material in the [`MaterialTable`]; stored in every cell.
///
/// IDs are fixed in each material's TOML file so saves stay stable when
/// materials are added.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MaterialId(pub u8);

/// How a material moves (SPEC §2.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MovementClass {
    /// Never moves, and is never displaced.
    Solid,
    /// Part of a rigid body (D17): the cells of one pour move together as
    /// one object, sinking through lighter fluids (or rising through
    /// heavier ones) without rotating, and resting on powders, other
    /// bodies and the floor. Never displaced; glued bodies never move.
    Rigid,
    /// Falls and piles up at its angle of repose.
    Powder,
    /// Falls and spreads sideways up to its dispersion.
    Liquid,
    /// Rises ("reverse falling sand") through liquids, spreading sideways
    /// under ceilings. Never displaced itself, so each exchange with a
    /// liquid is made by the bubble's own move, once per tick.
    Buoyant,
    /// Rests on the surface for its float time, then sinks like a powder.
    Floating,
}

impl MovementClass {
    /// Whether moving cells can swap into cells of this class. Only liquids
    /// (water, and air above the surface) are displaced; solids, rigid
    /// bodies, powders, floaters and buoyant cells are not.
    #[must_use]
    pub fn is_displaceable(self) -> bool {
        self == Self::Liquid
    }

    /// Whether a pour brush may overwrite cells of this class: liquids and
    /// buoyant cells such as bubbles.
    #[must_use]
    pub fn is_fluid(self) -> bool {
        matches!(self, Self::Liquid | Self::Buoyant)
    }
}

/// Rigid cells: the low bit of the variant byte mirrors the body's glued
/// flag, so the renderer can shade glued bodies (D17) without a third
/// texture channel. The colour shade comes from the other bits, so every
/// shade range of `desktank_render`'s palette starts on an even byte.
pub const GLUED_VARIANT_BIT: u8 = 1;

/// Maximum per-cell waterlogging level for driftwood, stored in the upper
/// seven bits of its cell `variant` byte (`variant >> 1`, 0 = dry, 127 =
/// fully waterlogged). Bit 0 stays [`GLUED_VARIANT_BIT`].
pub const MAX_DRIFTWOOD_WETNESS: u8 = 127;

/// Effective density of fully waterlogged driftwood relative to water (1.0):
/// dense enough to sink gently and settle on substrate without burying itself
/// like rock (2.6).
pub const DRIFTWOOD_WET_DENSITY: f32 = 1.2;

/// Extracts a driftwood cell's waterlogging level (`0..=MAX_DRIFTWOOD_WETNESS`)
/// from its `variant` byte.
#[must_use]
pub const fn driftwood_wetness(variant: u8) -> u8 {
    variant >> 1
}

/// Packs a driftwood cell's waterlogging level (`0..=MAX_DRIFTWOOD_WETNESS`,
/// clamped) and glued flag into its `variant` byte.
#[must_use]
pub const fn driftwood_variant(wetness: u8, glued: bool) -> u8 {
    let w = if wetness > MAX_DRIFTWOOD_WETNESS {
        MAX_DRIFTWOOD_WETNESS
    } else {
        wetness
    };
    (w << 1) | if glued { GLUED_VARIANT_BIT } else { 0 }
}

/// Largest allowed `dispersion`, in cells per tick (SPEC §2.1).
pub const MAX_DISPERSION: u8 = 8;
/// Default angle of repose, in degrees, for powders that don't set one.
pub const DEFAULT_REPOSE: f32 = 35.0;
/// Allowed `repose` range in degrees. 45° is the steepest pile a one-cell
/// diagonal slide makes; the shallow end bounds the slide reach to
/// [`MAX_RUN`] cells.
pub const REPOSE_RANGE: core::ops::RangeInclusive<f32> = 15.0..=45.0;
/// Longest sideways slide (in cells per row of drop) a powder looks along.
pub const MAX_RUN: usize = 4;
/// Longest allowed `float_time`, in seconds (fits the u16 cell state).
pub const MAX_FLOAT_TIME: f32 = 3000.0;

/// One material, validated.
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    /// Stable ID stored in cells and saves.
    pub id: MaterialId,
    /// Unique name, e.g. `"sand"`.
    pub name: String,
    /// Movement class.
    pub class: MovementClass,
    /// Density relative to water (1.0). Decides who displaces whom.
    pub density: f32,
    /// Liquids and buoyant materials: how far they spread sideways per tick.
    pub dispersion: u8,
    /// Powders and sinking floaters: angle of repose in degrees.
    pub repose: f32,
    /// Floating: seconds spent resting at the surface before sinking.
    pub float_time: f32,
    /// Floating: density while still floating.
    pub float_density: f32,
    /// Buoyant: the material it turns into on touching that material (or
    /// the top edge), e.g. a bubble popping into air.
    pub pops_into: Option<MaterialId>,
    /// How much of the light reaching a cell it blocks, 0–1: 0 for fluids
    /// and 1 for other classes unless set (plant tissue lets some through).
    pub opacity: f32,
    /// Powders and floaters: small loose particles (food, waste, detritus)
    /// that creatures swim through instead of colliding with (T7).
    pub passable: bool,
    /// Placeholder palette colour (sRGB).
    pub colour: [u8; 3],
    /// The palette sub-category it is poured from, or `None` if it isn't
    /// offered for pouring (air, the tissue plants grow, device cells).
    pub palette: Option<PalettePath>,
}

/// Error loading or validating materials.
#[derive(Debug, Error)]
pub enum MaterialError {
    /// A TOML source didn't parse or didn't match the schema.
    #[error("material source {index}: {source}")]
    Parse {
        /// Index of the source in the list passed in.
        index: usize,
        /// The parse error.
        #[source]
        source: toml::de::Error,
    },
    /// Two materials share an ID.
    #[error("duplicate material id {0}")]
    DuplicateId(u8),
    /// Two materials share a name.
    #[error("duplicate material name {0:?}")]
    DuplicateName(String),
    /// A property is out of range or doesn't fit the movement class.
    #[error("material {name:?}: {reason}")]
    Invalid {
        /// Material name.
        name: String,
        /// What is wrong.
        reason: &'static str,
    },
    /// `palette` names no sub-category that holds materials.
    #[error("material {name:?}: {source}")]
    Palette {
        /// Material name.
        name: String,
        /// What is wrong with the path.
        #[source]
        source: PalettePathError,
    },
    /// `pops_into` names a material that doesn't exist.
    #[error("material {name:?}: unknown pops_into material {target:?}")]
    UnknownTarget {
        /// Material name.
        name: String,
        /// The missing target.
        target: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MaterialDef {
    id: u8,
    name: String,
    class: MovementClass,
    density: f32,
    #[serde(default)]
    dispersion: u8,
    repose: Option<f32>,
    float_time: Option<f32>,
    float_density: Option<f32>,
    pops_into: Option<String>,
    opacity: Option<f32>,
    #[serde(default)]
    passable: bool,
    colour: String,
    palette: Option<String>,
}

/// The palette embedded in the binary (SPEC T1, T5, T6). Our own values.
const BUILTIN: &[&str] = &[
    include_str!("../../../assets/materials/water.toml"),
    include_str!("../../../assets/materials/air.toml"),
    include_str!("../../../assets/materials/sand.toml"),
    include_str!("../../../assets/materials/gravel.toml"),
    include_str!("../../../assets/materials/rock.toml"),
    include_str!("../../../assets/materials/air_bubble.toml"),
    include_str!("../../../assets/materials/flake_food.toml"),
    include_str!("../../../assets/materials/aquasoil.toml"),
    include_str!("../../../assets/materials/driftwood.toml"),
    include_str!("../../../assets/materials/co2_bubble.toml"),
    include_str!("../../../assets/materials/sinking_pellet.toml"),
    include_str!("../../../assets/materials/bloodworm.toml"),
    include_str!("../../../assets/materials/algae_wafer.toml"),
    include_str!("../../../assets/materials/root_tab.toml"),
    include_str!("../../../assets/materials/liquid_fertiliser.toml"),
    include_str!("../../../assets/materials/waste.toml"),
    include_str!("../../../assets/materials/detritus.toml"),
    include_str!("../../../assets/materials/bubbler.toml"),
    include_str!("../../../assets/materials/filter_outlet.toml"),
    include_str!("../../../assets/materials/dead_plant.toml"),
    include_str!("../../../assets/materials/seed.toml"),
    include_str!("../../../assets/materials/cutting.toml"),
    include_str!("../../../assets/materials/stem.toml"),
    include_str!("../../../assets/materials/leaf.toml"),
    include_str!("../../../assets/materials/root.toml"),
    include_str!("../../../assets/materials/runner.toml"),
    include_str!("../../../assets/materials/floating_leaf.toml"),
    include_str!("../../../assets/materials/rhizome.toml"),
    include_str!("../../../assets/materials/egg.toml"),
    include_str!("../../../assets/materials/algae.toml"),
];

/// All materials, ordered by ID.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialTable {
    materials: Vec<Material>,
}

impl MaterialTable {
    /// The materials embedded from `assets/materials/*.toml`.
    ///
    /// # Errors
    ///
    /// Returns an error if an embedded file is invalid (a build defect,
    /// covered by tests).
    pub fn builtin() -> Result<Self, MaterialError> {
        Self::from_toml_sources(BUILTIN)
    }

    /// Parses one TOML document per material and validates the set.
    ///
    /// # Errors
    ///
    /// Returns an error if a source doesn't parse, a property is invalid,
    /// or IDs, names or references clash.
    pub fn from_toml_sources(sources: &[&str]) -> Result<Self, MaterialError> {
        let mut defs = Vec::with_capacity(sources.len());
        for (index, src) in sources.iter().enumerate() {
            let def: MaterialDef =
                toml::from_str(src).map_err(|source| MaterialError::Parse { index, source })?;
            defs.push(def);
        }
        defs.sort_by_key(|d| d.id);
        for pair in defs.windows(2) {
            if pair[0].id == pair[1].id {
                return Err(MaterialError::DuplicateId(pair[0].id));
            }
        }
        for (i, d) in defs.iter().enumerate() {
            if defs[..i].iter().any(|o| o.name == d.name) {
                return Err(MaterialError::DuplicateName(d.name.clone()));
            }
        }
        let materials = defs
            .iter()
            .map(|d| validate(d, &defs))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { materials })
    }

    /// The material with this ID, if any.
    #[must_use]
    pub fn get(&self, id: MaterialId) -> Option<&Material> {
        self.materials
            .binary_search_by_key(&id, |m| m.id)
            .ok()
            .map(|i| &self.materials[i])
    }

    /// The ID of the material with this name, if any.
    #[must_use]
    pub fn id(&self, name: &str) -> Option<MaterialId> {
        self.materials.iter().find(|m| m.name == name).map(|m| m.id)
    }

    /// All materials, ordered by ID.
    pub fn iter(&self) -> impl Iterator<Item = &Material> {
        self.materials.iter()
    }
}

fn validate(d: &MaterialDef, all: &[MaterialDef]) -> Result<Material, MaterialError> {
    let invalid = |reason| MaterialError::Invalid {
        name: d.name.clone(),
        reason,
    };
    if !(d.density.is_finite() && d.density > 0.0) {
        return Err(invalid("density must be finite and > 0"));
    }
    if d.dispersion > MAX_DISPERSION {
        return Err(invalid("dispersion must be at most 8"));
    }
    let colour = parse_colour(&d.colour).ok_or_else(|| invalid("colour must be \"#rrggbb\""))?;
    let slides = matches!(d.class, MovementClass::Powder | MovementClass::Floating);
    let repose = match (slides, d.repose) {
        (true, r) => {
            let r = r.unwrap_or(DEFAULT_REPOSE);
            if !REPOSE_RANGE.contains(&r) {
                return Err(invalid("repose must be within 15..=45 degrees"));
            }
            r
        }
        (false, None) => DEFAULT_REPOSE,
        (false, Some(_)) => return Err(invalid("repose only applies to powder and floating")),
    };
    let floating = d.class == MovementClass::Floating;
    let (float_time, float_density) = match (floating, d.float_time, d.float_density) {
        (true, Some(t), Some(fd)) => {
            // At least one 1/20 s tick, so a floater always floats.
            if !(0.05..=MAX_FLOAT_TIME).contains(&t) {
                return Err(invalid("float_time must be within 0.05..=3000 seconds"));
            }
            if !(fd.is_finite() && fd > 0.0) {
                return Err(invalid("float_density must be finite and > 0"));
            }
            (t, fd)
        }
        (true, _, _) => return Err(invalid("floating needs float_time and float_density")),
        (false, None, None) => (0.0, d.density),
        (false, _, _) => {
            return Err(invalid(
                "float_time and float_density only apply to floating",
            ));
        }
    };
    let pops_into = match &d.pops_into {
        None => None,
        Some(_) if d.class != MovementClass::Buoyant => {
            return Err(invalid("pops_into only applies to buoyant"));
        }
        Some(target) => Some(
            all.iter()
                .find(|o| &o.name == target)
                .map(|o| MaterialId(o.id))
                .ok_or_else(|| MaterialError::UnknownTarget {
                    name: d.name.clone(),
                    target: target.clone(),
                })?,
        ),
    };
    let opacity = match d.opacity {
        None if d.class.is_fluid() => 0.0,
        None => 1.0,
        Some(o) if o.is_finite() && (0.0..=1.0).contains(&o) => o,
        Some(_) => return Err(invalid("opacity must be within 0..=1")),
    };
    if d.passable && !slides {
        return Err(invalid("passable only applies to powder and floating"));
    }
    let palette = d
        .palette
        .as_deref()
        .map(|p| PalettePath::parse(p, Holds::Materials))
        .transpose()
        .map_err(|source| MaterialError::Palette {
            name: d.name.clone(),
            source,
        })?;
    Ok(Material {
        id: MaterialId(d.id),
        name: d.name.clone(),
        class: d.class,
        density: d.density,
        dispersion: d.dispersion,
        repose,
        float_time,
        float_density,
        pops_into,
        opacity,
        passable: d.passable,
        colour,
        palette,
    })
}

pub(crate) fn parse_colour(s: &str) -> Option<[u8; 3]> {
    let hex = s.strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_materials_load() {
        let t = MaterialTable::builtin().unwrap();
        for name in [
            "water",
            "air",
            "sand",
            "gravel",
            "rock",
            "air_bubble",
            "flake_food",
        ] {
            assert!(t.id(name).is_some(), "missing {name}");
        }
        let water = t.get(t.id("water").unwrap()).unwrap();
        assert!((water.density - 1.0).abs() < f32::EPSILON);
        let bubble = t.get(t.id("air_bubble").unwrap()).unwrap();
        assert_eq!(bubble.pops_into, t.id("air"));
        // Ordered by ID.
        let ids: Vec<_> = t.iter().map(|m| m.id).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn rejects_bad_definitions() {
        let dup = "id = 1\nname = \"a\"\nclass = \"solid\"\ndensity = 1.0\ncolour = \"#000000\"";
        let dup2 = "id = 1\nname = \"b\"\nclass = \"solid\"\ndensity = 1.0\ncolour = \"#000000\"";
        assert!(matches!(
            MaterialTable::from_toml_sources(&[dup, dup2]),
            Err(MaterialError::DuplicateId(1))
        ));
        let steep = "id = 1\nname = \"a\"\nclass = \"powder\"\ndensity = 1.5\nrepose = 60.0\ncolour = \"#000000\"";
        assert!(matches!(
            MaterialTable::from_toml_sources(&[steep]),
            Err(MaterialError::Invalid { .. })
        ));
        let unknown =
            "id = 1\nname = \"a\"\nclass = \"solid\"\ndensity = 1.0\ncolour = \"#000000\"\nfoo = 1";
        assert!(matches!(
            MaterialTable::from_toml_sources(&[unknown]),
            Err(MaterialError::Parse { index: 0, .. })
        ));
        let dangling = "id = 1\nname = \"a\"\nclass = \"buoyant\"\ndensity = 0.1\npops_into = \"nope\"\ncolour = \"#000000\"";
        assert!(matches!(
            MaterialTable::from_toml_sources(&[dangling]),
            Err(MaterialError::UnknownTarget { .. })
        ));
        let base = "id = 1\nname = \"a\"\nclass = \"solid\"\ndensity = 1.0\ncolour = \"#000000\"";
        assert!(matches!(
            MaterialTable::from_toml_sources(&[&format!("{base}\npalette = \"tools\"")]),
            Err(MaterialError::Palette {
                source: PalettePathError::WrongKind { .. },
                ..
            })
        ));
        assert!(matches!(
            MaterialTable::from_toml_sources(&[&format!("{base}\npalette = \"nowhere\"")]),
            Err(MaterialError::Palette {
                source: PalettePathError::Unknown { .. },
                ..
            })
        ));
        let rock =
            MaterialTable::from_toml_sources(&[&format!("{base}\npalette = \"build/hardscape\"")])
                .unwrap();
        assert_eq!(
            rock.get(MaterialId(1))
                .unwrap()
                .palette
                .map(PalettePath::name),
            Some("build/hardscape".to_owned())
        );
    }

    #[test]
    fn driftwood_variant_round_trips_wetness_and_glued() {
        for w in 0..=MAX_DRIFTWOOD_WETNESS {
            for glued in [false, true] {
                let v = driftwood_variant(w, glued);
                assert_eq!(driftwood_wetness(v), w);
                assert_eq!(v & GLUED_VARIANT_BIT != 0, glued);
            }
        }
        assert_eq!(
            driftwood_wetness(driftwood_variant(u8::MAX, false)),
            MAX_DRIFTWOOD_WETNESS
        );
        assert_eq!(
            driftwood_wetness(driftwood_variant(u8::MAX, true)),
            MAX_DRIFTWOOD_WETNESS
        );
    }
}
