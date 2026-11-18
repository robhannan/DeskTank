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
//!   towards `level`.
//!
//! `assets/fields.toml` holds the field constants, the filter outlet's
//! current and the bubbler. Material names are resolved against a
//! [`MaterialTable`]; [`Ecology::builtin_for`] drops entries naming
//! materials a custom table lacks, while [`Ecology::builtin`] requires all
//! of them (a test guards the embedded files).
//!
//! A cell's `state` holds its age in ticks for decay and emit, or the
//! amount drawn from a buffer, so a material may use only one of the two,
//! and floating materials (whose state is their float timer) and rigid ones
//! (whose state is their body id, D17) neither. Reactions may involve rigid
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReactionDef {
    a: String,
    b: String,
    into: String,
    b_into: Option<String>,
    p: f32,
    when: Option<WhenDef>,
    release: Option<ReleaseDef>,
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
struct FieldsFile {
    nutrient: DiffusingDef,
    co2: DiffusingDef,
    light: LightDef,
    current: CurrentDef,
    bubbler: BubblerDef,
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

/// One `[[reaction]]`, keyed by its `a` material.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Reaction {
    pub b: MaterialId,
    pub into: MaterialId,
    pub b_into: Option<MaterialId>,
    /// Chance per tick of *not* reacting: `(1 - p)^(1/20)`.
    pub keep_per_tick: f64,
    pub gate: Option<Gate>,
    pub release: Option<Release>,
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
}

impl Behaviour {
    pub(crate) fn is_living(&self) -> bool {
        !self.reactions.is_empty()
            || self.decay.is_some()
            || self.emit.is_some()
            || self.buffer.is_some()
    }
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
            let gate = d.when.map(|w| gate(w, &what)).transpose()?;
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
                keep_per_tick: (1.0 - f64::from(d.p)).powf(1.0 / f64::from(TICKS_PER_SECOND)),
                gate,
                release,
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
        for m in materials.iter() {
            let b = &eco.behaviour[usize::from(m.id.0)];
            let aged = b.decay.is_some() || b.emit.is_some();
            let what = || format!("material {:?}", m.name);
            if aged && b.buffer.is_some() {
                return Err(invalid(&what, "can't both age (decay, emit) and buffer"));
            }
            // Their state is the float timer or the body id (D17).
            if (aged || b.buffer.is_some())
                && matches!(m.class, MovementClass::Floating | MovementClass::Rigid)
            {
                return Err(invalid(
                    &what,
                    "floating and rigid materials can't decay, emit or buffer",
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
