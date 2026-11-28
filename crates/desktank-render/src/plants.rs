//! Plant appearance textures (SPEC §2.7 *Appearance*).
//!
//! **Plant texture** (T6b): a 256×256 `Rgba8Unorm` texture with one texel
//! per plant id, at column `id & 255` and row `id >> 8`, so every id
//! (1–65535) has one (the cell texture carries the id in its state bytes).
//! T6d: red holds the plant's health and green its colouration, each at
//! [`desktank_sim::TINT_STEPS`] steps, from which the shader tints the
//! whole plant (pale, yellowing, then brown and translucent as it dies)
//! and shifts it towards its species' colouration colour; blue holds a
//! ball's roll angle ([`desktank_sim::PlantInfo::roll`], `0..=255` for
//! `0..2π` clockwise, 0 for other plants), which turns the marimo velvet,
//! and alpha holds the species id. It is rebuilt from
//! the plant table when [`desktank_sim::World::plants_version`] changes,
//! and only the rows that changed are uploaded, so a plant's look changes
//! without waking a chunk or rewriting its cells.
//!
//! **Species texture** (T6d): [`SPECIES_WIDTH`]×256, one row per species
//! id, built once: each part's colour ramp resampled to [`RAMP`] shades
//! (root, stem, leaf), then the part shapes ([`desktank_sim::plants::Look`]),
//! the sizes the shader scales positions by and the colouration colour
//! ([`desktank_sim::plants::Colouration`]). Rows without a species
//! repeat the default species, so a cell whose plant texel isn't written
//! yet still draws as a plant.

use desktank_sim::plants::{Form, PlantSpecies, PlantSpeciesTable};
use desktank_sim::{Part, World};

/// Texture side: 256 ids per row, 256 rows.
pub const SIDE: usize = 256;
/// Bytes per row.
const ROW_BYTES: usize = SIDE * 4;

/// A 0–1 value (health or colouration) as the texel byte the shader reads:
/// its [`desktank_sim::TINT_STEPS`] step, so the texel changes exactly when
/// the plants version does.
#[must_use]
pub fn texel_byte(v: f32) -> u8 {
    let step = u32::from(desktank_sim::tint_step(v));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0..=255
    let byte = (f64::from(step) * 255.0 / f64::from(desktank_sim::TINT_STEPS)).round() as u8;
    byte
}

