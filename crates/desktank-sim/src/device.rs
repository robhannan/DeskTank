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
//! Built-in kinds:
//! - **Bubbler** (DV7): bed-mounted air-stone diffuser disc (drops onto
//!   substrate/hardscape/floor), emits a dense curtain of rising air bubbles
//!   across its porous face with no water-current jet or filtration intakes.
//! - **Internal filter** (DV1): suction-cupped to the side glass at any height
//!   under the water line; its nozzle drives a horizontal jet away from the glass.
//! - **Sponge filter** (DV4): bed-mounted (drops onto substrate/hardscape/floor),
//!   emits air-driven uplift bubbles and gentle upward flow, is intrinsically
//!   fry-safe, and invites shrimp to graze its porous foam.
//! - **HOB filter** (DV5): rim-mounted (hangs on the top edge partly above the
//!   water line), pours a waterfall into the surface that ripples it, and drops
//!   its intake tube in the back depth band so fish pass in front of it.

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
    /// A bed-mounted air-stone diffuser bubbler (DV7).
    Bubbler,
    /// An internal power filter on the side glass (DV1).
    InternalFilter,
    /// A bed-mounted air-driven sponge filter (DV4).
    SpongeFilter,
    /// A rim-mounted hang-on-back waterfall filter (DV5).
    HobFilter,
}

impl DeviceKind {
    /// Every kind, in palette order.
    pub const ALL: [Self; 4] = [
        Self::Bubbler,
        Self::InternalFilter,
        Self::SpongeFilter,
        Self::HobFilter,
    ];

