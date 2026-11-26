//! Algae varieties (D28, #369): the `[[algae.variety]]` entries of
//! `assets/fields.toml`. Each variety says where it can grow (its hosts),
//! the conditions on the existing fields that let it grow or make it die
//! back, how fast it changes, how hard grazers find it and how the shader
//! draws it.

use serde::Deserialize;

use super::{EcologyError, invalid};

/// Variety ids are the coat byte's high nibble. 0 is bare.
pub const MAX_VARIETY: u8 = 15;

/// A variety grows on sand, gravel, clown vomit gravel and aquasoil.
pub const HOST_SUBSTRATE: u8 = 1;
/// A variety grows on rock and driftwood.
pub const HOST_HARDSCAPE: u8 = 2;
/// A variety grows on the front pane.
pub const HOST_FRONT_GLASS: u8 = 4;
/// A variety grows on the back wall and both side walls.
pub const HOST_REAR_GLASS: u8 = 8;

/// How the shader draws a variety (`tank.wgsl` `ALGAE_*`).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum AlgaeStyle {
    /// Patchy translucent film, with olive specks.
    Film = 0,
    /// A smooth, slimy sheen with soft streaks.
    Slime = 1,
    /// Crisp small round spots.
    Spots = 2,
    /// A fine dusty speckle.
    Dust = 3,
}

/// A variety's appearance, for the renderer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AlgaeLook {
    /// The variety id: the coat byte's high nibble.
    pub id: u8,
    /// Which look the shader draws.
    pub style: AlgaeStyle,
    /// The shade for thin or shadowed coat, sRGB-encoded.
    pub dark: [f32; 3],
    /// The shade for thick or lit coat, sRGB-encoded.
    pub light: [f32; 3],
    /// How opaque the thickest coat is, 0..=1.
    pub strength: f32,
}

/// An inclusive upper and exclusive lower limit on a condition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Range {
    min: f32,
    max: f32,
}

impl Range {
    /// Whether `value` is above the lower and at most the upper limit.
    pub fn contains(self, value: f32) -> bool {
        value > self.min && value <= self.max
    }
}

/// The local conditions a variety reacts to.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Conditions {
    pub nutrient: f32,
    pub light: f32,
    pub co2: f32,
    /// The filters' biological maturity, 0 (none or new) to 1 (cycled).
    pub maturity: f32,
}

/// What a variety does in some conditions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// It establishes, creeps and thickens.
    Grow,
    /// Too dark to grow, but nothing hurts it: it stays as it is.
    Dormant,
    /// Not its conditions: existing coat recedes.
    Fade,
}

/// One algae variety, resolved.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Variety {
    pub id: u8,
    pub name: String,
    /// The higher priority wins where several varieties would grow.
    pub priority: u8,
    /// `HOST_*` bits.
    pub hosts: u8,
    pub nutrient: Range,
    pub light: Range,
    pub co2: Range,
    pub maturity: Range,
    pub seed_rate: f32,
    pub creep_rate: f32,
    pub growth_rate: f32,
    pub die_back_rate: f32,
    /// How much a grazer without a taste for it resists, 0..=100 percent.
    pub resistance: u8,
    pub look: AlgaeLook,
}

