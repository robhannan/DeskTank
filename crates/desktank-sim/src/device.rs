//! Prefab devices (`docs/spec/devices.md`, D27): the device kinds, loaded from
//! `assets/devices/`, and the device instances the world keeps.
//!
//! A device is an instance in the world's device table (id → kind, anchor,
//! facing), like rigid bodies and plants. Its footprint cells are solid
//! cells of the `device` material holding its id in their state, so powders
//! and creatures collide with it in every depth band, and scooping any of
//! them removes the whole device. It is drawn as a hand-drawn sprite; the
//! footprint is every cell its art covers.
//!
//! DV1 has one kind, the internal filter: suction-cupped to the side glass
//! at any height under the water line, its nozzle drives the horizontal jet
//! away from the glass. Filtering (intakes, media load, maturity) comes in
//! DV3.

use serde::Deserialize;

use crate::species::{Sprite, SpriteDef, TEXELS_PER_CELL, sprite};

/// Most devices a tank holds.
pub const MAX_DEVICES: usize = 16;

/// Minimum flow fraction at full media clog (D27, DV3: ~30% so an unrinsed
/// screensaver or lock-screen filter never kills the tank).
pub const CLOG_FLOW_FLOOR: f32 = 0.3;

/// Discrete media-load steps for flow scaling (every 5% clog) so the 2D
/// current field rebuilds at most 20 times from clean to full clog while
/// staying bit-for-bit identical across save/load.
const FLOW_STEPS: f32 = 20.0;

/// How a filter's media is cleaned (D27, DV3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RinseKind {
    /// Gentle rinse in tank water: clears media load and keeps bacteria maturity.
    Gentle,
    /// Hard rinse (e.g. under tap water): clears media load and sets maturity back sharply.
    Hard,
    /// Replace media: clears media load and resets maturity to zero.
    Replace,
}

/// A device's id in the world's device table (1..=65535; 0 is no device).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceId(pub u16);

/// What a device is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DeviceKind {
    /// An internal power filter on the side glass (DV1).
    InternalFilter,
}

impl DeviceKind {
    /// Every kind, in palette order.
    pub const ALL: [Self; 1] = [Self::InternalFilter];

    /// Its name in saves, scripts and `assets/devices/`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::InternalFilter => "internal_filter",
        }
    }

    /// The kind named `name`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }

    fn index(self) -> usize {
        match self {
            Self::InternalFilter => 0,
        }
    }
}

/// Where a kind of device may go.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mount {
    /// Against a side glass (the nearer one, or the other when flipped),
    /// at any height wholly under the water line.
    SideGlass,
}

impl Mount {
    /// Whether devices with this mount can be flipped between opposite sides
    /// (left and right glass).
    #[must_use]
    pub const fn flippable(self) -> bool {
        match self {
            Self::SideGlass => true,
        }
    }
}

/// One kind's art and placement rules.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceArt {
    /// The kind.
    pub kind: DeviceKind,
    /// Display label.
    pub label: String,
    /// Where it may go.
    pub mount: Mount,
    /// Size in cells.
    pub size: (usize, usize),
    /// Footprint cells, row-major over `size`, facing right: the cells its
    /// art covers.
    pub footprint: Vec<bool>,
    /// Footprint cells in all.
    pub cells: usize,
    /// The nozzle tip in cells from the top-left, facing right.
    pub nozzle: (usize, usize),
    /// Unit direction the nozzle jets, facing right (`+x` right, `+y` down).
    pub nozzle_dir: [f32; 2],
    /// Intake cells from the top-left, facing right: water and loose
    /// particles are drawn towards them.
    pub intakes: Vec<(usize, usize)>,
    /// Whether this kind is intrinsically safe for fry and small shrimp
    /// without an intake guard (e.g. sponge filter).
    pub fry_safe: bool,
    /// The art, facing right (mounted on the left glass).
    pub sprite: Sprite,
}

