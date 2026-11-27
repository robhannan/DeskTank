//! New Tank configuration shared by every host: the starter presets, the
//! tank's physical size in centimetres and the initial settings.
//!
//! A [`NewTankConfig`] builds a [`World`] deterministically from its seed, so
//! the desktop app, the web build and the Android Dream make the same tank
//! for the same choices. Presets only lay out materials, hardscape and
//! devices; they hold no OS or GPU code (SPEC §2.1).

use rand_core::Rng;
use rand_pcg::Pcg32;

use crate::rng::splitmix64;
use crate::species::CELLS_PER_CM;
use crate::{
    Brush, DEFAULT_CREATURE_CAP, DEFAULT_TIME_RATIO, DeviceKind, MaterialTable, Rect, World,
    WorldConfig, WorldError,
};

/// A starter layout for a new tank.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TankPreset {
    /// Water and air only.
    #[default]
    Empty,
    /// A contoured gravel bed with rock accents and, optionally, starter
    /// equipment: no plants or creatures.
    Artificial,
}

impl TankPreset {
    /// Every preset, in the order a picker lists them.
    pub const ALL: &'static [Self] = &[Self::Empty, Self::Artificial];

    /// Its stable slug, for settings, command lines and URLs.
    #[must_use]
    pub fn id(&self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Artificial => "artificial",
        }
    }

    /// Its display name.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Empty => "Empty",
            Self::Artificial => "Artificial",
        }
    }

    /// One line saying what the preset makes.
    #[must_use]
    pub fn description(&self) -> &'static str {
        match self {
            Self::Empty => "A tank of clean water and nothing else: build it from scratch.",
            Self::Artificial => {
                "A coloured gravel bed with a few rocks, a filter and a bubbler. No plants."
            }
        }
    }

    /// The preset whose [`id`](Self::id) is `id`, ignoring ASCII case.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|p| p.id().eq_ignore_ascii_case(id))
    }
}

/// A tank of `width` × `height` cells in centimetres, at
/// [`CELLS_PER_CM`] cells per centimetre.
#[must_use]
#[allow(clippy::cast_precision_loss)] // Tank sides are at most 8192 cells.
pub fn dimensions_cm(width: usize, height: usize) -> (f32, f32) {
    (width as f32 / CELLS_PER_CM, height as f32 / CELLS_PER_CM)
}

/// Everything a host chooses when it makes a new tank.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NewTankConfig {
    /// Size, seed and water level.
    pub world: WorldConfig,
    /// The starter layout.
    pub preset: TankPreset,
    /// Whether the preset places its starter devices (presets with none
    /// ignore it).
    pub include_devices: bool,
    /// Easy mode: only old age kills (D4).
    pub easy_mode: bool,
    /// The most creatures the tank holds (clamped like
    /// [`World::set_creature_cap`]).
    pub population_cap: u16,
    /// The life-time multiplier (clamped like [`World::set_time_ratio`]).
    pub time_ratio: f32,
}

impl Default for NewTankConfig {
    fn default() -> Self {
        Self {
            world: WorldConfig::default(),
            preset: TankPreset::Empty,
            include_devices: true,
            easy_mode: false,
            population_cap: u16::try_from(DEFAULT_CREATURE_CAP).unwrap_or(u16::MAX),
            time_ratio: DEFAULT_TIME_RATIO,
        }
    }
}

impl NewTankConfig {
    /// The tank's width and height in centimetres.
    #[must_use]
    pub fn dimensions_cm(&self) -> (f32, f32) {
        dimensions_cm(self.world.width, self.world.height)
    }

    /// Builds the tank with the built-in materials.
    ///
    /// # Errors
    ///
    /// Returns an error if the built-in materials are invalid or the size or
    /// water level is out of range.
    pub fn build(&self) -> Result<World, WorldError> {
        let materials =
            MaterialTable::builtin().map_err(|e| WorldError::Materials(e.to_string()))?;
        self.build_with_materials(materials)
    }

    /// Builds the tank with `materials`.
    ///
    /// # Errors
    ///
    /// Returns an error if the size or water level is out of range or the
    /// table lacks a material the preset uses.
    pub fn build_with_materials(&self, materials: MaterialTable) -> Result<World, WorldError> {
        let mut world = World::new(self.world, materials)?;
        match self.preset {
            TankPreset::Empty => {}
            TankPreset::Artificial => {
                artificial(&mut world, self.world.seed, self.include_devices)?;
            }
        }
        world.set_easy_mode(self.easy_mode);
        world.set_creature_cap(usize::from(self.population_cap));
        world.set_time_ratio(self.time_ratio);
        Ok(world)
    }
}

impl World {
    /// A world made from a [`NewTankConfig`] with the built-in materials
    /// (see [`NewTankConfig::build`]).
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`NewTankConfig::build`].
    pub fn from_new_tank(config: &NewTankConfig) -> Result<World, WorldError> {
        config.build()
    }
}

/// Mixed into the seed so the layout stream differs from the world's own.
const LAYOUT_SALT: u64 = 0xA27F_1C0D_5EED_7A4B;
/// The bed is this share (percent) of the tank height at its lowest and
/// highest, never more than a third of the water.
const BED_LOW_PCT: usize = 8;
const BED_HIGH_PCT: usize = 12;

