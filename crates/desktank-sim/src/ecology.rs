//! The reaction table, slow material processes, field settings and device
//! settings, loaded from TOML (SPEC §2.1, §2.2, T5).
//!
//! `assets/reactions.toml` holds what living cells do on random ticks:
//!
//! - `[[reaction]]`: `a` next to `b` becomes `into` (and `b` becomes
//!   `b_into`) with probability `p` per world second, optionally gated on a
//!   field (`when = { field, below | above }`) and releasing into a field;
//! - `[[decay]]`: after `after` seconds a cell becomes `into`;
//! - `[[emit]]`: a cell adds `rate` per second to a field, up to `up_to`;
//! - `[[buffer]]`: a cell stores up to `capacity` and trades with a field
//!   towards `level`;
//! - `[[grow]]` (T6): a cell grows a new cell into a neighbour (plants,
//!   moss, algae), while fields allow, up to a structure size;
//! - `[[wilt]]` (T6): a cell turns into another while a need fails or it
//!   touches none of the materials it must cling to;
//! - `[[spawn]]` (T6): cells appear on exposed surfaces (algae spores).
//!
//! `assets/fields.toml` holds the field constants, the filter outlet's
//! current, the bubbler, the water change and the water-quality score. Material names are resolved against a
//! [`MaterialTable`]; [`Ecology::builtin_for`] drops entries naming
//! materials a custom table lacks, while [`Ecology::builtin`] requires all
//! of them (a test guards the embedded files).
//!
//! A cell's `state` holds its age in ticks for decay and emit, or the
//! amount drawn from a buffer, or a plant cell's distance from where its
//! part started (a `[[grow]]` count), so a material may use only one of
//! these, and floating materials (whose state is their float timer) and
//! rigid ones (whose state is their body id, D17) none. Reactions may involve rigid
//! cells: their conversions go through the body bookkeeping.

use serde::Deserialize;
use thiserror::Error;

use crate::field::{FIELD_DT, FIELD_SCALE, FieldKind, MAX_DIFFUSION};
use crate::material::{MaterialId, MaterialTable, MovementClass};
use crate::world::TICKS_PER_SECOND;

/// Longest decay time, in seconds (ages are `u16` ticks).
pub const MAX_DECAY_TIME: f32 = 3000.0;

const REACTIONS: &str = include_str!("../../../assets/reactions.toml");
const FIELDS: &str = include_str!("../../../assets/fields.toml");

/// Error loading or resolving the ecology files.
#[derive(Debug, Error)]
pub enum EcologyError {
    /// A file didn't parse or didn't match the schema.
    #[error("{file}: {source}")]
    Parse {
        /// Which file.
        file: &'static str,
        /// The parse error.
        #[source]
        source: toml::de::Error,
    },
    /// An entry names a material the table doesn't have.
    #[error("unknown material {0:?}")]
    UnknownMaterial(String),
    /// A value is out of range or the entry doesn't fit the material.
    #[error("{what}: {reason}")]
    Invalid {
        /// The entry, e.g. `decay for "detritus"`.
        what: String,
        /// What is wrong.
        reason: &'static str,
    },
}

/// A field amount added when a rule fires.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseDef {
    field: FieldKind,
    amount: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct WhenDef {
    field: FieldKind,
    below: Option<f32>,
    above: Option<f32>,
}

/// One condition or a list of them (all must hold).
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
enum WhenList {
    One(WhenDef),
    Many(Vec<WhenDef>),
}

impl Default for WhenList {
    fn default() -> Self {
        Self::Many(Vec::new())
    }
}

