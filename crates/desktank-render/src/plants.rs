//! Plant appearance textures (SPEC §2.7 *Appearance*).
//!
//! **Plant texture** (T6b): a 256×256 `Rgba8Unorm` texture with one texel
//! per plant id, at column `id & 255` and row `id >> 8`, so every id
//! (1–65535) has one. Its RGB is the health tint the shader multiplies into
//! every cell of that plant (the cell texture carries the id in its state
//! bytes) and its alpha the plant's species id (T6d). It is rebuilt from
//! the plant table when [`desktank_sim::World::plants_version`] changes,
//! and only the rows that changed are uploaded, so a plant's look changes
//! without waking a chunk or rewriting its cells.
//!
//! **Species texture** (T6d): [`SPECIES_WIDTH`]×256, one row per species
//! id, built once: each part's colour ramp resampled to [`RAMP`] shades
//! (root, stem, leaf), then the part shapes ([`desktank_sim::plants::Look`])
//! and the sizes the shader scales positions by. Rows without a species
//! repeat the default species, so a cell whose plant texel isn't written
//! yet still draws as a plant.

use desktank_sim::plants::{Form, PlantSpecies, PlantSpeciesTable};
use desktank_sim::{Part, World};

/// Texture side: 256 ids per row, 256 rows.
pub const SIDE: usize = 256;
/// Bytes per row.
const ROW_BYTES: usize = SIDE * 4;

/// Health at and above which a plant is drawn untinted.
const VIGOROUS: f32 = 0.8;
/// Health at which leaves start to drop (`desktank_sim`'s plant update):
/// the tint is fully yellowed here.
const YELLOW_AT: f32 = 0.5;
/// Pale yellow-green: a plant short of what it needs.
const YELLOW: [f32; 3] = [1.0, 0.92, 0.55];
/// Brown: a dying plant.
const BROWN: [f32; 3] = [0.62, 0.45, 0.26];

