//! The creature sprite atlas and per-frame sprite instances (SPEC §4.2).
//!
//! Every species' frames are packed side by side, one shelf per species,
//! into one RGBA texture (sRGB-encoded, read raw with `textureLoad` like
//! the palette). Each frame, every creature becomes one instance: its
//! top-left cell, on-screen size, atlas frame, mirror and quarter turns,
//! drawn as a quad by the sprite pipeline at the grid's cell scale, with
//! [`desktank_sim::TEXELS_PER_CELL`] texels per cell side for an adult
//! (half a cell per texel). A crawler on a wall is turned a quarter, so its
//! quad swaps width and height and the shader turns the texel lookup;
//! quarter turns keep the pixel art crisp. WebGL2-safe: a per-instance
//! vertex buffer, no storage buffers.
//!
//! Life stages (T9): a growing fry or juvenile is drawn at its smaller body
//! size from the full frame, an elder a little greyer. Corpses follow the
//! creatures, belly up on their first frame, desaturated and losing texels
//! as they rot.

use desktank_sim::{LifeStage, SpeciesTable, World};

/// Atlas width in texels; species shelves stack downwards.
pub const ATLAS_WIDTH: usize = 256;
/// Most sprites drawn in a frame (the population cap is lower).
pub const MAX_SPRITES: usize = 256;
/// Ticks each animation frame shows for (4 frames a second).
const TICKS_PER_FRAME: u64 = 5;
/// Floats per instance: cell (2), on-screen size (2), atlas origin (2),
/// mirror and quarter turns clockwise (2), the frame's size in texels (2),
/// and the look: 0 alive, 1 elder, 2 dead, then how far a corpse has
/// rotted (2).
pub const INSTANCE_FLOATS: usize = 12;
/// Look values (the instance's eleventh float).
pub const LOOK_ALIVE: f32 = 0.0;
/// An elder.
pub const LOOK_ELDER: f32 = 1.0;
/// A corpse.
pub const LOOK_DEAD: f32 = 2.0;

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

    /// One instance per creature, then per corpse (up to [`MAX_SPRITES`]
    /// in all), as [`INSTANCE_FLOATS`] floats each, into `out`. Returns the
    /// count.
    pub fn instances(&self, world: &World, out: &mut Vec<f32>) -> u32 {
        out.clear();
        let tick = world.tick_count();
        let mut n = 0;
        for c in world.creatures() {
            let Some(e) = self.entries[usize::from(c.species().0)] else {
                continue;
            };
            if n == MAX_SPRITES {
                return u32::try_from(n).unwrap_or(u32::MAX);
            }
            let phase = tick / TICKS_PER_FRAME + u64::from(c.id().0);
            let frame = usize::try_from(phase % e.frames as u64).unwrap_or(0);
            let look = if world.life_stage(c) == LifeStage::Elder {
                LOOK_ELDER
            } else {
                LOOK_ALIVE
            };
            let sprite = Sprite {
                centre: c.centre(),
                // On-screen size in cells: the body (half the frame's
                // texels for an adult, less while it grows).
                size: world.creature_size(c),
                origin: Self::frame_origin(&e, frame),
                frame: e.size,
                flip: c.facing_left(),
                turn: c.turn() % 4,
                look: [look, 0.0],
            };
            sprite.push(out);
            n += 1;
        }
        for c in world.corpses() {
            let Some(e) = self.entries[usize::from(c.species().0)] else {
                continue;
            };
            if n == MAX_SPRITES {
                break;
            }
            // Belly up: turned twice, mirrored back so it still faces the
            // way it did.
            let sprite = Sprite {
                centre: c.centre(),
                size: c.size(),
                origin: Self::frame_origin(&e, 0),
                frame: e.size,
                flip: !c.facing_left(),
                turn: 2,
                look: [LOOK_DEAD, c.decay()],
            };
            sprite.push(out);
            n += 1;
        }
        u32::try_from(n).unwrap_or(u32::MAX)
    }
}

/// One sprite instance before it is packed into floats.
struct Sprite {
    /// Body centre in cells.
    centre: (f32, f32),
    /// Upright body size in cells.
    size: (usize, usize),
    /// Atlas texel of the frame's top-left, and the frame's size in texels.
    origin: (usize, usize),
    frame: (usize, usize),
    flip: bool,
    turn: u8,
    look: [f32; 2],
}

impl Sprite {
    fn push(&self, out: &mut Vec<f32>) {
        // Sprite sides and atlas texels are small: exact in f32.
        #[allow(clippy::cast_precision_loss)]
        let f = |v: usize| v as f32;
        let (w, h) = (f(self.size.0), f(self.size.1));
        let (w, h) = if self.turn % 2 == 1 { (h, w) } else { (w, h) };
        let (cx, cy) = self.centre;
        out.extend_from_slice(&[
            cx - w / 2.0,
            cy - h / 2.0,
            w,
            h,
            f(self.origin.0),
            f(self.origin.1),
            if self.flip { 1.0 } else { 0.0 },
            f32::from(self.turn),
            f(self.frame.0),
            f(self.frame.1),
            self.look[0],
            self.look[1],
        ]);
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
        assert!(include_str!("tank.wgsl").contains("@location(5) look: vec2<f32>"));
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
    #[allow(clippy::float_cmp, clippy::cast_precision_loss)] // small exact values
    fn fry_are_drawn_small_elders_grey_and_corpses_belly_up() {
        let mut w = world();
        let atlas = Atlas::build(w.species());
        let tetra = w.species().id("neon_tetra").unwrap();
        let s = w.species().get(tetra).unwrap();
        let (full_w, full_h) = s.size();
        let frame = [s.sprite.width as f32, s.sprite.height as f32];
        let fry = w.add_creature_aged(tetra, 20, 30, 0).unwrap();
        let old = w.add_creature(tetra, 40, 30).unwrap();
        let span = w.creature_lifespan(w.creature(old).unwrap());
        w.set_creature_age(old, span * 9 / 10);
        let dying = w.add_creature(tetra, 50, 20).unwrap();
        w.set_creature_age(dying, span * 2);
        w.tick();
        assert!(w.creature(dying).is_none());
        let mut out = Vec::new();
        assert_eq!(atlas.instances(&w, &mut out), 3);
        let (fry_i, old_i, dead_i) = (&out[0..12], &out[12..24], &out[24..36]);
        // The fry: its smaller body on screen, the whole frame sampled.
        let (bw, bh) = w.creature_size(w.creature(fry).unwrap());
        assert!(bw < full_w && bh <= full_h);
        assert_eq!(fry_i[2..4], [bw as f32, bh as f32]);
        assert_eq!(fry_i[8..10], frame);
        assert_eq!(fry_i[10], LOOK_ALIVE);
        assert_eq!(old_i[10], LOOK_ELDER);
        assert_eq!(old_i[2..4], [full_w as f32, full_h as f32]);
        // The corpse: full size, upside down on frame 0, not yet rotting.
        assert_eq!(dead_i[7], 2.0);
        assert_eq!(dead_i[10], LOOK_DEAD);
        assert_eq!(dead_i[11], 0.0);
        let e = atlas.entries[usize::from(tetra.0)].unwrap();
        assert_eq!(dead_i[4..6], [e.origin.0 as f32, e.origin.1 as f32]);
    }
}