/// `rows` rows of texels with no plant: healthy, uncoloured, species 0 (a
/// row of the species texture that repeats the default species, so no
/// real species id is reserved).
fn empty_rows(rows: usize) -> Vec<u8> {
    [u8::MAX, 0, 0, 0].repeat(rows * SIDE)
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

    /// Forgets the plants version last built from, so the next
    /// [`PlantTints::update`] rebuilds for a different world. The image and
    /// row count stay: they describe the texture as uploaded, so the update
    /// diffs against it and clears rows the new world no longer uses.
    pub fn invalidate(&mut self) {
        self.version = None;
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
            fresh[at..at + 4].copy_from_slice(&[
                texel_byte(plant.health),
                texel_byte(plant.colour),
                plant.roll,
                plant.species.0,
            ]);
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
/// cells (blade length for the first and last of a rosette or carpet, then
/// its leaf pad width; fronds, root length and fronds for a floater; radius
/// for the first and third of a ball; 0 for a mat).
pub const SIZES_COLUMN: usize = SHAPES_COLUMN + 1;
/// Column of the colouration texel: the colour a plant shifts towards in
/// strong light with low nutrients (T6d), alpha 255 if it colours at all.
pub const COLOUR_COLUMN: usize = SIZES_COLUMN + 1;

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
    // Runners, rhizomes and floating leaves have no ramps of their own: the
    // shader draws them from the stem's and the leaf's.
    for part in [Part::Root, Part::Stem, Part::Leaf] {
        let ramp = match part {
            Part::Root => &s.colours.root,
            Part::Stem | Part::Runner | Part::Rhizome => &s.colours.stem,
            Part::Leaf | Part::Float => &s.colours.leaf,
        };
        for (k, shade) in resample(ramp).into_iter().enumerate() {
            let at = (part.index() * RAMP + k) * 4;
            row[at..at + 4].copy_from_slice(&[shade[0], shade[1], shade[2], u8::MAX]);
        }
    }
    let look = s.look;
    let at = SHAPES_COLUMN * 4;
    row[at..at + 4].copy_from_slice(&[
        look.leaf as u8,
        look.stem as u8,
        look.root as u8,
        look.arrangement as u8,
    ]);
    // A rosette's, carpet's or epiphyte's blades shade by length, as stems
    // by height.
    let sizes = match &s.form {
        // A stem plant's node spacing, for the arrangement of its leaves.
        Form::Stem(f) => [f.height, f.root_depth, f.leaf_length, f.internode],
        // A tiger lotus type's leaf pads: leaf cells past the blade length,
        // up to `pad` further.
        Form::Rosette(f) | Form::Carpet(f) => [f.blade_length, f.root_depth, f.blade_length, f.pad],
        Form::Mat(_) => [0; 4],
        // A floater's roots shade by length; its fronds by how far out.
        Form::Floater(f) => [f.fronds, f.root_length, f.fronds, 0],
        Form::Epiphyte(f) => [f.leaf_length, f.rhizome_length, f.leaf_length, 0],
        // A ball shades by how far out a cell grew.
        Form::Ball(f) => [f.radius, 0, f.radius, 0],
    };
    let at = SIZES_COLUMN * 4;
    row[at..at + 4].copy_from_slice(&sizes);
    if let Some(c) = s.colouration {
        let at = COLOUR_COLUMN * 4;
        row[at..at + 4].copy_from_slice(&[c.colour[0], c.colour[1], c.colour[2], u8::MAX]);
    }
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
    fn texel_bytes_follow_the_tint_steps() {
        assert_eq!(texel_byte(1.0), 255);
        assert_eq!(texel_byte(0.0), 0);
        assert_eq!(texel_byte(2.0), 255);
        assert_eq!(texel_byte(0.5), 128);
        // Values within one step share a byte.
        assert_eq!(texel_byte(0.5), texel_byte(0.51));
        let mut last = 0;
        for k in 0..=100 {
            #[allow(clippy::cast_precision_loss)]
            let b = texel_byte(k as f32 / 100.0);
            assert!(b >= last, "{k}");
            last = b;
        }
    }

    #[test]
    fn a_rolling_balls_angle_is_the_blue_byte() {
        use desktank_sim::{Brush, MaterialTable, Rect, WorldConfig};
        const W: usize = 160;
        const H: usize = 90;
        const FLOOR: usize = H - 8;
        let rect = |x0, y0, x1, y1| Rect { x0, y0, x1, y1 };
        let mut w = World::new(
            WorldConfig {
                width: W,
                height: H,
                seed: 2,
                water_level: 0.95,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let soil = w.materials().id("aquasoil").unwrap();
        let rock = w.materials().id("rock").unwrap();
        let water = w.materials().id("water").unwrap();
        w.fill_rect(rect(0, FLOOR, W, H), soil, Brush::Replace)
            .unwrap();
        // A ball resting on a ledge, with the bed then dropping away to its
        // right (a 1:1 slope) so it rolls down it.
        let (start, top) = (64, FLOOR - 12);
        let end = start + (FLOOR - top);
        w.fill_rect(rect(0, top, end, FLOOR), rock, Brush::Replace)
            .unwrap();
        let s = w.plant_species().id("marimo").unwrap();
        let fragment = w.plant_species().get(s).unwrap().default_propagule();
        w.paint_plant(64, i64::try_from(top).unwrap() - 1, 0, s, fragment)
            .unwrap();
        for _ in 0..6 * 1200 {
            w.tick();
        }
        let id = w.plants().find(|p| p.species == s).unwrap().id;
        let mut tints = PlantTints::default();
        let blue = |tints: &PlantTints| tints.image()[usize::from(id.0) * 4 + 2];
        tints.update(&w, |_, _| {});
        assert_eq!((w.plant(id).unwrap().roll, blue(&tints)), (0, 0));
        for x in start + 1..end {
            let y = top + (x - start);
            w.fill_rect(rect(x, top, x + 1, y), water, Brush::Replace)
                .unwrap();
        }
        for _ in 0..10 {
            for _ in 0..1200 {
                w.tick();
            }
            let roll = w.plant(id).unwrap().roll;
            if roll != 0 {
                let mut uploaded = Vec::new();
                tints.update(&w, |row, _| uploaded.push(row));
                assert_eq!(blue(&tints), roll);
                assert_eq!(uploaded, [usize::from(id.0) >> 8]);
                return;
            }
        }
        panic!("the ball never rolled");
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
        // Rotala colours; hygrophila doesn't.
        let c = rotala.colouration.unwrap();
        assert_eq!(
            texel(row, COLOUR_COLUMN),
            [c.colour[0], c.colour[1], c.colour[2], 255]
        );
        let hygro = usize::from(table.id("hygrophila").unwrap().0);
        assert_eq!(texel(hygro, COLOUR_COLUMN)[3], 0);
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
                .all(|t| *t == [255, 0, 0, 0])
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
            ("COLOUR_COLUMN", COLOUR_COLUMN),
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
        // The upper cells' position offset is the sim's.
        let line = format!(
            "const LEAF_UPPER: i32 = {};",
            desktank_sim::plants::LEAF_UPPER
        );
        assert!(wgsl.contains(&line), "tank.wgsl lacks `{line}`");
    }

    /// The shader counts a stem leaf's length along its row to four cells past
    /// the one it is in, and reads the leaf taper's `half` (a stem's half
    /// height) and `leaf_length` as 8-bit sizes: every shipped stem species
    /// stays within them, so what the sim grows and what the shader draws
    /// agree.
    #[test]
    fn shipped_stem_species_fit_the_shaders_loops() {
        let m = desktank_sim::MaterialTable::builtin().unwrap();
        let table = PlantSpeciesTable::builtin_for(&m).unwrap();
        for sp in table.iter() {
            let Form::Stem(f) = &sp.form else { continue };
            assert!(
                f.height / 2 <= 32,
                "{}: half height {}",
                sp.name,
                f.height / 2
            );
            assert!(f.leaf_length <= 5, "{}: leaf {}", sp.name, f.leaf_length);
            assert!(f.internode >= 2, "{}", sp.name);
        }
    }
}