/// Why the device kinds couldn't be loaded.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DeviceError {
    /// The TOML didn't parse.
    #[error("{file}: {message}")]
    Parse {
        /// The kind's file name.
        file: String,
        /// What went wrong.
        message: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtDef {
    name: String,
    label: String,
    mount: Mount,
    nozzle: [usize; 2],
    #[serde(default = "default_nozzle_dir")]
    nozzle_dir: [f32; 2],
    #[serde(default)]
    intakes: Vec<[usize; 2]>,
    #[serde(default)]
    fry_safe: bool,
    sprite: SpriteDef,
}

fn default_nozzle_dir() -> [f32; 2] {
    [1.0, 0.0]
}

/// The device kinds, by [`DeviceKind`].
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceKinds {
    arts: Vec<DeviceArt>,
}

const BUILTIN: &[&str] = &[include_str!("../../../assets/devices/internal_filter.toml")];

impl DeviceKinds {
    /// The kinds embedded in the binary.
    ///
    /// # Errors
    ///
    /// Returns an error if a kind's TOML is invalid or a kind is missing.
    pub fn builtin() -> Result<Self, DeviceError> {
        let mut arts: Vec<DeviceArt> = Vec::new();
        for src in BUILTIN {
            let bad = |file: &str, message: String| DeviceError::Parse {
                file: file.to_owned(),
                message,
            };
            let def: ArtDef = toml::from_str(src).map_err(|e| bad("device", e.to_string()))?;
            let kind = DeviceKind::from_name(&def.name)
                .ok_or_else(|| bad(&def.name, "unknown device kind".to_owned()))?;
            let art = sprite(&def.sprite).map_err(|e| bad(&def.name, e.to_owned()))?;
            let size = (art.width / TEXELS_PER_CELL, art.height / TEXELS_PER_CELL);
            let frame = &art.frames[0];
            let footprint: Vec<bool> = (0..size.0 * size.1)
                .map(|c| {
                    let (cx, cy) = (c % size.0, c / size.0);
                    (0..TEXELS_PER_CELL * TEXELS_PER_CELL).any(|t| {
                        let x = cx * TEXELS_PER_CELL + t % TEXELS_PER_CELL;
                        let y = cy * TEXELS_PER_CELL + t / TEXELS_PER_CELL;
                        frame[y * art.width + x][3] > 0
                    })
                })
                .collect();
            let nozzle = (def.nozzle[0], def.nozzle[1]);
            if nozzle.0 >= size.0 || nozzle.1 >= size.1 || !footprint[nozzle.1 * size.0 + nozzle.0]
            {
                return Err(bad(
                    &def.name,
                    "the nozzle must be a footprint cell".to_owned(),
                ));
            }
            let [dx, dy] = def.nozzle_dir;
            let len = (dx * dx + dy * dy).sqrt();
            if !(dx.is_finite() && dy.is_finite() && len > 0.0) {
                return Err(bad(
                    &def.name,
                    "nozzle_dir must be a finite non-zero vector".to_owned(),
                ));
            }
            let nozzle_dir = [dx / len, dy / len];
            let mut intakes = Vec::with_capacity(def.intakes.len());
            for [ix, iy] in def.intakes {
                if ix >= size.0 || iy >= size.1 || !footprint[iy * size.0 + ix] {
                    return Err(bad(
                        &def.name,
                        "every intake must be a footprint cell".to_owned(),
                    ));
                }
                intakes.push((ix, iy));
            }
            arts.push(DeviceArt {
                kind,
                label: def.label,
                mount: def.mount,
                size,
                cells: footprint.iter().filter(|&&f| f).count(),
                footprint,
                nozzle,
                nozzle_dir,
                intakes,
                fry_safe: def.fry_safe,
                sprite: art,
            });
        }
        arts.sort_by_key(|a| a.kind.index());
        for (i, k) in DeviceKind::ALL.iter().enumerate() {
            if arts.get(i).map(|a| a.kind) != Some(*k) {
                return Err(DeviceError::Parse {
                    file: k.name().to_owned(),
                    message: "missing or duplicated".to_owned(),
                });
            }
        }
        Ok(Self { arts })
    }

    /// The art and rules for `kind`.
    #[must_use]
    pub fn get(&self, kind: DeviceKind) -> &DeviceArt {
        &self.arts[kind.index()]
    }

    /// Every kind, in [`DeviceKind::ALL`] order.
    pub fn iter(&self) -> impl Iterator<Item = &DeviceArt> {
        self.arts.iter()
    }
}

/// A device in the tank.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Device {
    /// Its id (its footprint cells hold it).
    pub id: DeviceId,
    /// What it is.
    pub kind: DeviceKind,
    /// Top-left cell of its art's box.
    pub x: usize,
    /// Top-left row of its art's box.
    pub y: usize,
    /// Mirrored: on the right glass, jetting left.
    pub facing_left: bool,
    /// Mechanical media load in `[0.0, 1.0]` (0 = clean, 1 = clogged).
    pub media_load: f32,
    /// Biological media maturity in `[0.0, 1.0]` (0 = fresh, 1 = mature).
    pub maturity: f32,
    /// Whether the intake guard is fitted (protects fry and small shrimp).
    pub intake_guard: bool,
}