/// The tint for a plant of `health` (0–1): white while vigorous, paling to
/// yellow as health falls to 0.5, then browning as it dies.
#[must_use]
pub fn tint(health: f32) -> [u8; 3] {
    let h = health.clamp(0.0, 1.0);
    let mix = |a: [f32; 3], b: [f32; 3], t: f32| -> [f32; 3] {
        core::array::from_fn(|k| a[k] + (b[k] - a[k]) * t.clamp(0.0, 1.0))
    };
    let rgb = if h >= YELLOW_AT {
        mix([1.0; 3], YELLOW, (VIGOROUS - h) / (VIGOROUS - YELLOW_AT))
    } else {
        mix(YELLOW, BROWN, (YELLOW_AT - h) / YELLOW_AT)
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0..=1
    rgb.map(|v| (v * 255.0).round() as u8)
}

/// `rows` rows of texels with no plant: untinted, species 0 (a row of the
/// species texture that repeats the default species, so no real species id
/// is reserved).
fn empty_rows(rows: usize) -> Vec<u8> {
    [u8::MAX, u8::MAX, u8::MAX, 0].repeat(rows * SIDE)
}

/// The texture's contents on the CPU, to find the rows that changed.
#[derive(Clone, Debug)]
pub struct PlantTints {
    image: Vec<u8>,
    /// Rows that may hold a plant's texel (the rest are untinted).
    rows: usize,
    /// Plants version last built from.
    version: Option<u64>,
}

impl Default for PlantTints {
    fn default() -> Self {
        Self {
            image: empty_rows(SIDE),
            rows: 0,
            version: None,
        }
    }
}

impl PlantTints {
    /// The whole image, row-major (for the first upload).
    #[must_use]
    pub fn image(&self) -> &[u8] {
        &self.image
    }

    /// Rebuilds from `world`'s plants if they changed since the last call,
    /// and hands each changed row (its index and bytes) to `upload`.
    pub fn update(&mut self, world: &World, mut upload: impl FnMut(usize, &[u8])) {
        if self.version == Some(world.plants_version()) {
            return;
        }
        self.version = Some(world.plants_version());
        let rows = world
            .plants()
            .last()
            .map_or(0, |p| usize::from(p.id.0 >> 8) + 1);
        let mut fresh = empty_rows(rows.max(self.rows));
        for plant in world.plants() {
            let at = usize::from(plant.id.0) * 4;
            let [red, green, blue] = tint(plant.health);
            fresh[at..at + 4].copy_from_slice(&[red, green, blue, plant.species.0]);
        }
        let old_rows = self.image.as_chunks_mut::<ROW_BYTES>().0.iter_mut();
        for (row, (new, old)) in fresh
            .as_chunks::<ROW_BYTES>()
            .0
            .iter()
            .zip(old_rows)
            .enumerate()
        {
            if new != old {
                old.copy_from_slice(new);
                upload(row, new);
            }
        }
        self.rows = rows;
    }
}

/// Shades per part in the species texture: every ramp is resampled to
/// this many.
pub const RAMP: usize = 8;
/// Species texture width: three ramps, the shapes texel and the sizes
/// texel, rounded up. Row 0 (no species id) and rows of missing ids repeat
/// the default species.
pub const SPECIES_WIDTH: usize = 32;
/// Column of the shapes texel: leaf, stem and root shape discriminants.
pub const SHAPES_COLUMN: usize = 3 * RAMP;
/// Column of the sizes texel: stem height, root depth and leaf length in
/// cells (blade length for the first and last of a rosette or carpet; 0 for
/// a mat).
pub const SIZES_COLUMN: usize = SHAPES_COLUMN + 1;

/// The species texture's contents, row-major (`SPECIES_WIDTH` texels per
/// row, 256 rows).
#[must_use]
pub fn species_image(table: &PlantSpeciesTable) -> Vec<u8> {
    let row_bytes = SPECIES_WIDTH * 4;
    let mut out = vec![0; SIDE * row_bytes];
    let default = table.default_species().map(species_row);
    for (id, row) in out
        .as_chunks_mut::<{ SPECIES_WIDTH * 4 }>()
        .0
        .iter_mut()
        .enumerate()
    {
        let own = u8::try_from(id)
            .ok()
            .and_then(|id| table.get(desktank_sim::PlantSpeciesId(id)))
            .map(species_row);
        if let Some(bytes) = own.or(default) {
            *row = bytes;
        }
    }
    out
}

/// One species' row of the species texture.
fn species_row(s: &PlantSpecies) -> [u8; SPECIES_WIDTH * 4] {
    let mut row = [0; SPECIES_WIDTH * 4];
    // Runners have no ramp of their own: the shader draws them from the
    // stem's.
    for part in [Part::Root, Part::Stem, Part::Leaf] {
        let ramp = match part {
            Part::Root => &s.colours.root,
            Part::Stem | Part::Runner => &s.colours.stem,
            Part::Leaf => &s.colours.leaf,
        };
        for (k, shade) in resample(ramp).into_iter().enumerate() {
            let at = (part.index() * RAMP + k) * 4;
            row[at..at + 4].copy_from_slice(&[shade[0], shade[1], shade[2], u8::MAX]);
        }
    }
    let look = s.look;
    let at = SHAPES_COLUMN * 4;
    row[at..at + 4].copy_from_slice(&[look.leaf as u8, look.stem as u8, look.root as u8, 0]);
    // A rosette's or carpet's blades shade by length, as stems by height.
    let sizes = match &s.form {
        Form::Stem(f) => [f.height, f.root_depth, f.leaf_length, 0],
        Form::Rosette(f) | Form::Carpet(f) => [f.blade_length, f.root_depth, f.blade_length, 0],
        Form::Mat(_) => [0; 4],
    };
    let at = SIZES_COLUMN * 4;
    row[at..at + 4].copy_from_slice(&sizes);
    row
}

/// `ramp` (1 or more shades, dark to light) stretched or squeezed to
/// [`RAMP`] shades by linear interpolation between neighbours.
fn resample(ramp: &[[u8; 3]]) -> [[u8; 3]; RAMP] {
    let last = ramp.len().saturating_sub(1);
    core::array::from_fn(|k| {
        #[allow(clippy::cast_precision_loss)] // tiny counts
        let at = k as f32 * last as f32 / (RAMP - 1) as f32;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0..=last
        let lo = (at.floor() as usize).min(last);
        let hi = (lo + 1).min(last);
        let t = at - at.floor();
        let (a, b) = (ramp.get(lo), ramp.get(hi));
        match (a, b) {
            (Some(a), Some(b)) => core::array::from_fn(|c| {
                let v = f32::from(a[c]) + (f32::from(b[c]) - f32::from(a[c])) * t;
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0..=255
                let v = v.round() as u8;
                v
            }),
            _ => [0x40, 0x90, 0x40],
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_pales_then_browns() {
        assert_eq!(tint(1.0), [255, 255, 255]);
        assert_eq!(tint(0.85), [255, 255, 255]);
        let pale = tint(0.6);
        assert!(pale[2] < 200 && pale[0] == 255, "{pale:?}");
        let yellow = tint(0.5);
        let brown = tint(0.0);
        assert!(brown.iter().zip(yellow).all(|(b, y)| *b < y));
        // Monotonic: a sicker plant is never brighter.
        let mut last = tint(1.0);
        for k in (0..=100).rev() {
            #[allow(clippy::cast_precision_loss)]
            let t = tint(k as f32 / 100.0);
            assert!(t.iter().zip(last).all(|(a, b)| *a <= b), "{k}");
            last = t;
        }
    }

    #[test]
    fn ramps_are_resampled_end_to_end() {
        let r = resample(&[[0, 0, 0], [70, 140, 210]]);
        assert_eq!(r[0], [0, 0, 0]);
        assert_eq!(r[RAMP - 1], [70, 140, 210]);
        assert_eq!(r[1], [10, 20, 30]);
        assert_eq!(resample(&[[5, 6, 7]]), [[5, 6, 7]; RAMP]);
    }

    #[test]
    fn species_rows_hold_ramps_shapes_and_sizes() {
        let materials = desktank_sim::MaterialTable::builtin().unwrap();
        let table = PlantSpeciesTable::builtin_for(&materials).unwrap();
        let image = species_image(&table);
        let texel = |row: usize, col: usize| -> [u8; 4] {
            let at = (row * SPECIES_WIDTH + col) * 4;
            image[at..at + 4].try_into().unwrap()
        };
        let rotala = table.get(table.id("rotala").unwrap()).unwrap();
        let row = usize::from(rotala.id.0);
        // The leaf ramp's ends are the TOML's ends.
        let leaf = &rotala.colours.leaf;
        let first = texel(row, Part::Leaf.index() * RAMP);
        let last = texel(row, Part::Leaf.index() * RAMP + RAMP - 1);
        assert_eq!(first[..3], leaf[0]);
        assert_eq!(last[..3], leaf[leaf.len() - 1]);
        let shapes = texel(row, SHAPES_COLUMN);
        assert_eq!(shapes[0], rotala.look.leaf as u8);
        assert_eq!(shapes[1], rotala.look.stem as u8);
        let Form::Stem(f) = &rotala.form else {
            panic!("rotala is a stem plant")
        };
        assert_eq!(
            texel(row, SIZES_COLUMN)[..3],
            [f.height, f.root_depth, f.leaf_length]
        );
        // Rows without a species repeat the default.
        let default = usize::from(table.default_species().unwrap().id.0);
        let at = |r: usize| &image[r * SPECIES_WIDTH * 4..(r + 1) * SPECIES_WIDTH * 4];
        assert_eq!(at(0), at(default));
        // A plant texel with no plant points at row 0.
        assert!(
            PlantTints::default()
                .image()
                .as_chunks::<4>()
                .0
                .iter()
                .all(|t| *t == [255, 255, 255, 0])
        );
        assert_eq!(at(255), at(default));
        assert_ne!(at(row), at(default));
    }

    /// The shader's layout and shape constants match the Rust side.
    #[test]
    fn shader_constants_match_the_rust_side() {
        use desktank_sim::plants::{LeafShape, RootShape, StemShape};
        let wgsl = include_str!("tank.wgsl");
        for (name, value) in [
            ("RAMP", RAMP),
            ("SHAPES_COLUMN", SHAPES_COLUMN),
            ("SIZES_COLUMN", SIZES_COLUMN),
        ] {
            let line = format!("const {name}: i32 = {value};");
            assert!(wgsl.contains(&line), "tank.wgsl lacks `{line}`");
        }
        for p in Part::ALL {
            let name = format!("{p:?}").to_uppercase();
            let line = format!("const PART_{name}: i32 = {};", p.index());
            assert!(wgsl.contains(&line), "tank.wgsl lacks `{line}`");
        }
        let check = |prefix: &str, name: String, value: u8| {
            let line = format!("const {prefix}_{}: i32 = {value};", name.to_uppercase());
            assert!(wgsl.contains(&line), "tank.wgsl lacks `{line}`");
        };
        for s in LeafShape::ALL {
            check("LEAF", format!("{s:?}"), s as u8);
        }
        for s in StemShape::ALL {
            check("STEM", format!("{s:?}"), s as u8);
        }
        for s in RootShape::ALL {
            check("ROOT", format!("{s:?}"), s as u8);
        }
    }
}
