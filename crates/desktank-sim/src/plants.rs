//! Plant species, loaded from TOML (SPEC §2.7, D18): growth form,
//! environment preferences, growth, hardiness, propagation and colours.
//!
//! Each species is one file in `assets/plants/`. Plants are individuals of
//! these species (the world's plant table, [`crate::World::plants`]); their
//! cells use a few generic part materials shared by every species
//! ([`Part`]), so a species costs no material id.
//!
//! **Cell layout** (T6b). A plant part cell's `state` holds its plant id
//! (1–65535). Its variant byte holds the part-local position in the low
//! [`POSITION_BITS`] bits (how far the cell is from where its part began:
//! stem height, leaf length, root depth; 0 for a mat) and colour noise in
//! the top two bits ([`plant_variant`]). A seed or cutting's `state` holds
//! the [`PlantSpeciesId`] it grows into (0 for the default species, the
//! lowest id).
//!
//! **Preferences.** For light, nutrient and CO₂ a species gives `min`,
//! `optimum` and `max`: the response is 0 outside `min..max`, 1 at the
//! optimum and linear in between ([`Range::response`]). The growth rate
//! scales with the product of the three responses. A `min` below 0 means
//! it grows with none of that factor (CO₂, which a tank without injection
//! lacks): at 0 the response is `-min / (optimum - min)`.
//!
//! Units (SPEC §2.1): times in seconds of world time, rates per minute of
//! world time, field values in field units.

use std::collections::BTreeMap;

use serde::Deserialize;
use thiserror::Error;

use crate::material::{MaterialId, MaterialTable, parse_colour};

/// Index of a plant species in the [`PlantSpeciesTable`]; fixed in each
/// species' TOML file so saves stay stable when species are added.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlantSpeciesId(pub u8);

/// Bits of a plant cell's variant byte holding its part-local position.
pub const POSITION_BITS: u32 = 6;
/// Largest part-local position a cell records.
pub const MAX_POSITION: u8 = (1 << POSITION_BITS) - 1;

/// A plant cell's variant byte: colour noise in the top two bits (from
/// `noise`'s top bits) and the part-local position (clamped to
/// [`MAX_POSITION`]) below.
#[must_use]
pub fn plant_variant(noise: u8, position: u8) -> u8 {
    (noise & !MAX_POSITION) | position.min(MAX_POSITION)
}

/// The part-local position in a plant cell's variant byte.
#[must_use]
pub fn plant_position(variant: u8) -> u8 {
    variant & MAX_POSITION
}

/// The generic plant part materials shared by every species (SPEC §2.7).
/// T6c adds blades, rhizomes, runners and floating leaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Part {
    /// Grows down and sideways through the substrate.
    Root,
    /// The upright stem; also the cells of a mat (moss).
    Stem,
    /// Grows sideways from stem nodes.
    Leaf,
}

impl Part {
    /// Every part, in the order part counts use.
    pub const ALL: [Self; 3] = [Self::Root, Self::Stem, Self::Leaf];

    /// The part's material name.
    #[must_use]
    pub fn material_name(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Stem => "stem",
            Self::Leaf => "leaf",
        }
    }

    /// Index into part counts.
    #[must_use]
    pub fn index(self) -> usize {
        self as usize
    }
}

/// An environment factor a plant responds to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Factor {
    /// The light field.
    Light,
    /// The nutrient field.
    Nutrient,
    /// The CO₂ field.
    Co2,
}

impl Factor {
    /// Every factor, in the order samples use.
    pub const ALL: [Self; 3] = [Self::Light, Self::Nutrient, Self::Co2];

    /// Lower-case name, e.g. for the inspect card.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Nutrient => "nutrients",
            Self::Co2 => "CO₂",
        }
    }
}

/// What is holding a plant back most: the factor with the lowest response,
/// and whether it is below or above the optimum.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Limit {
    /// Every factor is at its optimum.
    #[default]
    None,
    /// The factor is below the optimum.
    TooLow(Factor),
    /// The factor is above the optimum.
    TooHigh(Factor),
}

impl Limit {
    /// Words for the inspect card, e.g. "light too low".
    #[must_use]
    pub fn describe(self) -> String {
        match self {
            Self::None => "growing well".to_owned(),
            Self::TooLow(f) => format!("{} too low", f.name()),
            Self::TooHigh(f) => format!("{} too high", f.name()),
        }
    }

    /// A short machine-readable key: `none`, or `<factor>-low|high` with
    /// factor `light`, `nutrient` or `co2`.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::TooLow(Factor::Light) => "light-low",
            Self::TooHigh(Factor::Light) => "light-high",
            Self::TooLow(Factor::Nutrient) => "nutrient-low",
            Self::TooHigh(Factor::Nutrient) => "nutrient-high",
            Self::TooLow(Factor::Co2) => "co2-low",
            Self::TooHigh(Factor::Co2) => "co2-high",
        }
    }
}