impl Device {
    /// Effective flow multiplier in `[clog_floor, 1.0]` for this device's
    /// media load, quantized to 5% load steps so current rebuilds are bounded
    /// and save/load stays exact.
    #[must_use]
    pub fn flow_factor_with(&self, clog_floor: f32) -> f32 {
        let floor = clog_floor.clamp(0.0, 1.0);
        let step = (self.media_load.clamp(0.0, 1.0) * FLOW_STEPS).round() / FLOW_STEPS;
        1.0 - (1.0 - floor) * step
    }

    /// Effective flow multiplier using [`CLOG_FLOW_FLOOR`].
    #[must_use]
    pub fn flow_factor(&self) -> f32 {
        self.flow_factor_with(CLOG_FLOW_FLOOR)
    }

    /// Whether world cell `(x, y)` is one of its footprint cells.
    #[must_use]
    pub fn covers(&self, art: &DeviceArt, x: usize, y: usize) -> bool {
        let (w, h) = art.size;
        if x < self.x || y < self.y || x >= self.x + w || y >= self.y + h {
            return false;
        }
        let lx = x - self.x;
        let lx = if self.facing_left { w - 1 - lx } else { lx };
        art.footprint[(y - self.y) * w + lx]
    }

    /// Its footprint cells in the world, row-major.
    pub fn cells<'a>(&'a self, art: &'a DeviceArt) -> impl Iterator<Item = (usize, usize)> + 'a {
        let (w, h) = art.size;
        (0..w * h)
            .map(move |c| (self.x + c % w, self.y + c / w))
            .filter(move |&(x, y)| self.covers(art, x, y))
    }

    /// The nozzle tip cell in the world.
    #[must_use]
    pub fn nozzle(&self, art: &DeviceArt) -> (usize, usize) {
        let (nx, ny) = art.nozzle;
        let nx = if self.facing_left {
            art.size.0 - 1 - nx
        } else {
            nx
        };
        (self.x + nx, self.y + ny)
    }

    /// Unit direction the nozzle jets in the world (`+x` right, `+y` down).
    #[must_use]
    pub fn nozzle_dir(&self, art: &DeviceArt) -> [f32; 2] {
        let [dx, dy] = art.nozzle_dir;
        [if self.facing_left { -dx } else { dx }, dy]
    }

    /// Intake cells in the world.
    pub fn intakes<'a>(&'a self, art: &'a DeviceArt) -> impl Iterator<Item = (usize, usize)> + 'a {
        art.intakes.iter().map(move |&(ix, iy)| {
            let ix = if self.facing_left {
                art.size.0 - 1 - ix
            } else {
                ix
            };
            (self.x + ix, self.y + iy)
        })
    }
}

/// A device's status, for the inspect card.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeviceStatus {
    /// What it is.
    pub kind: DeviceKind,
    /// Jet speed at the nozzle, in cells per second.
    pub flow: f32,
    /// Mounted on the right glass, jetting left.
    pub facing_left: bool,
    /// Mechanical media load in `[0.0, 1.0]` (0 = clean, 1 = clogged).
    pub media_load: f32,
    /// Biological media maturity in `[0.0, 1.0]` (0 = fresh, 1 = mature).
    pub maturity: f32,
    /// Whether the intake guard is fitted.
    pub intake_guard: bool,
    /// Whether this kind is intrinsically fry-safe (no guard needed).
    pub fry_safe: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_internal_filter_footprint_follows_its_art() {
        let kinds = DeviceKinds::builtin().unwrap();
        let art = kinds.get(DeviceKind::InternalFilter);
        assert_eq!(art.size, (14, 30));
        assert_eq!(art.mount, Mount::SideGlass);
        // The body is solid; the corner beside the nozzle is open.
        assert!(art.footprint[15 * 14 + 5]);
        assert!(!art.footprint[13]);
        assert!(art.cells > 300 && art.cells < 14 * 30);
        // Mirrored, the nozzle is on the left.
        let d = |facing_left| Device {
            id: DeviceId(1),
            kind: DeviceKind::InternalFilter,
            x: 100,
            y: 50,
            facing_left,
            media_load: 0.0,
            maturity: 0.0,
            intake_guard: false,
        };
        assert_eq!(d(false).nozzle(art), (113, 58));
        assert_eq!(d(true).nozzle(art), (100, 58));
        assert!(d(true).covers(art, 100, 58));
        assert_eq!(d(true).cells(art).count(), art.cells);
    }
}
