//! The creature sprite atlas and per-frame sprite instances (SPEC §4.2).
//!
//! Every species' frames are packed side by side, one shelf per species,
//! into one RGBA texture (sRGB-encoded, read raw with `textureLoad` like
//! the palette). Each frame, every creature becomes one instance: its
//! top-left cell, on-screen size, atlas frame, mirror and quarter turns,
//! drawn as a quad by the sprite pipeline at the grid's cell scale, with
//! [`TEXELS_PER_CELL`] texels per cell side (half a cell per texel). A
//! crawler on a wall is turned a quarter, so its quad swaps width and
//! height and the shader turns the texel lookup; quarter turns keep the
//! pixel art crisp. WebGL2-safe: a per-instance vertex
//! buffer, no storage buffers.

use desktank_sim::{SpeciesTable, TEXELS_PER_CELL, World};

/// Atlas width in texels; species shelves stack downwards.
pub const ATLAS_WIDTH: usize = 256;
/// Most sprites drawn in a frame (the population cap is lower).
pub const MAX_SPRITES: usize = 256;
/// Ticks each animation frame shows for (4 frames a second).
const TICKS_PER_FRAME: u64 = 5;
/// Floats per instance: cell (2), on-screen size (2), atlas origin (2),
/// mirror and quarter turns clockwise (2).
pub const INSTANCE_FLOATS: usize = 8;

/// Where one species' frames sit in the atlas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    origin: (usize, usize),
    /// Frame size in texels.
    size: (usize, usize),
    frames: usize,
}

/// The packed atlas image and where each species is in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Atlas {
    /// RGBA rows, [`ATLAS_WIDTH`] texels wide.
    pub image: Vec<u8>,
    /// Size in texels.
    pub size: (u32, u32),
    /// By species id.
    entries: Vec<Option<Entry>>,
}

impl Atlas {
    /// Packs every species' frames. Frames that don't fit the width are
    /// wrapped onto further rows of the same shelf.
    #[must_use]
    #[allow(clippy::many_single_char_names)] // texel maths
    pub fn build(species: &SpeciesTable) -> Self {
        let mut entries = vec![None; 256];
        let mut placed = Vec::new();
        let mut y = 0;
        for s in species.iter() {
            let (w, h) = (s.sprite.width, s.sprite.height);
            let per_row = (ATLAS_WIDTH / w).max(1);
            let n = s.sprite.frames.len();
            entries[usize::from(s.id.0)] = Some(Entry {
                origin: (0, y),
                size: (w, h),
                frames: n,
            });
            placed.push((s, y, per_row));
            y += h * n.div_ceil(per_row);
        }
        let height = y.max(1);
        let mut image = vec![0; ATLAS_WIDTH * height * 4];
        for (s, y0, per_row) in placed {
            let (w, h) = (s.sprite.width, s.sprite.height);
            for (f, texels) in s.sprite.frames.iter().enumerate() {
                let (fx, fy) = ((f % per_row) * w, y0 + (f / per_row) * h);
                for (i, t) in texels.iter().enumerate() {
                    let (x, y) = (fx + i % w, fy + i / w);
                    let o = (y * ATLAS_WIDTH + x) * 4;
                    image[o..o + 4].copy_from_slice(t);
                }
            }
        }
        let side = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        Self {
            image,
            size: (side(ATLAS_WIDTH), side(height)),
            entries,
        }
    }

    /// Atlas texel of frame `frame`'s top-left for species `id`.
    fn frame_origin(e: &Entry, frame: usize) -> (usize, usize) {
        let per_row = (ATLAS_WIDTH / e.size.0).max(1);
        (
            e.origin.0 + (frame % per_row) * e.size.0,
            e.origin.1 + (frame / per_row) * e.size.1,
        )
    }

    /// One instance per creature (up to [`MAX_SPRITES`]), as
    /// [`INSTANCE_FLOATS`] floats each, into `out`. Returns the count.
    pub fn instances(&self, world: &World, out: &mut Vec<f32>) -> u32 {
        out.clear();
        let tick = world.tick_count();
        let mut n = 0;
        for c in world.creatures().iter().take(MAX_SPRITES) {
            let Some(e) = self.entries[usize::from(c.species().0)] else {
                continue;
            };
            let phase = tick / TICKS_PER_FRAME + u64::from(c.id().0);
            let frame = usize::try_from(phase % e.frames as u64).unwrap_or(0);
            let (ox, oy) = Self::frame_origin(&e, frame);
            let (cx, cy) = c.centre();
            // On-screen size in cells: the body, half the frame's texels.
            let (w, h) = (e.size.0 / TEXELS_PER_CELL, e.size.1 / TEXELS_PER_CELL);
            #[allow(clippy::cast_precision_loss)] // sprite sides and atlas texels are small
            let (w, h, ox, oy) = (w as f32, h as f32, ox as f32, oy as f32);
            let turn = c.turn() % 4;
            let (w, h) = if turn % 2 == 1 { (h, w) } else { (w, h) };
            let flip = if c.facing_left() { 1.0 } else { 0.0 };
            out.extend_from_slice(&[
                cx - w / 2.0,
                cy - h / 2.0,
                w,
                h,
                ox,
                oy,
                flip,
                f32::from(turn),
            ]);
            n += 1;
        }
        n
    }
}