/// A preference range for one factor (`min < optimum < max`).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Range {
    /// Below this the response is 0.
    pub min: f32,
    /// The response is 1 here.
    pub optimum: f32,
    /// Above this the response is 0.
    pub max: f32,
}

impl Range {
    /// 0 outside `min..max`, 1 at the optimum, linear in between.
    #[must_use]
    pub fn response(&self, v: f32) -> f32 {
        if !(v > self.min && v < self.max) {
            0.0
        } else if v <= self.optimum {
            (v - self.min) / (self.optimum - self.min)
        } else {
            (self.max - v) / (self.max - self.optimum)
        }
    }
}

/// A species' preference ranges.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    /// Light.
    pub light: Range,
    /// Nutrient.
    pub nutrient: Range,
    /// CO₂.
    pub co2: Range,
}

impl Preferences {
    /// The range for `factor`.
    #[must_use]
    pub fn range(&self, factor: Factor) -> Range {
        match factor {
            Factor::Light => self.light,
            Factor::Nutrient => self.nutrient,
            Factor::Co2 => self.co2,
        }
    }

    /// The product of the responses to `values` (light, nutrient, CO₂, in
    /// [`Factor::ALL`] order) and what limits it most.
    #[must_use]
    pub fn response(&self, values: [f32; 3]) -> (f32, Limit) {
        let mut product = 1.0;
        let mut worst = (1.0, Limit::None);
        for (f, v) in Factor::ALL.into_iter().zip(values) {
            let range = self.range(f);
            let r = range.response(v);
            product *= r;
            if r < worst.0 {
                let limit = if v <= range.optimum {
                    Limit::TooLow(f)
                } else {
                    Limit::TooHigh(f)
                };
                worst = (r, limit);
            }
        }
        (product, worst.1)
    }
}

/// How a species grows: a small growth program in Rust, parametrised by the
/// TOML (SPEC §2.7 *Growth forms*). T6c adds rosettes, carpets, floaters
/// and epiphytes.
#[derive(Clone, Debug, PartialEq)]
pub enum Form {
    /// An upright stem with leaves at nodes and roots in the substrate.
    Stem(StemForm),
    /// A mat that creeps over the surfaces it clings to (moss).
    Mat(MatForm),
}

/// Parameters of the stem form.
#[derive(Clone, Debug, PartialEq)]
pub struct StemForm {
    /// Tallest stem, in cells.
    pub height: u8,
    /// Stem cells from one node (pair of leaves) to the next (≥ 2, so
    /// leaves on one side sit at least a row apart).
    pub internode: u8,
    /// Longest leaf, in cells.
    pub leaf_length: u8,
    /// Deepest root, in cells from where it started.
    pub root_depth: u8,
    /// Substrates a seed or cutting roots in and roots grow through.
    pub substrates: Vec<MaterialId>,
}

/// Parameters of the mat form.
#[derive(Clone, Debug, PartialEq)]
pub struct MatForm {
    /// Materials it clings to; each mat cell must touch one, or it falls
    /// off as dead plant matter.
    pub on: Vec<MaterialId>,
}

/// Growth speed and size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Growth {
    /// New cells per minute at full response.
    pub rate: f32,
    /// Mature size in cells: it stops growing there.
    pub size: u32,
    /// Nutrient each new cell takes from the field where it grows.
    pub cost: f32,
    /// Lifespan in seconds, if it ages: past it, health declines.
    pub lifespan: Option<f32>,
}

/// How a species copes with conditions outside its ranges.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hardiness {
    /// Seconds outside its ranges before health starts to fall.
    pub tolerance: f32,
    /// Health lost per second after that (health runs from 0 to 1).
    pub decline: f32,
    /// Health regained per second while inside its ranges.
    pub recovery: f32,
}

/// How a species spreads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Propagation {
    /// Grows from seeds.
    pub seeds: bool,
    /// Grows from cuttings.
    pub cuttings: bool,
    /// A cut-off piece holding a stem lives on as a new plant.
    pub fragments: bool,
}

/// Colour ramps per part, dark to light (sRGB). The renderer shades each
/// plant cell from its part's ramp by position (T6d); the palette swatch
/// and the web picker use the middle leaf shade.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ramps {
    /// Root shades.
    pub root: Vec<[u8; 3]>,
    /// Stem shades.
    pub stem: Vec<[u8; 3]>,
    /// Leaf shades.
    pub leaf: Vec<[u8; 3]>,
}

/// Most shades in one ramp.
pub const MAX_RAMP: usize = 8;