impl Variety {
    pub fn verdict(&self, c: Conditions) -> Verdict {
        if !self.nutrient.contains(c.nutrient)
            || !self.co2.contains(c.co2)
            || !self.maturity.contains(c.maturity)
            || c.light > self.light.max
        {
            Verdict::Fade
        } else if c.light > self.light.min {
            Verdict::Grow
        } else {
            Verdict::Dormant
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RangeDef {
    min: Option<f32>,
    max: Option<f32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LookDef {
    style: AlgaeStyle,
    dark: String,
    light: String,
    strength: f32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct VarietyDef {
    id: u8,
    name: String,
    #[serde(default)]
    priority: u8,
    hosts: Vec<String>,
    #[serde(default)]
    nutrient: RangeDef,
    #[serde(default)]
    light: RangeDef,
    #[serde(default)]
    co2: RangeDef,
    #[serde(default)]
    maturity: RangeDef,
    seed_rate: Option<f32>,
    creep_rate: Option<f32>,
    growth_rate: Option<f32>,
    die_back_rate: Option<f32>,
    #[serde(default)]
    resistance: u8,
    look: LookDef,
}

/// The `[algae]` defaults a variety inherits.
#[derive(Clone, Copy)]
pub(super) struct Defaults {
    pub nutrient: f32,
    pub light: f32,
    pub seed_rate: f32,
    pub creep_rate: f32,
    pub growth_rate: f32,
    pub die_back_rate: f32,
}

fn range(d: RangeDef, min: f32, what: &dyn Fn() -> String) -> Result<Range, EcologyError> {
    let (min, max) = (d.min.unwrap_or(min), d.max.unwrap_or(f32::INFINITY));
    if !(min.is_finite() || min == f32::NEG_INFINITY) || max.is_nan() || min >= max {
        return Err(invalid(what, "a range needs finite limits with min < max"));
    }
    Ok(Range { min, max })
}

fn colour(hex: &str, what: &dyn Fn() -> String) -> Result<[f32; 3], EcologyError> {
    let bad = || invalid(what, "colours are #rrggbb");
    let digits = hex
        .strip_prefix('#')
        .filter(|d| d.len() == 6)
        .ok_or_else(bad)?;
    let mut out = [0.0; 3];
    for (channel, pair) in out.iter_mut().zip(digits.as_bytes().chunks(2)) {
        let pair = std::str::from_utf8(pair).map_err(|_| bad())?;
        *channel = f32::from(u8::from_str_radix(pair, 16).map_err(|_| bad())?) / 255.0;
    }
    Ok(out)
}

pub(super) fn resolve(
    defs: &[VarietyDef],
    defaults: Defaults,
) -> Result<Vec<Variety>, EcologyError> {
    if defs.is_empty() {
        return Err(invalid(
            &|| "algae".to_owned(),
            "at least one [[algae.variety]] is needed",
        ));
    }
    let mut out: Vec<Variety> = Vec::with_capacity(defs.len());
    for d in defs {
        let what = || format!("algae variety {:?}", d.name);
        if !(1..=MAX_VARIETY).contains(&d.id) {
            return Err(invalid(&what, "id must be within 1..=15"));
        }
        if out.iter().any(|v| v.id == d.id || v.name == d.name) {
            return Err(invalid(&what, "ids and names must be unique"));
        }
        if d.name.is_empty()
            || !d
                .name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            return Err(invalid(&what, "names are lowercase snake_case"));
        }
        let mut hosts = 0;
        for host in &d.hosts {
            hosts |= match host.as_str() {
                "substrate" => HOST_SUBSTRATE,
                "hardscape" => HOST_HARDSCAPE,
                "front_glass" => HOST_FRONT_GLASS,
                "rear_glass" => HOST_REAR_GLASS,
                _ => {
                    return Err(invalid(
                        &what,
                        "hosts are substrate, hardscape, front_glass or rear_glass",
                    ));
                }
            };
        }
        if hosts == 0 {
            return Err(invalid(&what, "a variety needs a host"));
        }
        let rates = [
            d.seed_rate.unwrap_or(defaults.seed_rate),
            d.creep_rate.unwrap_or(defaults.creep_rate),
            d.growth_rate.unwrap_or(defaults.growth_rate),
            d.die_back_rate.unwrap_or(defaults.die_back_rate),
        ];
        if !rates.into_iter().all(|v| v.is_finite() && v > 0.0) {
            return Err(invalid(&what, "rates must be finite and positive"));
        }
        if d.resistance > 100 {
            return Err(invalid(&what, "resistance is a percentage, 0..=100"));
        }
        let strength = d.look.strength;
        if !(strength.is_finite() && strength > 0.0 && strength <= 1.0) {
            return Err(invalid(&what, "look strength must be within (0, 1]"));
        }
        let light = range(d.light, defaults.light, &what)?;
        if light.max < 0.0 || light.min < 0.0 || light.min > 1.0 {
            return Err(invalid(&what, "light limits are within 0..=1"));
        }
        out.push(Variety {
            id: d.id,
            name: d.name.clone(),
            priority: d.priority,
            hosts,
            nutrient: range(d.nutrient, defaults.nutrient, &what)?,
            light,
            co2: range(d.co2, f32::NEG_INFINITY, &what)?,
            maturity: range(d.maturity, f32::NEG_INFINITY, &what)?,
            seed_rate: rates[0],
            creep_rate: rates[1],
            growth_rate: rates[2],
            die_back_rate: rates[3],
            resistance: d.resistance,
            look: AlgaeLook {
                id: d.id,
                style: d.look.style,
                dark: colour(&d.look.dark, &what)?,
                light: colour(&d.look.light, &what)?,
                strength,
            },
        });
    }
    Ok(out)
}
