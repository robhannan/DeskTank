//! The per-plant texture (T6b, SPEC §2.7 *Appearance*): a 256×256
//! `Rgba8Unorm` texture with one texel per plant id, at column `id & 255`
//! and row `id >> 8`, so every id (1–65535) has one. Its RGB is the health
//! tint the shader multiplies into every cell of that plant (the cell
//! texture carries the id in its state bytes). It is rebuilt from the plant
//! table when [`desktank_sim::World::plants_version`] changes, and only the
//! rows that changed are uploaded, so a plant's look changes without waking
//! a chunk or rewriting its cells. T6d adds species colouration.

use desktank_sim::World;

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
            image: vec![u8::MAX; SIDE * ROW_BYTES],
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
        let mut fresh = vec![u8::MAX; rows.max(self.rows) * ROW_BYTES];
        for plant in world.plants() {
            let at = usize::from(plant.id.0) * 4;
            let [red, green, blue] = tint(plant.health);
            fresh[at..at + 4].copy_from_slice(&[red, green, blue, u8::MAX]);
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
}