/// How a species' leaves are drawn at two texels per cell side (T6d,
/// `[look] leaf` in its TOML). The discriminant is what the shader reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum LeafShape {
    /// A broad blade on a short petiole, rising to a pointed tip
    /// (hygrophila type).
    #[default]
    Lanceolate = 0,
    /// A fine one-texel blade rising from base to tip (rotala type).
    Needle = 1,
    /// A broad blade with a blunt, rounded tip (ludwigia or bacopa type).
    Round = 2,
    /// A midrib with a fringe of fine teeth above it (cabomba type).
    Feather = 3,
}

impl LeafShape {
    /// Every shape, in discriminant order.
    pub const ALL: [Self; 4] = [Self::Lanceolate, Self::Needle, Self::Round, Self::Feather];
}

/// How a species' stem cells are drawn (`[look] stem`). A mat's cells are
/// stems, so mats pick a mat shape here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum StemShape {
    /// A one-texel stalk that widens at nodes and the growing tip.
    #[default]
    Stalk = 0,
    /// A full-width stem, shaded on one side (bacopa type).
    Thick = 1,
    /// A soft mat whose exposed edges break into a ragged fringe (java
    /// moss type).
    Fuzz = 2,
    /// A mat of layered, downswept fronds (christmas moss type).
    Frond = 3,
}

impl StemShape {
    /// Every shape, in discriminant order.
    pub const ALL: [Self; 4] = [Self::Stalk, Self::Thick, Self::Fuzz, Self::Frond];
}

/// How a species' root cells are drawn (`[look] root`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum RootShape {
    /// A thin, wandering one-texel strand.
    #[default]
    Strand = 0,
    /// A full-width root (rhizome type).
    Thick = 1,
}

impl RootShape {
    /// Every shape, in discriminant order.
    pub const ALL: [Self; 2] = [Self::Strand, Self::Thick];
}

/// A species' drawn shapes per part (SPEC §2.7 *Appearance*, T6d). Every
/// key is optional in the TOML: leaves default to lanceolate, roots to
/// strands, and stems to stalks (fuzz for a mat).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Look {
    /// Leaf shape.
    pub leaf: LeafShape,
    /// Stem (or mat) shape.
    pub stem: StemShape,
    /// Root shape.
    pub root: RootShape,
}
/// Fastest growth rate, in cells per minute at full response (the plant
/// update grows at most 8 cells per plant every 5 s).
pub const MAX_RATE: f32 = 90.0;

/// One plant species, validated.
#[derive(Clone, Debug, PartialEq)]
pub struct PlantSpecies {
    /// Stable ID.
    pub id: PlantSpeciesId,
    /// Unique name, e.g. `"hygrophila"`.
    pub name: String,
    /// One line for the inspect card.
    pub description: String,
    /// Growth form and its parameters.
    pub form: Form,
    /// Environment preferences.
    pub prefer: Preferences,
    /// Growth speed and size.
    pub growth: Growth,
    /// Hardiness.
    pub hardiness: Hardiness,
    /// How it spreads.
    pub propagation: Propagation,
    /// Colour ramps.
    pub colours: Ramps,
    /// Drawn shapes.
    pub look: Look,
}

impl PlantSpecies {
    /// Its main colour: the middle leaf shade (stem for a mat).
    #[must_use]
    pub fn colour(&self) -> [u8; 3] {
        let ramp = match self.form {
            Form::Mat(_) => &self.colours.stem,
            Form::Stem(_) => &self.colours.leaf,
        };
        ramp.get(ramp.len() / 2)
            .copied()
            .unwrap_or([0x40, 0x90, 0x40])
    }

    /// Whether it grows from `propagule` (lists it under `propagation`).
    #[must_use]
    pub fn grows_from(&self, propagule: Propagule) -> bool {
        let p = self.propagation;
        match propagule {
            Propagule::Seeds => p.seeds,
            Propagule::Cuttings => p.cuttings,
            Propagule::Fragments => p.fragments,
        }
    }

    /// The ways it grows, in [`Propagule::ALL`] order (never empty).
    pub fn propagules(&self) -> impl Iterator<Item = Propagule> + '_ {
        Propagule::ALL.into_iter().filter(|&p| self.grows_from(p))
    }

    /// The propagule the palette pours until another is picked: cuttings
    /// if it grows from them (they root soonest), else its first way.
    #[must_use]
    pub fn default_propagule(&self) -> Propagule {
        if self.propagation.cuttings {
            Propagule::Cuttings
        } else {
            self.propagules().next().unwrap_or(Propagule::Cuttings)
        }
    }
}

/// A way to start a plant from the palette: what the plants slot pours
/// (the names are those of a species' `propagation` list).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Propagule {
    /// Seeds: the `seed` material, slow to germinate.
    Seeds,
    /// Stem cuttings: the `cutting` material, quick to root.
    Cuttings,
    /// Fragments: a cut-off piece of the plant. Poured, a fragment is the
    /// `cutting` material (a loose piece that roots or clings like one);
    /// it has no material of its own.
    Fragments,
}