impl WhenList {
    fn as_slice(&self) -> &[WhenDef] {
        match self {
            Self::One(w) => core::slice::from_ref(w),
            Self::Many(v) => v,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReactionDef {
    a: String,
    b: String,
    into: String,
    b_into: Option<String>,
    p: f32,
    #[serde(default)]
    when: WhenList,
    release: Option<ReleaseDef>,
}

/// A direction a cell can grow in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum DirDef {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoostDef {
    field: FieldKind,
    above: f32,
    factor: f32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GrowDef {
    material: String,
    into: Vec<String>,
    makes: String,
    dirs: Vec<DirDef>,
    p: f32,
    max: Option<u16>,
    #[serde(default)]
    needs: WhenList,
    boost: Option<BoostDef>,
    consume: Option<ReleaseDef>,
    #[serde(default)]
    touching: Vec<String>,
    #[serde(default)]
    alone: bool,
    becomes: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WiltDef {
    material: String,
    into: String,
    p: f32,
    #[serde(default)]
    unless: WhenList,
    #[serde(default)]
    attached: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnDef {
    material: String,
    on: Vec<String>,
    rate: f32,
    #[serde(default)]
    when: WhenList,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecayDef {
    material: String,
    after: f32,
    into: String,
    release: Option<ReleaseDef>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmitDef {
    material: String,
    field: FieldKind,
    rate: f32,
    up_to: Option<f32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BufferDef {
    material: String,
    field: FieldKind,
    level: f32,
    capacity: f32,
    rate: f32,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReactionsFile {
    #[serde(default)]
    reaction: Vec<ReactionDef>,
    #[serde(default)]
    decay: Vec<DecayDef>,
    #[serde(default)]
    emit: Vec<EmitDef>,
    #[serde(default)]
    buffer: Vec<BufferDef>,
    #[serde(default)]
    grow: Vec<GrowDef>,
    #[serde(default)]
    wilt: Vec<WiltDef>,
    #[serde(default)]
    spawn: Vec<SpawnDef>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiffusingDef {
    diffusion: f32,
    decay: f32,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LightDef {
    surface: f32,
    water: f32,
    opaque: f32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CurrentDef {
    material: String,
    speed: f32,
    reach: u32,
    spread: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BubblerDef {
    material: String,
    emits: String,
    rate: f32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WaterChangeDef {
    fraction: f32,
    siphon: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct QualityDef {
    nutrient_ok: f32,
    nutrient_bad: f32,
    nutrient_weight: f32,
    pollutants: Vec<String>,
    pollution_bad: f32,
    pollution_weight: f32,
    plants: Vec<String>,
    plant_full: f32,
    plant_bonus: f32,
    algae: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FieldsFile {
    nutrient: DiffusingDef,
    co2: DiffusingDef,
    light: LightDef,
    current: CurrentDef,
    bubbler: BubblerDef,
    water_change: WaterChangeDef,
    quality: QualityDef,
}

/// A field amount added when a rule fires.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Release {
    pub field: FieldKind,
    pub amount: f32,
}

/// A field condition: the value at the cell must be below (or above) it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Gate {
    pub field: FieldKind,
    pub value: f32,
    pub below: bool,
}

impl Gate {
    /// Whether the condition holds for field value `v`.
    pub fn holds(self, v: f32) -> bool {
        if self.below {
            v < self.value
        } else {
            v > self.value
        }
    }
}

/// Most conditions one rule can have.
pub(crate) const MAX_GATES: usize = 4;

/// Up to [`MAX_GATES`] conditions, all of which must hold.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Gates {
    list: [Option<Gate>; MAX_GATES],
}

impl Gates {
    pub fn iter(&self) -> impl Iterator<Item = Gate> + '_ {
        self.list.iter().flatten().copied()
    }
}

/// A set of material IDs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MaterialSet([u64; 4]);

impl MaterialSet {
    pub fn insert(&mut self, m: MaterialId) {
        self.0[usize::from(m.0 >> 6)] |= 1 << (m.0 & 63);
    }

    pub fn contains(self, m: MaterialId) -> bool {
        self.0[usize::from(m.0 >> 6)] & (1 << (m.0 & 63)) != 0
    }

    pub fn is_empty(self) -> bool {
        self.0 == [0; 4]
    }
}

/// One `[[reaction]]`, keyed by its `a` material.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Reaction {
    pub b: MaterialId,
    pub into: MaterialId,
    pub b_into: Option<MaterialId>,
    /// Chance per tick of *not* reacting: `(1 - p)^(1/20)`.
    pub keep_per_tick: f64,
    pub gates: Gates,
    pub release: Option<Release>,
}

/// Grow directions as `(dx, dy)`, in the order a [`Grow`]'s bits use.
pub(crate) const DIRS: [(isize, isize); 4] = [(0, -1), (0, 1), (-1, 0), (1, 0)];

/// One `[[grow]]`, keyed by its `material`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Grow {
    pub into: MaterialSet,
    pub makes: MaterialId,
    /// Bit `k` set: may grow along `DIRS[k]`.
    pub dirs: u8,
    /// Chance per tick of not growing, and the same with the boost.
    pub keep_per_tick: f64,
    pub keep_boosted: f64,
    /// Largest count the new cell may have.
    pub max: u16,
    pub needs: Gates,
    pub boost: Option<Gate>,
    pub consume: Option<Release>,
    /// The new cell must touch one of these (if any are given).
    pub touching: MaterialSet,
    /// The new cell may touch no other cell of `makes`.
    pub alone: bool,
    pub becomes: Option<MaterialId>,
}

/// One `[[wilt]]`, keyed by its `material`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Wilt {
    pub into: MaterialId,
    pub keep_per_tick: f64,
    /// Wilts unless all of these hold.
    pub unless: Gates,
    /// Wilts if it touches none of these (if any are given).
    pub attached: MaterialSet,
}

/// One `[[spawn]]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Spawn {
    pub material: MaterialId,
    pub on: MaterialSet,
    /// Attempts per tick per column.
    pub per_column_tick: f64,
    pub when: Gates,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Decay {
    pub after_ticks: u16,
    pub into: MaterialId,
    pub release: Option<Release>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Emit {
    pub field: FieldKind,
    /// Per tick.
    pub rate: f32,
    pub up_to: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Buffer {
    pub field: FieldKind,
    pub level: f32,
    /// State units per field unit: the whole `u16` range is the capacity.
    pub units: f32,
    /// Per tick, in field units.
    pub rate: f32,
}

/// Everything one material does on random ticks.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Behaviour {
    pub reactions: Vec<Reaction>,
    pub decay: Option<Decay>,
    pub emit: Option<Emit>,
    pub buffer: Option<Buffer>,
    pub grows: Vec<Grow>,
    pub wilt: Option<Wilt>,
}

impl Behaviour {
    pub(crate) fn is_living(&self) -> bool {
        !self.reactions.is_empty()
            || self.decay.is_some()
            || self.emit.is_some()
            || self.buffer.is_some()
            || !self.grows.is_empty()
            || self.wilt.is_some()
    }
}

/// The water change (SPEC §2.3): a tray action.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterChangeConfig {
    /// Fraction of the water replaced: the nutrient and CO₂ fields lose
    /// this fraction.
    pub fraction: f32,
    pub(crate) siphon: MaterialSet,
}

/// How the water-quality score is worked out (SPEC §2.3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QualityConfig {
    /// Mean nutrient level in the water that costs nothing.
    pub nutrient_ok: f32,
    /// Mean nutrient level that costs the whole nutrient weight.
    pub nutrient_bad: f32,
    /// Score lost to excess nutrients, at most.
    pub nutrient_weight: f32,
    /// Pollutant cells per 1000 water cells that cost the whole pollution
    /// weight.
    pub pollution_bad: f32,
    /// Score lost to pollutants, at most.
    pub pollution_weight: f32,
    /// Plant cells per 1000 water cells that earn the whole plant bonus.
    pub plant_full: f32,
    /// Score added by plants, at most.
    pub plant_bonus: f32,
    /// The material whose cover of the substrate is reported.
    pub algae: Option<MaterialId>,
    pub(crate) pollutants: MaterialSet,
    pub(crate) plants: MaterialSet,
}

/// A diffusing field's constants.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Diffusing {
    /// Diffusion constant in cells² per second.
    pub diffusion: f32,
    /// Fraction lost per second.
    pub decay: f32,
}

/// How light falls through the tank.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightConfig {
    /// Light entering at the top.
    pub surface: f32,
    /// Fraction a field cell of water absorbs.
    pub water: f32,
    /// Fraction a field cell full of opaque cells blocks.
    pub opaque: f32,
}

/// The filter outlet's jet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CurrentConfig {
    /// The outlet material, if the table has it.
    pub material: Option<MaterialId>,
    /// Cells per second at the nozzle.
    pub speed: f32,
    /// Cells over which the jet fades to nothing.
    pub reach: u32,
    /// Field cells per extra field row of width on each side.
    pub spread: u32,
}

/// The bubbler.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BubblerConfig {
    /// The bubbler material, if the table has it.
    pub material: Option<MaterialId>,
    /// What it releases.
    pub emits: Option<MaterialId>,
    /// Releases per second per exposed top cell.
    pub rate: f32,
}

/// The resolved reaction table, processes, field and device settings for
/// one material table.
#[derive(Clone, Debug, PartialEq)]
pub struct Ecology {
    /// Indexed by material ID.
    pub(crate) behaviour: Vec<Behaviour>,
    /// Nutrient field constants.
    pub nutrient: Diffusing,
    /// CO₂ field constants.
    pub co2: Diffusing,
    /// Light settings.
    pub light: LightConfig,
    /// The filter outlet's current.
    pub current: CurrentConfig,
    /// The bubbler.
    pub bubbler: BubblerConfig,
    /// Surface spawns, in file order.
    pub(crate) spawns: Vec<Spawn>,
    /// The water change.
    pub water_change: WaterChangeConfig,
    /// The water-quality score.
    pub quality: QualityConfig,
}

impl Ecology {
    /// The embedded `assets/reactions.toml` and `assets/fields.toml`,
    /// requiring every material they name.
    ///
    /// # Errors
    ///
    /// Returns an error if a file is invalid or names a missing material.
    pub fn builtin(materials: &MaterialTable) -> Result<Self, EcologyError> {
        Self::from_toml(REACTIONS, FIELDS, materials, true)
    }

    /// The embedded files, leaving out entries that name materials
    /// `materials` lacks (for custom test tables).
    ///
    /// # Errors
    ///
    /// Returns an error if an embedded file is invalid (a build defect,
    /// covered by tests).
    pub fn builtin_for(materials: &MaterialTable) -> Result<Self, EcologyError> {
        Self::from_toml(REACTIONS, FIELDS, materials, false)
    }

    /// Parses a reactions file and a fields file against `materials`. With
    /// `strict`, an unknown material name is an error; otherwise its entry
    /// is left out.
    ///
    /// # Errors
    ///
    /// Returns an error if a file doesn't parse, a value is out of range, a
    /// material uses both age and buffer state, or (with `strict`) a name
    /// is unknown.
    #[allow(clippy::too_many_lines)] // one loop per entry kind
    pub fn from_toml(
        reactions: &str,
        fields: &str,
        materials: &MaterialTable,
        strict: bool,
    ) -> Result<Self, EcologyError> {
        let r: ReactionsFile = toml::from_str(reactions).map_err(|source| EcologyError::Parse {
            file: "reactions",
            source,
        })?;
        let f: FieldsFile = toml::from_str(fields).map_err(|source| EcologyError::Parse {
            file: "fields",
            source,
        })?;
        let mut eco = Self {
            behaviour: vec![Behaviour::default(); 256],
            nutrient: diffusing("nutrient", f.nutrient)?,
            co2: diffusing("co2", f.co2)?,
            light: light(f.light)?,
            current: current(&f.current, materials, strict)?,
            bubbler: bubbler(&f.bubbler, materials, strict)?,
            spawns: Vec::new(),
            water_change: water_change(&f.water_change, materials, strict)?,
            quality: quality(&f.quality, materials, strict)?,
        };
        for (name, d) in [("nutrient", eco.nutrient), ("co2", eco.co2)] {
            if !stable(d.diffusion, eco.current.speed) {
                return Err(invalid(
                    &|| format!("field {name}"),
                    "diffusion and current speed together are too fast to step stably",
                ));
            }
        }
        let resolve = |name: &str| lookup(name, materials, strict);
        for d in &r.reaction {
            let what = || format!("reaction {:?} + {:?}", d.a, d.b);
            check_prob(d.p, &what)?;
            let gates = gates(&d.when, &what)?;
            let release = d.release.map(|x| release(x, &what)).transpose()?;
            let b_into = match &d.b_into {
                Some(n) => match resolve(n)? {
                    Some(id) => Some(id),
                    None => continue,
                },
                None => None,
            };
            let (Some(a), Some(b), Some(into)) =
                (resolve(&d.a)?, resolve(&d.b)?, resolve(&d.into)?)
            else {
                continue;
            };
            eco.behaviour[usize::from(a.0)].reactions.push(Reaction {
                b,
                into,
                b_into,
                keep_per_tick: keep_per_tick(d.p),
                gates,
                release,
            });
        }
        for d in &r.grow {
            let what = || format!("grow for {:?}", d.material);
            let Some(rule) = grow_rule(d, materials, strict, &what)? else {
                continue;
            };
            let Some(m) = resolve(&d.material)? else {
                continue;
            };
            eco.behaviour[usize::from(m.0)].grows.push(rule);
        }
        for d in &r.wilt {
            let what = || format!("wilt for {:?}", d.material);
            check_prob(d.p, &what)?;
            let unless = gates(&d.unless, &what)?;
            let attached = set(&d.attached, materials, strict)?;
            if unless.iter().next().is_none() && attached.is_empty() {
                return Err(invalid(&what, "needs an unless or attached condition"));
            }
            let (Some(m), Some(into)) = (resolve(&d.material)?, resolve(&d.into)?) else {
                continue;
            };
            let slot = &mut eco.behaviour[usize::from(m.0)];
            if slot.wilt.is_some() {
                return Err(invalid(&what, "a material has at most one wilt"));
            }
            slot.wilt = Some(Wilt {
                into,
                keep_per_tick: keep_per_tick(d.p),
                unless,
                attached,
            });
        }
        for d in &r.spawn {
            let what = || format!("spawn of {:?}", d.material);
            // Each attempt scans a column, so keep it to about one column
            // scan per 64 columns per tick.
            if !(d.rate.is_finite() && d.rate > 0.0 && d.rate <= MAX_SPAWN_RATE) {
                return Err(invalid(&what, "rate must be within (0, 20] per second"));
            }
            let when = gates(&d.when, &what)?;
            let on = set(&d.on, materials, strict)?;
            let Some(material) = resolve(&d.material)? else {
                continue;
            };
            eco.spawns.push(Spawn {
                material,
                on,
                per_column_tick: f64::from(d.rate)
                    / f64::from(SPAWN_COLUMNS)
                    / f64::from(TICKS_PER_SECOND),
                when,
            });
        }
        for d in &r.decay {
            let what = || format!("decay for {:?}", d.material);
            if !(d.after.is_finite() && (0.05..=MAX_DECAY_TIME).contains(&d.after)) {
                return Err(invalid(&what, "after must be within 0.05..=3000 seconds"));
            }
            let release = d.release.map(|x| release(x, &what)).transpose()?;
            let (Some(m), Some(into)) = (resolve(&d.material)?, resolve(&d.into)?) else {
                continue;
            };
            let slot = &mut eco.behaviour[usize::from(m.0)];
            if slot.decay.is_some() {
                return Err(invalid(&what, "a material has at most one decay"));
            }
            slot.decay = Some(Decay {
                after_ticks: seconds_to_ticks(d.after),
                into,
                release,
            });
        }
        for d in &r.emit {
            let what = || format!("emit for {:?}", d.material);
            if !(d.rate.is_finite() && d.rate > 0.0) {
                return Err(invalid(&what, "rate must be finite and > 0"));
            }
            let up_to = d.up_to.unwrap_or(f32::INFINITY);
            if up_to.is_nan() || up_to <= 0.0 {
                return Err(invalid(&what, "up_to must be > 0"));
            }
            let Some(m) = resolve(&d.material)? else {
                continue;
            };
            let slot = &mut eco.behaviour[usize::from(m.0)];
            if slot.emit.is_some() {
                return Err(invalid(&what, "a material has at most one emit"));
            }
            slot.emit = Some(Emit {
                field: d.field,
                rate: d.rate / per_second(),
                up_to,
            });
        }
        for d in &r.buffer {
            let what = || format!("buffer for {:?}", d.material);
            let ok = |v: f32| v.is_finite() && v > 0.0;
            if !(ok(d.level) && ok(d.rate) && ok(d.capacity)) {
                return Err(invalid(&what, "level, capacity and rate must be > 0"));
            }
            let Some(m) = resolve(&d.material)? else {
                continue;
            };
            let slot = &mut eco.behaviour[usize::from(m.0)];
            if slot.buffer.is_some() {
                return Err(invalid(&what, "a material has at most one buffer"));
            }
            slot.buffer = Some(Buffer {
                field: d.field,
                level: d.level,
                units: f32::from(u16::MAX) / d.capacity,
                rate: d.rate / per_second(),
            });
        }
        // Materials whose state is a grow count: those that grow, the
        // cells they make and what they become.
        let mut counted = MaterialSet::default();
        for (i, b) in eco.behaviour.iter().enumerate() {
            if !b.grows.is_empty() {
                #[allow(clippy::cast_possible_truncation)] // 256 slots
                counted.insert(MaterialId(i as u8));
            }
            for g in &b.grows {
                counted.insert(g.makes);
                if let Some(m) = g.becomes {
                    counted.insert(m);
                }
            }
        }
        for m in materials.iter() {
            let b = &eco.behaviour[usize::from(m.id.0)];
            let aged = b.decay.is_some() || b.emit.is_some();
            let grows = counted.contains(m.id);
            let what = || format!("material {:?}", m.name);
            if usize::from(aged) + usize::from(b.buffer.is_some()) + usize::from(grows) > 1 {
                return Err(invalid(
                    &what,
                    "can only one of age (decay, emit), buffer or grow",
                ));
            }
            // Their state is the float timer or the body id (D17).
            if (aged || b.buffer.is_some() || grows)
                && matches!(m.class, MovementClass::Floating | MovementClass::Rigid)
            {
                return Err(invalid(
                    &what,
                    "floating and rigid materials can't decay, emit, buffer or grow",
                ));
            }
        }
        Ok(eco)
    }
}

fn per_second() -> f32 {
    #[allow(clippy::cast_precision_loss)] // small constant
    let t = TICKS_PER_SECOND as f32;
    t
}

/// Columns a `[[spawn]]` rate is given per: one chunk's width.
#[allow(clippy::cast_precision_loss)] // small constant
const SPAWN_COLUMNS: f32 = crate::world::CHUNK_SIZE as f32;

/// Largest `[[spawn]]` rate, attempts per second per 64 columns.
const MAX_SPAWN_RATE: f32 = 20.0;

/// Chance per tick of not firing, for a chance `p` per second:
/// `(1 - p)^(1/20)`, by [`nth_root`] so it is the same on every target.
fn keep_per_tick(p: f32) -> f64 {
    nth_root(1.0 - f64::from(p), TICKS_PER_SECOND)
}

/// `a^(1/n)` for `a` in `[0, 1]`, by Newton's method from 1 with plain
/// arithmetic (no `powf`, whose last bit can differ between platforms'
/// maths libraries), so it gives the same bits on every IEEE target.
pub(crate) fn nth_root(a: f64, n: u32) -> f64 {
    if a <= 0.0 {
        return 0.0;
    }
    if a >= 1.0 || n <= 1 {
        return a.min(1.0);
    }
    let nf = f64::from(n);
    let k = u16::try_from(n - 1).unwrap_or(u16::MAX);
    // From above the root the iterates fall monotonically; stop when they
    // no longer do.
    let mut y = 1.0_f64;
    for _ in 0..2000 {
        let next = y - (powu(y, k) * y - a) / (nf * powu(y, k));
        if next >= y {
            break;
        }
        y = next;
    }
    y
}

/// `base^k` by squaring: plain multiplications, so it is the same on every
/// IEEE platform (unlike `powi`).
pub(crate) fn powu(base: f64, mut k: u16) -> f64 {
    let (mut acc, mut b) = (1.0, base);
    while k > 0 {
        if k & 1 == 1 {
            acc *= b;
        }
        b *= b;
        k >>= 1;
    }
    acc
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // validated range
fn seconds_to_ticks(s: f32) -> u16 {
    (f64::from(s) * f64::from(TICKS_PER_SECOND))
        .round()
        .clamp(1.0, f64::from(u16::MAX)) as u16
}

fn invalid(what: &dyn Fn() -> String, reason: &'static str) -> EcologyError {
    EcologyError::Invalid {
        what: what(),
        reason,
    }
}

fn check_prob(p: f32, what: &dyn Fn() -> String) -> Result<(), EcologyError> {
    if p.is_finite() && p > 0.0 && p <= 1.0 {
        Ok(())
    } else {
        Err(invalid(what, "p must be within (0, 1]"))
    }
}

fn gates(list: &WhenList, what: &dyn Fn() -> String) -> Result<Gates, EcologyError> {
    let list = list.as_slice();
    if list.len() > MAX_GATES {
        return Err(invalid(what, "at most 4 conditions"));
    }
    let mut out = Gates::default();
    for (slot, w) in out.list.iter_mut().zip(list) {
        *slot = Some(gate(*w, what)?);
    }
    Ok(out)
}

/// Resolves a list of names; unknown names are an error with `strict`,
/// otherwise left out.
fn set(
    names: &[String],
    materials: &MaterialTable,
    strict: bool,
) -> Result<MaterialSet, EcologyError> {
    let mut out = MaterialSet::default();
    for n in names {
        if let Some(id) = lookup(n, materials, strict)? {
            out.insert(id);
        }
    }
    Ok(out)
}

/// A `[[grow]]` entry, or `None` if it names a missing material (lenient).
fn grow_rule(
    d: &GrowDef,
    materials: &MaterialTable,
    strict: bool,
    what: &dyn Fn() -> String,
) -> Result<Option<Grow>, EcologyError> {
    check_prob(d.p, what)?;
    let needs = gates(&d.needs, what)?;
    let consume = d.consume.map(|x| release(x, what)).transpose()?;
    let (boost, factor) = match d.boost {
        None => (None, 1.0),
        Some(b) => {
            if !(b.factor.is_finite() && b.factor >= 1.0 && d.p * b.factor <= 1.0) {
                return Err(invalid(what, "boost factor must be >= 1 and keep p <= 1"));
            }
            let when = WhenDef {
                field: b.field,
                below: None,
                above: Some(b.above),
            };
            (Some(gate(when, what)?), b.factor)
        }
    };
    if d.dirs.is_empty() || d.into.is_empty() {
        return Err(invalid(what, "needs at least one dir and into material"));
    }
    if d.max == Some(0) {
        return Err(invalid(what, "max must be at least 1"));
    }
    let mut dirs = 0u8;
    for dir in &d.dirs {
        dirs |= 1 << (*dir as u8);
    }
    let into = set(&d.into, materials, strict)?;
    let touching = set(&d.touching, materials, strict)?;
    let becomes = match &d.becomes {
        Some(n) => match lookup(n, materials, strict)? {
            Some(id) => Some(id),
            None => return Ok(None),
        },
        None => None,
    };
    let Some(makes) = lookup(&d.makes, materials, strict)? else {
        return Ok(None);
    };
    if into.is_empty() {
        return Ok(None);
    }
    Ok(Some(Grow {
        into,
        makes,
        dirs,
        keep_per_tick: keep_per_tick(d.p),
        keep_boosted: keep_per_tick(d.p * factor),
        max: d.max.unwrap_or(u16::MAX),
        needs,
        boost,
        consume,
        touching,
        alone: d.alone,
        becomes,
    }))
}

fn water_change(
    d: &WaterChangeDef,
    materials: &MaterialTable,
    strict: bool,
) -> Result<WaterChangeConfig, EcologyError> {
    if !(d.fraction.is_finite() && (0.0..=1.0).contains(&d.fraction)) {
        return Err(invalid(
            &|| "water_change".to_owned(),
            "fraction must be within 0..=1",
        ));
    }
    Ok(WaterChangeConfig {
        fraction: d.fraction,
        siphon: set(&d.siphon, materials, strict)?,
    })
}

fn quality(
    d: &QualityDef,
    materials: &MaterialTable,
    strict: bool,
) -> Result<QualityConfig, EcologyError> {
    let what = || "quality".to_owned();
    let pos = |v: f32| v.is_finite() && v > 0.0;
    let unit = |v: f32| v.is_finite() && (0.0..=1.0).contains(&v);
    if !(d.nutrient_ok.is_finite() && d.nutrient_ok >= 0.0 && d.nutrient_bad > d.nutrient_ok) {
        return Err(invalid(&what, "needs 0 <= nutrient_ok < nutrient_bad"));
    }
    if !(pos(d.pollution_bad) && pos(d.plant_full)) {
        return Err(invalid(&what, "pollution_bad and plant_full must be > 0"));
    }
    if !(unit(d.nutrient_weight) && unit(d.pollution_weight) && unit(d.plant_bonus)) {
        return Err(invalid(&what, "weights and bonus must be within 0..=1"));
    }
    Ok(QualityConfig {
        nutrient_ok: d.nutrient_ok,
        nutrient_bad: d.nutrient_bad,
        nutrient_weight: d.nutrient_weight,
        pollution_bad: d.pollution_bad,
        pollution_weight: d.pollution_weight,
        plant_full: d.plant_full,
        plant_bonus: d.plant_bonus,
        algae: lookup(&d.algae, materials, strict)?,
        pollutants: set(&d.pollutants, materials, strict)?,
        plants: set(&d.plants, materials, strict)?,
    })
}

fn gate(w: WhenDef, what: &dyn Fn() -> String) -> Result<Gate, EcologyError> {
    let (value, below) = match (w.below, w.above) {
        (Some(v), None) => (v, true),
        (None, Some(v)) => (v, false),
        _ => return Err(invalid(what, "when needs exactly one of below and above")),
    };
    if w.field == FieldKind::Light && !below && value > 1.0 {
        return Err(invalid(what, "light never exceeds 1"));
    }
    if !value.is_finite() || value < 0.0 {
        return Err(invalid(what, "when value must be finite and >= 0"));
    }
    Ok(Gate {
        field: w.field,
        value,
        below,
    })
}

fn release(r: ReleaseDef, what: &dyn Fn() -> String) -> Result<Release, EcologyError> {
    if r.field == FieldKind::Light {
        return Err(invalid(what, "light can't be released into"));
    }
    if !(r.amount.is_finite() && r.amount > 0.0) {
        return Err(invalid(what, "release amount must be finite and > 0"));
    }
    Ok(Release {
        field: r.field,
        amount: r.amount,
    })
}

fn diffusing(name: &'static str, d: DiffusingDef) -> Result<Diffusing, EcologyError> {
    let what = || format!("field {name}");
    if !(d.diffusion.is_finite() && (0.0..=MAX_DIFFUSION).contains(&d.diffusion)) {
        return Err(invalid(&what, "diffusion must be within 0..=80 cells²/s"));
    }
    if !(d.decay.is_finite() && (0.0..=1.0).contains(&d.decay)) {
        return Err(invalid(&what, "decay must be within 0..=1 per second"));
    }
    Ok(Diffusing {
        diffusion: d.diffusion,
        decay: d.decay,
    })
}

/// Whether one field step keeps every cell's own weight non-negative: what
/// diffusion (`4·D·dt/h²`) and upwind advection (up to `2·v·dt/h`) take out
/// of a cell must not exceed what it holds.
fn stable(diffusion: f32, speed: f32) -> bool {
    #[allow(clippy::cast_precision_loss)] // FIELD_SCALE is small
    let h = FIELD_SCALE as f32;
    4.0 * diffusion * FIELD_DT / (h * h) + 2.0 * speed * FIELD_DT / h <= 1.0
}

fn light(d: LightDef) -> Result<LightConfig, EcologyError> {
    let unit = |v: f32| v.is_finite() && (0.0..=1.0).contains(&v);
    if !(unit(d.surface) && unit(d.water) && unit(d.opaque)) {
        return Err(invalid(
            &|| "field light".to_owned(),
            "values must be within 0..=1",
        ));
    }
    Ok(LightConfig {
        surface: d.surface,
        water: d.water,
        opaque: d.opaque,
    })
}

fn lookup(
    name: &str,
    materials: &MaterialTable,
    strict: bool,
) -> Result<Option<MaterialId>, EcologyError> {
    match materials.id(name) {
        None if strict => Err(EcologyError::UnknownMaterial(name.to_owned())),
        id => Ok(id),
    }
}

fn current(
    d: &CurrentDef,
    materials: &MaterialTable,
    strict: bool,
) -> Result<CurrentConfig, EcologyError> {
    let what = || "current".to_owned();
    // At most one swap per cell per tick: 20 cells per second.
    if !(d.speed.is_finite() && d.speed > 0.0 && d.speed <= per_second()) {
        return Err(invalid(&what, "speed must be within (0, 20] cells/s"));
    }
    if d.reach == 0 || d.spread == 0 {
        return Err(invalid(&what, "reach and spread must be > 0"));
    }
    Ok(CurrentConfig {
        material: lookup(&d.material, materials, strict)?,
        speed: d.speed,
        reach: d.reach,
        spread: d.spread,
    })
}

fn bubbler(
    d: &BubblerDef,
    materials: &MaterialTable,
    strict: bool,
) -> Result<BubblerConfig, EcologyError> {
    if !(d.rate.is_finite() && d.rate > 0.0 && d.rate <= per_second()) {
        return Err(invalid(
            &|| "bubbler".to_owned(),
            "rate must be within (0, 20] per second",
        ));
    }
    Ok(BubblerConfig {
        material: lookup(&d.material, materials, strict)?,
        emits: lookup(&d.emits, materials, strict)?,
        rate: d.rate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_files_resolve_against_the_builtin_materials() {
        let t = MaterialTable::builtin().unwrap();
        let eco = Ecology::builtin(&t).unwrap();
        let b = |n: &str| &eco.behaviour[usize::from(t.id(n).unwrap().0)];
        assert!(b("detritus").decay.is_some());
        assert!(b("aquasoil").buffer.is_some());
        assert!(b("root_tab").emit.is_some() && b("root_tab").decay.is_some());
        assert_eq!(b("liquid_fertiliser").reactions.len(), 1);
        assert!(!b("sand").is_living());
        assert_eq!(eco.current.material, t.id("filter_outlet"));
        assert_eq!(eco.bubbler.emits, t.id("air_bubble"));
        // T6: plants, moss and algae.
        assert_eq!(b("stem").grows.len(), 2);
        assert!(b("leaf").wilt.is_some() && b("seed").grows[0].becomes == t.id("stem"));
        assert!(b("dead_plant").decay.is_some());
        assert_eq!(eco.spawns.len(), 1);
        assert_eq!(eco.spawns[0].material, t.id("algae").unwrap());
        assert_eq!(eco.quality.algae, t.id("algae"));
        assert!(eco.water_change.siphon.contains(t.id("detritus").unwrap()));
        assert!(!eco.water_change.siphon.contains(t.id("sand").unwrap()));
    }

    #[test]
    fn nth_root_matches_powf_and_is_exact_at_the_ends() {
        for a in [0.999_f64, 0.75, 0.5, 0.1, 1e-3, 1e-9] {
            let want = a.powf(1.0 / 20.0);
            let got = nth_root(a, 20);
            assert!((got / want - 1.0).abs() < 1e-14, "{a}: {got} vs {want}");
        }
        assert!(nth_root(0.0, 20).abs() < f64::MIN_POSITIVE);
        assert!((nth_root(1.0, 20) - 1.0).abs() < f64::EPSILON);
        assert!((nth_root(0.3, 1) - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    fn when_takes_one_condition_or_a_list() {
        let t = MaterialTable::builtin().unwrap();
        let eco = Ecology::from_toml(
            "[[reaction]]\na = \"sand\"\nb = \"water\"\ninto = \"sand\"\np = 0.5\n\
             when = [{ field = \"co2\", above = 1.0 }, { field = \"light\", below = 0.5 }]",
            FIELDS,
            &t,
            true,
        )
        .unwrap();
        let r = eco.behaviour[usize::from(t.id("sand").unwrap().0)].reactions[0];
        assert_eq!(r.gates.iter().count(), 2);
    }

    #[test]
    fn rejects_bad_t6_entries() {
        let t = MaterialTable::builtin().unwrap();
        let bad = |reactions: &str| {
            matches!(
                Ecology::from_toml(reactions, FIELDS, &t, true),
                Err(EcologyError::Invalid { .. })
            )
        };
        let grow = |extra: &str| {
            format!(
                "[[grow]]\nmaterial = \"moss\"\ninto = [\"water\"]\nmakes = \"moss\"\np = 0.5\n{extra}"
            )
        };
        assert!(!bad(&grow("dirs = [\"up\"]")));
        assert!(bad(&grow("dirs = []")));
        assert!(bad(&grow("dirs = [\"up\"]\nmax = 0")));
        // A boost may not push p past 1.
        assert!(bad(&grow(
            "dirs = [\"up\"]\nboost = { field = \"co2\", above = 0.1, factor = 3.0 }"
        )));
        // Growing cells' state is their count, so they can't also age.
        assert!(bad(
            "[[grow]]\nmaterial = \"detritus\"\ninto = [\"water\"]\nmakes = \"detritus\"\n\
             dirs = [\"up\"]\np = 0.5\n[[decay]]\nmaterial = \"detritus\"\nafter = 5.0\ninto = \"water\""
        ));
        assert!(bad(
            "[[grow]]\nmaterial = \"moss\"\ninto = [\"water\"]\nmakes = \"driftwood\"\ndirs = [\"up\"]\np = 0.5"
        ));
        assert!(bad(
            "[[wilt]]\nmaterial = \"moss\"\ninto = \"water\"\np = 0.5"
        ));
        assert!(bad(
            "[[spawn]]\nmaterial = \"algae\"\non = [\"sand\"]\nrate = 0.0"
        ));
        assert!(matches!(
            Ecology::from_toml(
                "[[spawn]]\nmaterial = \"algae\"\non = [\"lava\"]\nrate = 1.0",
                FIELDS,
                &t,
                true
            ),
            Err(EcologyError::UnknownMaterial(_))
        ));
    }

    #[test]
    fn rejects_bad_entries() {
        let t = MaterialTable::builtin().unwrap();
        let bad = |reactions: &str| Ecology::from_toml(reactions, FIELDS, &t, true);
        assert!(matches!(
            bad("[[reaction]]\na = \"sand\"\nb = \"water\"\ninto = \"rock\"\np = 1.5"),
            Err(EcologyError::Invalid { .. })
        ));
        assert!(matches!(
            bad("[[decay]]\nmaterial = \"lava\"\nafter = 1.0\ninto = \"rock\""),
            Err(EcologyError::UnknownMaterial(_))
        ));
        assert!(matches!(
            bad("[[decay]]\nmaterial = \"flake_food\"\nafter = 1.0\ninto = \"rock\""),
            Err(EcologyError::Invalid { .. })
        ));
        assert!(matches!(
            bad("[[decay]]\nmaterial = \"driftwood\"\nafter = 1.0\ninto = \"sand\""),
            Err(EcologyError::Invalid { .. })
        ));
        assert!(matches!(
            bad(
                "[[reaction]]\na = \"sand\"\nb = \"water\"\ninto = \"rock\"\np = 0.5\nwhen = { field = \"co2\" }"
            ),
            Err(EcologyError::Invalid { .. })
        ));
        assert!(matches!(bad("nope = 1"), Err(EcologyError::Parse { .. })));
        // Fast diffusion with a fast current would step unstably.
        let fast = FIELDS
            .replace("diffusion = 16.0", "diffusion = 70.0")
            .replace("speed = 10.0", "speed = 20.0");
        assert!(matches!(
            Ecology::from_toml(REACTIONS, &fast, &t, true),
            Err(EcologyError::Invalid { .. })
        ));
        // Lenient loading drops entries for missing materials.
        let eco = Ecology::from_toml(
            "[[decay]]\nmaterial = \"lava\"\nafter = 1.0\ninto = \"rock\"",
            FIELDS,
            &t,
            false,
        )
        .unwrap();
        assert!(eco.behaviour.iter().all(|b| !b.is_living()));
    }
}