/// Pixel `(x, y)` of an 8×8 palette icon for species `id`: its hand-drawn
/// icon centred, `None` where transparent.
#[allow(clippy::many_single_char_names)] // pixel maths
#[must_use]
pub fn icon(
    species: &SpeciesTable,
    id: desktank_sim::SpeciesId,
    x: u32,
    y: u32,
) -> Option<[u8; 3]> {
    let icon = &species.get(id)?.sprite.icon;
    let (w, h) = (icon.width, icon.height);
    let (ox, oy) = (8_usize.saturating_sub(w) / 2, 8_usize.saturating_sub(h) / 2);
    let (x, y) = (usize::try_from(x).ok()?, usize::try_from(y).ok()?);
    let (ix, iy) = (x.checked_sub(ox)?, y.checked_sub(oy)?);
    if ix >= w || iy >= h {
        return None;
    }
    let p = icon.pixels[iy * w + ix];
    (p[3] > 0).then_some([p[0], p[1], p[2]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktank_sim::{MaterialTable, WorldConfig};

    fn world() -> World {
        let config = WorldConfig {
            width: 64,
            height: 48,
            seed: 1,
            ..WorldConfig::default()
        };
        World::new(config, MaterialTable::builtin().unwrap()).unwrap()
    }

    #[test]
    #[allow(clippy::many_single_char_names)]
    fn packs_every_frame_where_the_instances_point() {
        let mut w = world();
        let atlas = Atlas::build(w.species());
        assert_eq!(
            atlas.image.len(),
            (atlas.size.0 * atlas.size.1 * 4) as usize
        );
        let tetra = w.species().id("neon_tetra").unwrap();
        w.add_creature(tetra, 30, 30).unwrap();
        let mut out = Vec::new();
        assert_eq!(atlas.instances(&w, &mut out), 1);
        assert_eq!(out.len(), INSTANCE_FLOATS);
        // The frame the instance names holds the species' texels.
        let s = w.species().get(tetra).unwrap();
        let frame = usize::try_from(w.creatures()[0].id().0).unwrap() % s.sprite.frames.len();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (ox, oy) = (out[4] as usize, out[5] as usize);
        for (i, t) in s.sprite.frames[frame].iter().enumerate() {
            let (x, y) = (ox + i % s.sprite.width, oy + i / s.sprite.width);
            let o = (y * ATLAS_WIDTH + x) * 4;
            assert_eq!(&atlas.image[o..o + 4], t);
        }
        // Centred on the body.
        let (cx, cy) = w.creatures()[0].centre();
        assert!((out[0] + out[2] / 2.0 - cx).abs() < 1e-6);
        assert!((out[1] + out[3] / 2.0 - cy).abs() < 1e-6);
    }

    #[test]
    fn a_crawler_on_the_glass_is_turned_with_its_quad_swapped() {
        let mut w = world();
        let atlas = Atlas::build(w.species());
        let nerite = w.species().id("nerite").unwrap();
        // Against the left glass: the foot turns to point left.
        w.add_creature(nerite, 2, 30).unwrap();
        for _ in 0..20 {
            w.tick();
        }
        let c = &w.creatures()[0];
        assert_eq!(c.turn(), 1);
        let mut out = Vec::new();
        atlas.instances(&w, &mut out);
        let (fw, fh) = w.species().get(nerite).unwrap().size();
        // Quads are in cells: the body, half the sprite's texels.
        let sprite = &w.species().get(nerite).unwrap().sprite;
        assert_eq!((fw * 2, fh * 2), (sprite.width, sprite.height));
        #[allow(clippy::cast_precision_loss)]
        let swapped = [fh as f32, fw as f32];
        assert_eq!(out[2..4], swapped);
        assert!((out[7] - 1.0).abs() < f32::EPSILON);
        // The quad covers the turned body exactly.
        let b = w.creature_body(c);
        #[allow(clippy::cast_precision_loss)]
        let corner = [b.x0 as f32, b.y0 as f32];
        assert_eq!(out[0..2], corner);
    }

    #[test]
    fn icons_centre_the_hand_drawn_icon() {
        let w = world();
        for name in ["neon_tetra", "corydoras", "betta", "nerite"] {
            let id = w.species().id(name).unwrap();
            let art = &w.species().get(id).unwrap().sprite.icon;
            let (ox, oy) = ((8 - art.width) / 2, (8 - art.height) / 2);
            for y in 0..8_u32 {
                for x in 0..8_u32 {
                    let (ix, iy) = (x as usize, y as usize);
                    let want =
                        (ix >= ox && iy >= oy && ix - ox < art.width && iy - oy < art.height)
                            .then(|| art.pixels[(iy - oy) * art.width + ix - ox])
                            .filter(|p| p[3] > 0)
                            .map(|p| [p[0], p[1], p[2]]);
                    assert_eq!(icon(w.species(), id, x, y), want, "{name} ({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn the_shader_draws_the_same_texels_per_cell() {
        let want = format!("const TEXELS_PER_CELL: f32 = {TEXELS_PER_CELL}.0;");
        assert!(include_str!("tank.wgsl").contains(&want));
    }
}