impl Propagule {
    /// Every propagule, in the order hosts list them.
    pub const ALL: [Self; 3] = [Self::Seeds, Self::Cuttings, Self::Fragments];

    /// Its name, as a species' `propagation` list and scripts spell it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Seeds => "seeds",
            Self::Cuttings => "cuttings",
            Self::Fragments => "fragments",
        }
    }

    /// The propagule with this name, if any.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.name() == name)
    }

    /// The material poured for it.
    #[must_use]
    pub fn material_name(self) -> &'static str {
        match self {
            Self::Seeds => "seed",
            Self::Cuttings | Self::Fragments => "cutting",
        }
    }
}

/// Error loading or validating plant species.
#[derive(Debug, Error)]
pub enum PlantSpeciesError {
    /// A TOML source didn't parse or didn't match the schema.
    #[error("plant species source {index}: {source}")]
    Parse {
        /// Index of the source in the list passed in.
        index: usize,
        /// The parse error.
        #[source]
        source: toml::de::Error,
    },
    /// Two species share an ID (ids start at 1).
    #[error("duplicate or zero plant species id {0}")]
    BadId(u8),
    /// Two species share a name.
    #[error("duplicate plant species name {0:?}")]
    DuplicateName(String),
    /// A property is out of range.
    #[error("plant species {name:?}: {reason}")]
    Invalid {
        /// Species name.
        name: String,
        /// What is wrong.
        reason: &'static str,
    },
    /// A material it names doesn't exist.
    #[error("plant species {name:?}: unknown material {material:?}")]
    UnknownMaterial {
        /// Species name.
        name: String,
        /// The missing material.
        material: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpeciesDef {
    id: u8,
    name: String,
    description: String,
    form: FormName,
    stem: Option<StemDef>,
    mat: Option<MatDef>,
    prefer: Preferences,
    growth: GrowthDef,
    hardiness: HardinessDef,
    propagation: Vec<Propagule>,
    colours: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    look: LookDef,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LookDef {
    leaf: Option<LeafShape>,
    stem: Option<StemShape>,
    root: Option<RootShape>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FormName {
    Stem,
    Mat,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StemDef {
    height: u8,
    internode: u8,
    leaf_length: u8,
    root_depth: u8,
    substrates: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MatDef {
    on: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GrowthDef {
    rate: f32,
    size: u32,
    cost: f32,
    lifespan: Option<f32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HardinessDef {
    tolerance: f32,
    decline: f32,
    recovery: f32,
}

/// The plant species embedded in the binary (T6b: T6's plant and moss, and
/// a high-light stem plant; then ludwigia, cabomba, bacopa and christmas
/// moss).
const BUILTIN: &[&str] = &[
    include_str!("../../../assets/plants/hygrophila.toml"),
    include_str!("../../../assets/plants/java_moss.toml"),
    include_str!("../../../assets/plants/rotala.toml"),
    include_str!("../../../assets/plants/ludwigia.toml"),
    include_str!("../../../assets/plants/cabomba.toml"),
    include_str!("../../../assets/plants/bacopa.toml"),
    include_str!("../../../assets/plants/christmas_moss.toml"),
];

/// All plant species, ordered by ID.
#[derive(Clone, Debug, PartialEq)]
pub struct PlantSpeciesTable {
    species: Vec<PlantSpecies>,
}

impl PlantSpeciesTable {
    /// The species embedded from `assets/plants/*.toml`, with materials
    /// looked up in `materials`. Materials the table lacks (a test's own
    /// set) are left out of substrates and surfaces.
    ///
    /// # Errors
    ///
    /// Returns an error if an embedded file is invalid (a build defect,
    /// covered by tests).
    pub fn builtin_for(materials: &MaterialTable) -> Result<Self, PlantSpeciesError> {
        Self::load(BUILTIN, materials, true)
    }

    /// Parses one TOML document per species and validates the set.
    ///
    /// # Errors
    ///
    /// Returns an error if a source doesn't parse, a property is invalid,
    /// IDs or names clash, or a material name is unknown.
    pub fn from_toml_sources(
        sources: &[&str],
        materials: &MaterialTable,
    ) -> Result<Self, PlantSpeciesError> {
        Self::load(sources, materials, false)
    }

    fn load(
        sources: &[&str],
        materials: &MaterialTable,
        skip_unknown: bool,
    ) -> Result<Self, PlantSpeciesError> {
        let mut defs = Vec::with_capacity(sources.len());
        for (index, src) in sources.iter().enumerate() {
            let def: SpeciesDef =
                toml::from_str(src).map_err(|source| PlantSpeciesError::Parse { index, source })?;
            defs.push(def);
        }
        defs.sort_by_key(|d| d.id);
        for (i, d) in defs.iter().enumerate() {
            if d.id == 0 || defs[..i].iter().any(|o| o.id == d.id) {
                return Err(PlantSpeciesError::BadId(d.id));
            }
            if defs[..i].iter().any(|o| o.name == d.name) {
                return Err(PlantSpeciesError::DuplicateName(d.name.clone()));
            }
        }
        let species = defs
            .iter()
            .map(|d| validate(d, materials, skip_unknown))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { species })
    }

    /// The species with this ID, if any.
    #[must_use]
    pub fn get(&self, id: PlantSpeciesId) -> Option<&PlantSpecies> {
        self.species
            .binary_search_by_key(&id, |s| s.id)
            .ok()
            .map(|i| &self.species[i])
    }

    /// The ID of the species with this name, if any.
    #[must_use]
    pub fn id(&self, name: &str) -> Option<PlantSpeciesId> {
        self.species.iter().find(|s| s.name == name).map(|s| s.id)
    }

    /// The species a seed or cutting with `state` grows into: that id if it
    /// exists, else the default (the lowest id).
    #[must_use]
    pub fn for_state(&self, state: u16) -> Option<&PlantSpecies> {
        u8::try_from(state)
            .ok()
            .and_then(|id| self.get(PlantSpeciesId(id)))
            .or_else(|| self.species.first())
    }

    /// The default species (the lowest id), for plant cells made by edits
    /// and for seeds and cuttings poured as plain materials.
    #[must_use]
    pub fn default_species(&self) -> Option<&PlantSpecies> {
        self.species.first()
    }

    /// All species, ordered by ID.
    pub fn iter(&self) -> impl Iterator<Item = &PlantSpecies> {
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
    skip_unknown: bool,
) -> Result<PlantSpecies, PlantSpeciesError> {
    let invalid = |reason| PlantSpeciesError::Invalid {
        name: d.name.clone(),
        reason,
    };
    let lookup = |names: &[String]| -> Result<Vec<MaterialId>, PlantSpeciesError> {
        let mut out = Vec::with_capacity(names.len());
        for n in names {
            match materials.id(n) {
                Some(id) => out.push(id),
                None if skip_unknown => {}
                None => {
                    return Err(PlantSpeciesError::UnknownMaterial {
                        name: d.name.clone(),
                        material: n.clone(),
                    });
                }
            }
        }
        Ok(out)
    };
    let form = match (d.form, &d.stem, &d.mat) {
        (FormName::Stem, Some(s), None) => {
            let max = MAX_POSITION;
            if !(1..=max).contains(&s.height)
                || !(2..=max).contains(&s.internode)
                || !(1..=max).contains(&s.leaf_length)
                || !(1..=max).contains(&s.root_depth)
            {
                return Err(invalid(
                    "stem height, leaf_length and root_depth must be 1..=63 and internode 2..=63",
                ));
            }
            Form::Stem(StemForm {
                height: s.height,
                internode: s.internode,
                leaf_length: s.leaf_length,
                root_depth: s.root_depth,
                substrates: lookup(&s.substrates)?,
            })
        }
        (FormName::Mat, None, Some(m)) => Form::Mat(MatForm { on: lookup(&m.on)? }),
        _ => {
            return Err(invalid(
                "needs exactly the [stem] or [mat] table its form names",
            ));
        }
    };
    for f in Factor::ALL {
        let r = d.prefer.range(f);
        let finite = r.min.is_finite() && r.optimum.is_finite() && r.max.is_finite();
        if !(finite && r.min < r.optimum && r.optimum < r.max) {
            return Err(invalid("each preference needs min < optimum < max"));
        }
    }
    let g = &d.growth;
    if !(g.rate.is_finite() && g.rate > 0.0 && g.rate <= MAX_RATE) {
        return Err(invalid(
            "growth rate must be within (0, 90] cells per minute",
        ));
    }
    if !(1..=100_000).contains(&g.size) {
        return Err(invalid("growth size must be 1..=100000 cells"));
    }
    if !(g.cost.is_finite() && (0.0..=1.0).contains(&g.cost)) {
        return Err(invalid("growth cost must be within 0..=1"));
    }
    if g.lifespan.is_some_and(|l| !(l.is_finite() && l > 0.0)) {
        return Err(invalid("lifespan must be > 0 seconds"));
    }
    let h = &d.hardiness;
    let ok = |v: f32| v.is_finite() && v >= 0.0;
    if !(ok(h.tolerance) && ok(h.decline) && h.decline > 0.0 && ok(h.recovery)) {
        return Err(invalid(
            "hardiness tolerance and recovery must be >= 0 and decline > 0",
        ));
    }
    let mut propagation = Propagation::default();
    for p in &d.propagation {
        match p {
            Propagule::Seeds => propagation.seeds = true,
            Propagule::Cuttings => propagation.cuttings = true,
            Propagule::Fragments => propagation.fragments = true,
        }
    }
    if propagation == Propagation::default() {
        return Err(invalid("propagation needs at least one way to spread"));
    }
    let mut ramps = [Vec::new(), Vec::new(), Vec::new()];
    for (key, shades) in &d.colours {
        let slot = match key.as_str() {
            "root" => 0,
            "stem" => 1,
            "leaf" => 2,
            _ => return Err(invalid("colours are given for root, stem and leaf")),
        };
        if !(1..=MAX_RAMP).contains(&shades.len()) {
            return Err(invalid("a colour ramp has 1..=8 shades"));
        }
        for s in shades {
            ramps[slot]
                .push(parse_colour(s).ok_or_else(|| invalid("colours must be \"#rrggbb\""))?);
        }
    }
    if ramps.iter().any(Vec::is_empty) {
        return Err(invalid("colours need a root, stem and leaf ramp"));
    }
    let [root, stem, leaf] = ramps;
    let default_stem = match form {
        Form::Stem(_) => StemShape::Stalk,
        Form::Mat(_) => StemShape::Fuzz,
    };
    let look = Look {
        leaf: d.look.leaf.unwrap_or_default(),
        stem: d.look.stem.unwrap_or(default_stem),
        root: d.look.root.unwrap_or_default(),
    };
    let mat_shape = matches!(look.stem, StemShape::Fuzz | StemShape::Frond);
    if mat_shape != matches!(form, Form::Mat(_)) {
        return Err(invalid(
            "look stem is stalk or thick for a stem form, fuzz or frond for a mat",
        ));
    }
    Ok(PlantSpecies {
        id: PlantSpeciesId(d.id),
        name: d.name.clone(),
        description: d.description.clone(),
        form,
        prefer: d.prefer,
        growth: Growth {
            rate: g.rate,
            size: g.size,
            cost: g.cost,
            lifespan: g.lifespan,
        },
        hardiness: Hardiness {
            tolerance: h.tolerance,
            decline: h.decline,
            recovery: h.recovery,
        },
        propagation,
        colours: Ramps { root, stem, leaf },
        look,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = r##"
        id = 9
        name = "test_weed"
        description = "A test plant."
        form = "stem"
        propagation = ["cuttings"]
        [stem]
        height = 20
        internode = 2
        leaf_length = 3
        root_depth = 6
        substrates = ["sand"]
        [prefer]
        light = { min = 0.1, optimum = 0.5, max = 1.0 }
        nutrient = { min = 0.0, optimum = 0.4, max = 2.0 }
        co2 = { min = -1.0, optimum = 1.0, max = 3.0 }
        [growth]
        rate = 10.0
        size = 100
        cost = 0.01
        [hardiness]
        tolerance = 10.0
        decline = 0.01
        recovery = 0.02
        [colours]
        root = ["#d9cfa8"]
        stem = ["#2f5a20", "#3f7a2e"]
        leaf = ["#4c8a30", "#6dbb45", "#8fd660"]
    "##;

    fn materials() -> MaterialTable {
        MaterialTable::builtin().unwrap()
    }

    #[test]
    fn builtin_species_load() {
        let m = materials();
        let t = PlantSpeciesTable::builtin_for(&m).unwrap();
        assert_eq!(t.len(), 7);
        let first = t.default_species().unwrap();
        assert_eq!(first.name, "hygrophila", "T6's plant is the default");
        assert!(matches!(first.form, Form::Stem(_)));
        let moss = t.get(t.id("java_moss").unwrap()).unwrap();
        assert!(matches!(&moss.form, Form::Mat(m) if m.on.len() == 2));
        assert_eq!(t.for_state(0).unwrap().id, first.id);
        assert_eq!(t.for_state(u16::from(moss.id.0)).unwrap().id, moss.id);
        assert_eq!(t.for_state(999).unwrap().id, first.id);
        // The two stem plants want different light, so each wins somewhere.
        let rotala = t.get(t.id("rotala").unwrap()).unwrap();
        assert!(rotala.prefer.light.optimum > first.prefer.light.optimum);
        assert!(rotala.prefer.light.min > first.prefer.light.min);
    }

    #[test]
    fn species_list_their_propagules_and_default_to_cuttings() {
        let m = materials();
        let t = PlantSpeciesTable::builtin_for(&m).unwrap();
        let get = |n| t.get(t.id(n).unwrap()).unwrap();
        let ways = |n| get(n).propagules().collect::<Vec<_>>();
        assert_eq!(ways("hygrophila"), Propagule::ALL);
        assert_eq!(ways("rotala"), [Propagule::Cuttings]);
        assert_eq!(ways("java_moss"), [Propagule::Fragments]);
        assert_eq!(get("hygrophila").default_propagule(), Propagule::Cuttings);
        assert_eq!(get("java_moss").default_propagule(), Propagule::Fragments);
        // Every propagule pours a material the built-in table has.
        for p in Propagule::ALL {
            assert_eq!(Propagule::from_name(p.name()), Some(p));
            assert!(m.id(p.material_name()).is_some(), "{p:?}");
        }
        assert_eq!(Propagule::from_name("spores"), None);
    }

    #[test]
    fn responses_are_linear_with_a_peak_at_the_optimum() {
        let r = Range {
            min: 0.0,
            optimum: 1.0,
            max: 3.0,
        };
        assert!((r.response(0.5) - 0.5).abs() < 1e-6);
        assert!((r.response(1.0) - 1.0).abs() < 1e-6);
        assert!((r.response(2.0) - 0.5).abs() < 1e-6);
        assert!(r.response(0.0).abs() < 1e-6 && r.response(3.5).abs() < 1e-6);
        assert!(r.response(f32::NAN).abs() < 1e-6);
        let t = PlantSpeciesTable::from_toml_sources(&[OK], &materials()).unwrap();
        let s = t.get(PlantSpeciesId(9)).unwrap();
        // Light 0.2 is a quarter of the way to the optimum; no CO₂ is half
        // the CO₂ response; nutrient at its optimum.
        let (p, limit) = s.prefer.response([0.2, 0.4, 0.0]);
        assert!((p - 0.125).abs() < 1e-6, "{p}");
        assert_eq!(limit, Limit::TooLow(Factor::Light));
        let (p, limit) = s.prefer.response([0.5, 2.5, 1.0]);
        assert!(p.abs() < 1e-6);
        assert_eq!(limit, Limit::TooHigh(Factor::Nutrient));
        assert_eq!(limit.describe(), "nutrients too high");
        assert_eq!(s.prefer.response([0.5, 0.4, 1.0]).1, Limit::None);
    }

    #[test]
    fn rejects_bad_definitions() {
        let m = materials();
        let bad = |from: &str, to: &str| {
            let src = OK.replace(from, to);
            assert_ne!(src, OK, "{from:?} not in the template");
            PlantSpeciesTable::from_toml_sources(&[&src], &m)
        };
        let invalid = |r: Result<PlantSpeciesTable, PlantSpeciesError>| {
            assert!(matches!(r, Err(PlantSpeciesError::Invalid { .. })), "{r:?}");
        };
        invalid(bad("optimum = 0.5", "optimum = 0.05"));
        invalid(bad("internode = 2", "internode = 1"));
        invalid(bad("height = 20", "height = 64"));
        invalid(bad("rate = 10.0", "rate = 0.0"));
        invalid(bad("decline = 0.01", "decline = 0.0"));
        invalid(bad("[\"cuttings\"]", "[]"));
        invalid(bad("form = \"stem\"", "form = \"mat\""));
        invalid(bad("root = [\"#d9cfa8\"]", "root = []"));
        invalid(bad("root = [\"#d9cfa8\"]", "trunk = [\"#d9cfa8\"]"));
        assert!(matches!(
            bad("[\"sand\"]", "[\"lava\"]"),
            Err(PlantSpeciesError::UnknownMaterial { .. })
        ));
        assert!(matches!(
            bad("[\"cuttings\"]", "[\"spores\"]"),
            Err(PlantSpeciesError::Parse { .. })
        ));
        assert!(matches!(
            bad("id = 9", "id = 0"),
            Err(PlantSpeciesError::BadId(0))
        ));
        assert!(matches!(
            PlantSpeciesTable::from_toml_sources(&[OK, OK], &m),
            Err(PlantSpeciesError::BadId(9))
        ));
    }

    #[test]
    fn looks_are_read_with_defaults_by_form() {
        let m = materials();
        let t = PlantSpeciesTable::builtin_for(&m).unwrap();
        let look = |n: &str| t.get(t.id(n).unwrap()).unwrap().look;
        assert_eq!(look("hygrophila").leaf, LeafShape::Lanceolate);
        assert_eq!(look("rotala").leaf, LeafShape::Needle);
        assert_eq!(look("java_moss").stem, StemShape::Fuzz);
        // Absent keys take the defaults.
        let s = PlantSpeciesTable::from_toml_sources(&[OK], &m).unwrap();
        assert_eq!(s.get(PlantSpeciesId(9)).unwrap().look, Look::default());
        let custom =
            format!("{OK}\n[look]\nleaf = \"feather\"\nstem = \"thick\"\nroot = \"thick\"\n");
        let s = PlantSpeciesTable::from_toml_sources(&[&custom], &m).unwrap();
        let look = s.get(PlantSpeciesId(9)).unwrap().look;
        assert_eq!(
            (look.leaf, look.stem, look.root),
            (LeafShape::Feather, StemShape::Thick, RootShape::Thick)
        );
        // A mat shape on a stem plant is invalid.
        let mat_on_stem = format!("{OK}\n[look]\nstem = \"fuzz\"\n");
        assert!(matches!(
            PlantSpeciesTable::from_toml_sources(&[&mat_on_stem], &m),
            Err(PlantSpeciesError::Invalid { .. })
        ));
        // Unknown shapes and keys are errors.
        for bad in ["leaf = \"spiky\"", "flower = \"round\""] {
            let src = format!("{OK}\n[look]\n{bad}\n");
            assert!(matches!(
                PlantSpeciesTable::from_toml_sources(&[&src], &m),
                Err(PlantSpeciesError::Parse { .. })
            ));
        }
    }

    #[test]
    fn ludwigia_cabomba_bacopa_and_christmas_moss_load_with_their_care_and_look() {
        let m = materials();
        let t = PlantSpeciesTable::builtin_for(&m).unwrap();
        let get = |n: &str| t.get(t.id(n).unwrap()).unwrap();
        let stem = |n: &str| match &get(n).form {
            Form::Stem(f) => f.clone(),
            Form::Mat(_) => panic!("{n} is a stem plant"),
        };
        let (hyg, rotala, java) = (get("hygrophila"), get("rotala"), get("java_moss"));
        let (ludwigia, cabomba, bacopa, xmas) = (
            get("ludwigia"),
            get("cabomba"),
            get("bacopa"),
            get("christmas_moss"),
        );
        // Ludwigia: round leaves on a stalk, reddening up the stem; light
        // between hygrophila's and rotala's.
        assert_eq!(
            (ludwigia.look.leaf, ludwigia.look.stem),
            (LeafShape::Round, StemShape::Stalk)
        );
        let red = |c: [u8; 3]| c[0] > c[1] && c[0] > c[2];
        assert!(red(*ludwigia.colours.leaf.last().unwrap()));
        assert!(!red(ludwigia.colours.leaf[0]));
        assert!(ludwigia.prefer.light.optimum > hyg.prefer.light.optimum);
        assert!(ludwigia.prefer.light.optimum < rotala.prefer.light.optimum);
        // Cabomba: feathery fans, the longest leaves, fast, wanting fairly
        // strong light and quick to suffer outside it.
        assert_eq!(cabomba.look.leaf, LeafShape::Feather);
        assert!(stem("cabomba").leaf_length > stem("hygrophila").leaf_length);
        assert!(cabomba.prefer.light.min > ludwigia.prefer.light.min);
        assert!(cabomba.growth.rate > rotala.growth.rate);
        assert!(cabomba.hardiness.tolerance < hyg.hardiness.tolerance);
        // Bacopa: round leaves on a thick stem, short and fleshy; the
        // slowest stem plant and the hardiest.
        assert_eq!(
            (bacopa.look.leaf, bacopa.look.stem),
            (LeafShape::Round, StemShape::Thick)
        );
        assert!(stem("bacopa").leaf_length < stem("ludwigia").leaf_length);
        for other in [hyg, rotala, ludwigia, cabomba] {
            assert!(bacopa.growth.rate < other.growth.rate, "{}", other.name);
            assert!(bacopa.hardiness.tolerance > other.hardiness.tolerance);
        }
        // Christmas moss: a frond mat, brighter than java moss and slower.
        assert!(matches!(&xmas.form, Form::Mat(f) if f.on.len() == 2));
        assert_eq!(
            (java.look.stem, xmas.look.stem),
            (StemShape::Fuzz, StemShape::Frond)
        );
        let bright = |c: [u8; 3]| u32::from(c[0]) + u32::from(c[1]) + u32::from(c[2]);
        assert!(bright(xmas.colour()) > bright(java.colour()));
        assert!(xmas.growth.rate < java.growth.rate);
        assert_eq!(
            xmas.propagules().collect::<Vec<_>>(),
            [Propagule::Fragments]
        );
        // Each species has its own swatch colour in the picker.
        let all: Vec<_> = t.iter().collect();
        for (i, a) in all.iter().enumerate() {
            for b in &all[..i] {
                assert_ne!(a.colour(), b.colour(), "{} {}", a.name, b.name);
            }
        }
    }

    #[test]
    fn variant_bytes_keep_noise_and_position_apart() {
        let v = plant_variant(0b1011_0110, 5);
        assert_eq!(plant_position(v), 5);
        assert_eq!(v >> POSITION_BITS, 0b10);
        assert_eq!(plant_position(plant_variant(0xff, 200)), MAX_POSITION);
    }
}