    /// Its name in saves, scripts and `assets/devices/`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Bubbler => "bubbler",
            Self::InternalFilter => "internal_filter",
            Self::SpongeFilter => "sponge_filter",
            Self::HobFilter => "hob_filter",
        }
    }

    /// The kind named `name`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }

    fn index(self) -> usize {
        match self {
            Self::Bubbler => 0,
            Self::InternalFilter => 1,
            Self::SpongeFilter => 2,
            Self::HobFilter => 3,
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
    /// Dropped straight down onto substrate, hardscape or the tank floor,
    /// wholly under the water line (symmetric; wall-flip ignored).
    Bed,
    /// Hanging on the top rim of the tank (`y = 0`), partly above the water
    /// line with its intake submerged (also allowed at `surface_row == 0` when
    /// filled to the brim); mirrors left/right when flipped.
    Rim,
}

impl Mount {
    /// Whether devices with this mount can be flipped left/right.
    #[must_use]
    pub const fn flippable(self) -> bool {
        match self {
            Self::SideGlass | Self::Rim => true,
            Self::Bed => false,
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
    /// The nozzle tip in cells from the top-left, facing right, if this
    /// device drives a directed water-current jet (`None` for pure air
    /// diffusers such as the bubbler).
    pub nozzle: Option<(usize, usize)>,
    /// Unit direction the nozzle jets, facing right (`+x` right, `+y` down).
    pub nozzle_dir: [f32; 2],
    /// Flow strength multiplier relative to `[current].speed` (1.0 for power
    /// filters, gentler for air-driven uplift, 0.0 for non-jet diffusers).
    pub flow_scale: f32,
    /// Bubble outlet cells from the top-left, facing right: emit rising air
    /// bubbles from the water cell directly above each outlet.
    pub bubble_outlets: Vec<(usize, usize)>,
    /// Clean bubble emission rate per outlet cell, in bubbles per second
    /// (scaled by the device's clog flow factor; 0 for no bubbles).
    pub bubble_rate: f32,
    /// Intake cells from the top-left, facing right: water and loose
    /// particles are drawn towards them.
    pub intakes: Vec<(usize, usize)>,
    /// Whether this kind is intrinsically safe for fry and small shrimp
    /// without an intake guard (e.g. sponge filter).
    pub fry_safe: bool,
    /// Inclusive local cell rows `(y0, y1)` of the foam body that shrimp seek
    /// out and graze, if this device invites shrimp grazing.
    pub graze_rows: Option<(usize, usize)>,
    /// Local cell row from which this device's footprint occupies only the
    /// back depth band (`DEPTH_BANDS - 1`, D21) so fish swim and render in
    /// front of it (e.g. a HOB filter's intake tube). `None` means solid in
    /// every band.
    pub back_band_from: Option<usize>,
    /// Waterfall spillway span `(x0, x1, y)` in local cells (inclusive columns
    /// `x0..=x1` at lip row `y`, facing right) that drives surface ripples.
    pub spillway: Option<(usize, usize, usize)>,
    /// The art, facing right (mounted on the left glass).
    pub sprite: Sprite,
}

impl DeviceArt {
    /// Whether this device filters water (has intake cells that capture
    /// particles, clog media, mature biologically, and can be rinsed).
    #[must_use]
    pub fn is_filter(&self) -> bool {
        !self.intakes.is_empty()
    }
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
    #[serde(default)]
    nozzle: Option<[usize; 2]>,
    #[serde(default = "default_nozzle_dir")]
    nozzle_dir: [f32; 2],
    #[serde(default = "default_flow_scale")]
    flow_scale: f32,
    #[serde(default)]
    bubble_outlets: Vec<[usize; 2]>,
    #[serde(default)]
    bubble_rate: f32,
    #[serde(default)]
    intakes: Vec<[usize; 2]>,
    #[serde(default)]
    fry_safe: bool,
    #[serde(default)]
    graze_rows: Option<[usize; 2]>,
    #[serde(default)]
    back_band_from: Option<usize>,
    #[serde(default)]
    spillway: Option<[usize; 3]>,
    sprite: SpriteDef,
}

fn default_nozzle_dir() -> [f32; 2] {
    [1.0, 0.0]
}

fn default_flow_scale() -> f32 {
    1.0
}

/// The device kinds, by [`DeviceKind`].
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceKinds {
    arts: Vec<DeviceArt>,
}

const BUILTIN: &[&str] = &[
    include_str!("../../../assets/devices/bubbler.toml"),
    include_str!("../../../assets/devices/internal_filter.toml"),
    include_str!("../../../assets/devices/sponge_filter.toml"),
    include_str!("../../../assets/devices/hob_filter.toml"),
];

impl DeviceKinds {
    /// The kinds embedded in the binary.
    ///
    /// # Errors
    ///
    /// Returns an error if a kind's TOML is invalid or a kind is missing.
    pub fn builtin() -> Result<Self, DeviceError> {
        let mut arts: Vec<DeviceArt> = BUILTIN
            .iter()
            .map(|src| Self::parse_art(src))
            .collect::<Result<_, _>>()?;
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

    #[allow(clippy::too_many_lines)] // one validation check per TOML field
    fn parse_art(src: &str) -> Result<DeviceArt, DeviceError> {
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
        let in_footprint =
            |x: usize, y: usize| x < size.0 && y < size.1 && footprint[y * size.0 + x];
        let nozzle = match def.nozzle {
            Some([nx, ny]) => {
                if !in_footprint(nx, ny) {
                    return Err(bad(
                        &def.name,
                        "the nozzle must be a footprint cell".to_owned(),
                    ));
                }
                Some((nx, ny))
            }
            None => None,
        };
        let nozzle_dir = if nozzle.is_some() {
            let [dx, dy] = def.nozzle_dir;
            let len = (dx * dx + dy * dy).sqrt();
            if !(dx.is_finite() && dy.is_finite() && len > 0.0) {
                return Err(bad(
                    &def.name,
                    "nozzle_dir must be a finite non-zero vector".to_owned(),
                ));
            }
            [dx / len, dy / len]
        } else {
            [0.0, 0.0]
        };
        if nozzle.is_some() {
            if !(def.flow_scale.is_finite() && def.flow_scale > 0.0) {
                return Err(bad(
                    &def.name,
                    "flow_scale must be finite and positive when a nozzle is set".to_owned(),
                ));
            }
        } else if !(def.flow_scale.is_finite() && def.flow_scale >= 0.0) {
            return Err(bad(
                &def.name,
                "flow_scale must be finite and non-negative when no nozzle is set".to_owned(),
            ));
        }
        if !(def.bubble_rate.is_finite() && def.bubble_rate >= 0.0) {
            return Err(bad(
                &def.name,
                "bubble_rate must be finite and non-negative".to_owned(),
            ));
        }
        let mut bubble_outlets = Vec::with_capacity(def.bubble_outlets.len());
        for [bx, by] in def.bubble_outlets {
            if !in_footprint(bx, by) {
                return Err(bad(
                    &def.name,
                    "every bubble outlet must be a footprint cell".to_owned(),
                ));
            }
            bubble_outlets.push((bx, by));
        }
        let mut intakes = Vec::with_capacity(def.intakes.len());
        for [ix, iy] in def.intakes {
            if !in_footprint(ix, iy) {
                return Err(bad(
                    &def.name,
                    "every intake must be a footprint cell".to_owned(),
                ));
            }
            intakes.push((ix, iy));
        }
        let graze_rows = match def.graze_rows {
            Some([y0, y1]) if y0 <= y1 && y1 < size.1 => Some((y0, y1)),
            Some(_) => {
                return Err(bad(
                    &def.name,
                    "graze_rows must be an inclusive range within sprite height".to_owned(),
                ));
            }
            None => None,
        };
        let back_band_from = match def.back_band_from {
            Some(y) if y > 0 && y < size.1 => Some(y),
            Some(_) => {
                return Err(bad(
                    &def.name,
                    "back_band_from must be strictly inside sprite height".to_owned(),
                ));
            }
            None => None,
        };
        let spillway = match def.spillway {
            Some([x0, x1, y]) if x0 <= x1 && x1 < size.0 && y < size.1 => {
                if !in_footprint(x0, y) || !in_footprint(x1, y) {
                    return Err(bad(
                        &def.name,
                        "spillway endpoints must be footprint cells".to_owned(),
                    ));
                }
                Some((x0, x1, y))
            }
            Some(_) => {
                return Err(bad(
                    &def.name,
                    "spillway must be [x0, x1, y] within sprite bounds with x0 <= x1".to_owned(),
                ));
            }
            None => None,
        };
        Ok(DeviceArt {
            kind,
            label: def.label,
            mount: def.mount,
            size,
            cells: footprint.iter().filter(|&&f| f).count(),
            footprint,
            nozzle,
            nozzle_dir,
            flow_scale: def.flow_scale,
            bubble_outlets,
            bubble_rate: def.bubble_rate,
            intakes,
            fry_safe: def.fry_safe,
            graze_rows,
            back_band_from,
            spillway,
            sprite: art,
        })
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

    /// The nozzle tip cell in the world, if this device has one.
    #[must_use]
    pub fn nozzle(&self, art: &DeviceArt) -> Option<(usize, usize)> {
        let (nx, ny) = art.nozzle?;
        let nx = if self.facing_left {
            art.size.0 - 1 - nx
        } else {
            nx
        };
        Some((self.x + nx, self.y + ny))
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

    /// Outward unit direction into open water from world intake cell `(ix, iy)`.
    #[must_use]
    pub fn intake_outward(&self, art: &DeviceArt, ix: usize, _iy: usize) -> [f32; 2] {
        match art.mount {
            Mount::SideGlass => self.nozzle_dir(art),
            Mount::Bed => {
                let lx = ix.saturating_sub(self.x);
                match lx.cmp(&(art.size.0 / 2)) {
                    core::cmp::Ordering::Less => [-1.0, 0.0],
                    core::cmp::Ordering::Greater => [1.0, 0.0],
                    core::cmp::Ordering::Equal => [0.0, -1.0],
                }
            }
            Mount::Rim => {
                // Point outwards from the vertical intake strainer's centre column.
                let sum: usize = self.intakes(art).map(|(x, _)| x).sum();
                let n = art.intakes.len().max(1);
                match (ix * n).cmp(&sum) {
                    core::cmp::Ordering::Less => [-1.0, 0.0],
                    core::cmp::Ordering::Greater => [1.0, 0.0],
                    core::cmp::Ordering::Equal => [0.0, 1.0],
                }
            }
        }
    }

    /// World row from which this device's footprint lies in the back depth
    /// band (`DEPTH_BANDS - 1`), if any.
    #[must_use]
    pub fn back_band_row(&self, art: &DeviceArt) -> Option<usize> {
        art.back_band_from.map(|r| self.y + r)
    }

    /// Waterfall spillway span `(x0, x1_exclusive, y)` in world cells, if any.
    #[must_use]
    pub fn spillway_world(&self, art: &DeviceArt) -> Option<(usize, usize, usize)> {
        let (x0, x1, y) = art.spillway?;
        let (lx0, lx1) = if self.facing_left {
            (art.size.0 - 1 - x1, art.size.0 - 1 - x0)
        } else {
            (x0, x1)
        };
        Some((self.x + lx0, self.x + lx1 + 1, self.y + y))
    }

    /// Bubble outlet cells in the world.
    pub fn bubble_outlets<'a>(
        &'a self,
        art: &'a DeviceArt,
    ) -> impl Iterator<Item = (usize, usize)> + 'a {
        art.bubble_outlets.iter().map(move |&(bx, by)| {
            let bx = if self.facing_left {
                art.size.0 - 1 - bx
            } else {
                bx
            };
            (self.x + bx, self.y + by)
        })
    }

    /// Inclusive world row range `(y0, y1)` of this device's shrimp-grazeable
    /// foam body, if any.
    #[must_use]
    pub fn graze_band(&self, art: &DeviceArt) -> Option<(usize, usize)> {
        let (y0, y1) = art.graze_rows?;
        Some((self.y + y0, self.y + y1))
    }
}

/// A device's status, for the inspect card.
#[derive(Clone, Copy, Debug, PartialEq)]
#[allow(clippy::struct_excessive_bools)] // inspect-card flags
pub struct DeviceStatus {
    /// What it is.
    pub kind: DeviceKind,
    /// Whether this device filters water (intakes, media load, maturity, Rinse).
    pub is_filter: bool,
    /// Jet speed at the nozzle, in cells per second (0 for non-jet diffusers).
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
        assert_eq!(d(false).nozzle(art), Some((113, 58)));
        assert_eq!(d(true).nozzle(art), Some((100, 58)));
        assert!(d(true).covers(art, 100, 58));
        assert_eq!(d(true).cells(art).count(), art.cells);
    }

    #[test]
    fn the_bubbler_is_bed_mounted_fry_safe_and_pure_air_diffuser() {
        let kinds = DeviceKinds::builtin().unwrap();
        let art = kinds.get(DeviceKind::Bubbler);
        assert_eq!(art.size, (14, 6));
        assert_eq!(art.mount, Mount::Bed);
        assert!(!art.mount.flippable());
        assert!(!art.is_filter());
        assert!(art.fry_safe);
        assert_eq!(art.nozzle, None);
        assert_eq!(art.flow_scale, 0.0);
        assert_eq!(art.intakes, Vec::new());
        assert!(art.bubble_rate > 0.0 && art.bubble_outlets.len() == 8);
        // Every bubble outlet has open water (or top edge) directly above it.
        for &(bx, by) in &art.bubble_outlets {
            assert!(art.footprint[by * art.size.0 + bx]);
            if by > 0 {
                assert!(
                    !art.footprint[(by - 1) * art.size.0 + bx],
                    "outlet ({bx}, {by}) must not be covered by another footprint cell"
                );
            }
        }
        // Water above the puck, with a solid body and side fitting below.
        assert!(!art.footprint[0]);
        assert!(!art.footprint[13]);
        assert!(art.footprint[3 * 14 + 6]);
        assert!(art.footprint[3 * 14 + 13]);
    }

    #[test]
    fn the_sponge_filter_is_bed_mounted_fry_safe_and_bubbling() {
        let kinds = DeviceKinds::builtin().unwrap();
        let art = kinds.get(DeviceKind::SpongeFilter);
        assert_eq!(art.size, (15, 26));
        assert_eq!(art.mount, Mount::Bed);
        assert!(art.is_filter());
        assert!(art.fry_safe);
        assert!(art.flow_scale > 0.0 && art.flow_scale < 1.0);
        assert!(art.bubble_rate > 0.0 && !art.bubble_outlets.is_empty());
        assert_eq!(art.nozzle, Some((7, 0)));
        assert_eq!(art.nozzle_dir, [0.0, -1.0]);
        assert_eq!(art.graze_rows, Some((8, 23)));
        assert!(art.intakes.len() >= 2);
        // Open corners beside the uplift tube; solid sponge body below.
        assert!(!art.footprint[0]);
        assert!(art.footprint[7]);
        assert!(art.footprint[15 * 15 + 7]);
    }

    #[test]
    fn the_hob_filter_is_rim_mounted_with_back_band_tube_and_spillway() {
        let kinds = DeviceKinds::builtin().unwrap();
        let art = kinds.get(DeviceKind::HobFilter);
        assert_eq!(art.size, (22, 32));
        assert_eq!(art.mount, Mount::Rim);
        assert!(art.mount.flippable());
        assert!(!art.fry_safe);
        assert_eq!(art.back_band_from, Some(10));
        assert_eq!(art.spillway, Some((1, 12, 9)));
        assert_eq!(art.intakes.len(), 4);
        assert!(art.nozzle_dir[1] > 0.9, "waterfall jets downwards");
        // Under the spillway is open water; the intake tube sits on the right.
        assert!(!art.footprint[15 * 22 + 5]);
        assert!(art.footprint[15 * 22 + 16]);
        let unflipped = Device {
            id: DeviceId(1),
            kind: DeviceKind::HobFilter,
            x: 40,
            y: 0,
            facing_left: false,
            media_load: 0.0,
            maturity: 0.0,
            intake_guard: false,
        };
        let flipped = Device {
            facing_left: true,
            ..unflipped
        };
        assert_eq!(unflipped.spillway_world(art), Some((41, 53, 9)));
        assert_eq!(flipped.spillway_world(art), Some((49, 61, 9)));
        assert_eq!(unflipped.back_band_row(art), Some(10));
        assert!(unflipped.nozzle_dir(art)[0] < 0.0);
        assert!(flipped.nozzle_dir(art)[0] > 0.0);
    }
}