/// A uniform value in `lo..=hi` (`hi` raised to `lo` if smaller).
fn between(rng: &mut Pcg32, lo: usize, hi: usize) -> usize {
    let span = u64::try_from(hi.max(lo) - lo).map_or(1, |d| d.saturating_add(1));
    lo + usize::try_from(u64::from(rng.next_u32()) % span).unwrap_or(0)
}

fn rect(x0: usize, y0: usize, x1: usize, y1: usize) -> Rect {
    Rect { x0, y0, x1, y1 }
}

/// The bed's height above the floor in each column: seeded control points
/// joined by smoothstep ramps, integer-only so every platform agrees.
fn bed_profile(rng: &mut Pcg32, width: usize, low: usize, high: usize) -> Vec<usize> {
    let spacing = (width / 6).max(8);
    let points: Vec<usize> = (0..=width / spacing + 1)
        .map(|_| between(rng, low, high))
        .collect();
    let sp = i64::try_from(spacing).unwrap_or(8);
    let unit = sp * sp * sp;
    (0..width)
        .map(|x| {
            let seg = x / spacing;
            let along = i64::try_from(x % spacing).unwrap_or(0);
            let to_i = |v: usize| i64::try_from(v).unwrap_or(0);
            let from = to_i(points[seg]);
            let to = to_i(points[seg + 1]);
            // Smoothstep 3t² − 2t³ over 0..=spacing, scaled by spacing³.
            let step = along * along * (3 * sp - 2 * along);
            let v = (from * unit + (to - from) * step + unit / 2) / unit;
            usize::try_from(v).unwrap_or(0)
        })
        .collect()
}

/// The Artificial preset: gravel bed, (optionally) devices, then rocks.
fn artificial(world: &mut World, seed: u64, include_devices: bool) -> Result<(), WorldError> {
    let (w, h) = (world.width(), world.height());
    let gravel = crate::scene::material(world, "clown_vomit_gravel")?;
    let rock = crate::scene::material(world, "rock")?;
    let mut rng = Pcg32::new(splitmix64(seed ^ LAYOUT_SALT), 0x5ced);

    // A gentle bed, never more than a third of the water so small tanks
    // keep room above it.
    let surface = world.surface_row();
    let water_rows = h - surface;
    let low = (h * BED_LOW_PCT / 100).max(2).min(water_rows / 3);
    let high = (h * BED_HIGH_PCT / 100)
        .max(low)
        .min(water_rows / 3)
        .max(low);
    let profile = bed_profile(&mut rng, w, low, high);
    if low > 0 {
        for (x, &height) in profile.iter().enumerate() {
            world.fill_rect(rect(x, h - height, x + 1, h), gravel, Brush::Replace)?;
        }
    }

    // Devices go in first so the rocks can keep clear of where they landed.
    let mut keep_clear: Vec<(usize, usize)> = Vec::new();
    if include_devices {
        let mid = i64::try_from(surface + water_rows / 2).unwrap_or(0);
        let w_i = i64::try_from(w).unwrap_or(0);
        let filter_left = rng.next_u32() & 1 == 0;
        let filter_x = if filter_left { 0 } else { w_i - 1 };
        let bubbler_pct = if filter_left { 55..=80 } else { 20..=45 };
        let bubbler_x = i64::try_from(between(
            &mut rng,
            w * bubbler_pct.start() / 100,
            w * bubbler_pct.end() / 100,
        ))
        .unwrap_or(0);
        for (kind, x) in [
            (DeviceKind::InternalFilter, filter_x),
            (DeviceKind::Bubbler, bubbler_x),
        ] {
            if let Some(id) = world.add_device(kind, x, mid, false)?
                && let Some(d) = world.device(id)
            {
                let width = world.device_kinds().get(kind).size.0;
                keep_clear.push((d.x, d.x + width));
            }
        }
    }

    // Two to four rocks sitting in the bed, glued so they never shift.
    let count = between(&mut rng, 2, 4);
    let max_r = (h / 14).min(w / 20).max(2);
    let min_r = (h / 28).clamp(1, max_r);
    let mut rocks: Vec<(usize, usize)> = Vec::new(); // (cx, r)
    for _ in 0..count {
        for _ in 0..24 {
            let r = between(&mut rng, min_r, max_r);
            let cx = between(&mut rng, r + 1, w.saturating_sub(r + 2).max(r + 1));
            let clear = |(a, b): (usize, usize)| cx + r + 2 <= a || b + r + 2 <= cx;
            let apart = |(ox, or): (usize, usize)| cx.abs_diff(ox) > r + or + 2;
            if cx + r >= w
                || !keep_clear.iter().copied().all(clear)
                || !rocks.iter().copied().all(apart)
            {
                continue;
            }
            // Centre half a radius above the bed, so the rock is half sunk
            // and its top stays well under the surface.
            let top = h - profile[cx];
            let cy = top.saturating_sub(r / 2);
            if cy < surface + r + 2 || cy + r >= h {
                continue;
            }
            let (px, py) = (
                i64::try_from(cx).unwrap_or(0),
                i64::try_from(cy).unwrap_or(0),
            );
            world.paint(px, py, u32::try_from(r).unwrap_or(1), rock, Brush::Replace)?;
            if let Some(body) = world.body_at(cx, cy) {
                world.set_anchored(body, true);
            }
            rocks.push((cx, r));
            break;
        }
    }
    Ok(())
}
