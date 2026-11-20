//! The creature sprite atlas and per-frame sprite instances (SPEC §4.2).
//!
//! Every species' frames are packed side by side, two shelves per look
//! (each colour morph, and each sex for a species drawn differently by
//! sex, T9 part 2: its frames, then its fine frames at twice the size),
//! into one RGBA
//! texture (sRGB-encoded, read raw with `textureLoad` like
//! the palette). Each frame, every creature becomes one instance: its
//! top-left cell, on-screen size, atlas frame, mirror and quarter turns,
//! drawn as a quad by the sprite pipeline at the grid's cell scale, with
//! [`desktank_sim::TEXELS_PER_CELL`] texels per cell side for an adult
//! (half a cell per texel), or from the fine frames at four texels per cell
//! side when the display scale is divisible by four. A crawler on a wall
//! is turned a quarter, so its quad swaps width and height and the shader
//! turns the texel lookup; quarter turns keep the pixel art crisp. WebGL2-safe: a per-instance
//! vertex buffer, no storage buffers.
//!
//! Life stages (T9): a growing fry or juvenile is drawn at its smaller body
//! size from the full frame, an elder a little greyer. Corpses follow the
//! creatures, belly up on their first frame, desaturated and losing texels
//! as they rot. Each creature or corpse is drawn in its look: the morph it
//! shows, for its sex.
//!
//! Night glow: a species with a `[glow]` (the blinker) gives each of its
//! instances its glow colour and strength (times a slow pulse); the shader
//! adds it after the daylight tint, scaled by the darkness, rimming the
//! outline and spilling a short halo past the frame. The atlas marks the
//! texels that glow differently in their alpha (eye whites [`ALPHA_EYE`],
//! pupils [`ALPHA_UNLIT`]; still opaque to every other test).

use desktank_sim::{LifeStage, SpeciesId, SpeciesTable, TEXELS_PER_CELL, TICKS_PER_SECOND, World};

/// Atlas width in texels; species shelves stack downwards. 512 wide keeps
/// the stack within WebGL2's guaranteed 2048 texels tall now that species
/// carry several colour morphs of large art (the axolotl alone has three).
pub const ATLAS_WIDTH: usize = 512;
/// Most sprites drawn in a frame: every creature the cap allows, live
/// food and corpses.
pub const MAX_SPRITES: usize =
    desktank_sim::MAX_CREATURE_CAP + desktank_sim::MAX_LIVE_FOOD + desktank_sim::MAX_CORPSES;
/// Ticks each animation frame shows for (4 frames a second).
const TICKS_PER_FRAME: u64 = 5;
/// Floats per instance: cell (2), on-screen size (2), atlas origin (2),
/// mirror and quarter turns clockwise (2), the frame's size in texels (2),
/// the look: 0 alive, 1 elder, 2 dead, then how far a corpse has rotted
/// (2), the fine frame's atlas origin (2), or -1 for a body drawn
/// smaller than its frame, the depth in bands (1, D21), and the night glow
/// (4): its colour (sRGB-encoded) and strength times the pulse, 0 for none.
pub const INSTANCE_FLOATS: usize = 19;
/// Atlas alpha of a glowing species' eye-white texel: it glows brighter
/// than the body.
pub const ALPHA_EYE: u8 = 254;
/// Atlas alpha of a glowing species' texel that doesn't glow (a pupil).
pub const ALPHA_UNLIT: u8 = 253;
/// Ticks in one slow glow pulse (6 seconds).
const PULSE_TICKS: u64 = 6 * TICKS_PER_SECOND as u64;
/// The pulse's dimmest point, as a fraction of full glow.
const PULSE_LOW: f32 = 0.82;
/// Look values (the instance's eleventh float).
pub const LOOK_ALIVE: f32 = 0.0;
/// An elder.
pub const LOOK_ELDER: f32 = 1.0;
/// A corpse.
pub const LOOK_DEAD: f32 = 2.0;

/// Where one look's frames sit in the atlas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    origin: (usize, usize),
    /// Top-left of the first fine frame (twice the size).
    fine: (usize, usize),
    /// Frame size in texels.
    size: (usize, usize),
    frames: usize,
}

/// The packed atlas image and where each species' looks are in it.
#[derive(Clone, Debug, PartialEq)]
pub struct Atlas {
    /// RGBA rows, [`ATLAS_WIDTH`] texels wide.
    pub image: Vec<u8>,
    /// Size in texels.
    pub size: (u32, u32),
    /// By species id, then look ([`desktank_sim::Species::look`]).
    entries: Vec<Vec<Entry>>,
    /// By species id: the glow colour (sRGB-encoded, 0–1), strength and
    /// whether it pulses, for species that glow.
    glows: Vec<Option<([f32; 3], f32, bool)>>,
}

impl Atlas {
    /// Packs every species' frames. Frames that don't fit the width are
    /// wrapped onto further rows of the same shelf.
    #[must_use]
    #[allow(clippy::many_single_char_names)] // texel maths
    pub fn build(species: &SpeciesTable) -> Self {
        let mut entries = vec![Vec::new(); 256];
        let mut glows = vec![None; 256];
        let mut placed = Vec::new();
        let mut y = 0;
        // Rows a shelf of `n` frames of `w × h` texels takes.
        let rows = |n: usize, (w, h): (usize, usize)| h * n.div_ceil((ATLAS_WIDTH / w).max(1));
        for s in species.iter() {
            let size = (s.sprite.width, s.sprite.height);
            glows[usize::from(s.id.0)] = s
                .glow
                .as_ref()
                .map(|g| (g.colour.map(|c| f32::from(c) / 255.0), g.strength, g.pulse));
            for look in &s.looks {
                let n = look.frames.len();
                let origin = (0, y);
                y += rows(n, size);
                let fine = (0, y);
                y += rows(n, (size.0 * 2, size.1 * 2));
                entries[usize::from(s.id.0)].push(Entry {
                    origin,
                    fine,
                    size,
                    frames: n,
                });
                placed.push((look, size, origin, fine, s.glow.as_ref()));
            }
        }
        let height = y.max(1);
        let mut image = vec![0; ATLAS_WIDTH * height * 4];
        for (look, (w, h), origin, fine, glow) in placed {
            for (frames, origin, size) in [
                (&look.frames, origin, (w, h)),
                (&look.fine_frames, fine, (w * 2, h * 2)),
            ] {
                for (f, texels) in frames.iter().enumerate() {
                    let (fx, fy) = Self::frame_origin(origin, size, f);
                    for (i, t) in texels.iter().enumerate() {
                        let (x, y) = (fx + i % size.0, fy + i / size.0);
                        let o = (y * ATLAS_WIDTH + x) * 4;
                        image[o..o + 4].copy_from_slice(t);
                        if let Some(g) = glow
                            && t[3] > 0
                        {
                            let rgb = [t[0], t[1], t[2]];
                            if g.unlit.contains(&rgb) {
                                image[o + 3] = ALPHA_UNLIT;
                            } else if g.eyes.contains(&rgb) {
                                image[o + 3] = ALPHA_EYE;
                            }
                        }
                    }
                }
            }
        }
        let side = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        Self {
            image,
            size: (side(ATLAS_WIDTH), side(height)),
            entries,
            glows,
        }
    }

    /// Where `species`' look for `morph` and sex is, if anywhere.
    fn entry(&self, world: &World, species: SpeciesId, morph: u8, female: bool) -> Option<Entry> {
        let look = world.species().get(species)?.look(morph, female);
        self.entries[usize::from(species.0)].get(look).copied()
    }

    /// Atlas texel of frame `frame`'s top-left in the shelf at `origin` of
    /// `w × h`-texel frames.
    fn frame_origin(
        origin: (usize, usize),
        (w, h): (usize, usize),
        frame: usize,
    ) -> (usize, usize) {
        let per_row = (ATLAS_WIDTH / w).max(1);
        (
            origin.0 + (frame % per_row) * w,
            origin.1 + (frame / per_row) * h,
        )
    }

    /// The base and fine atlas origins of frame `frame` of entry `e`.
    fn origins(e: &Entry, frame: usize) -> ((usize, usize), (usize, usize)) {
        (
            Self::frame_origin(e.origin, e.size, frame),
            Self::frame_origin(e.fine, (e.size.0 * 2, e.size.1 * 2), frame),
        )
    }

    /// One instance per creature, then per corpse (up to [`MAX_SPRITES`]
    /// in all), as [`INSTANCE_FLOATS`] floats each, into `out`, back to
    /// front: deepest first, so nearer creatures are drawn over further
    /// ones (D21; ties keep the world's order, creatures before corpses).
    /// `sprites` is scratch space, reused between frames. Returns the
    /// count.
    pub fn instances(&self, world: &World, sprites: &mut Vec<Sprite>, out: &mut Vec<f32>) -> u32 {
        out.clear();
        sprites.clear();
        let tick = world.tick_count();
        for c in world.creatures() {
            let Some(e) = self.entry(world, c.species(), world.creature_morph(c), c.female())
            else {
                continue;
            };
            if sprites.len() == MAX_SPRITES {
                break;
            }
            let phase = tick / TICKS_PER_FRAME + u64::from(c.id().0);
            let frame = usize::try_from(phase % e.frames as u64).unwrap_or(0);
            let look = if world.life_stage(c) == LifeStage::Elder {
                LOOK_ELDER
            } else {
                LOOK_ALIVE
            };
            let (origin, fine) = Self::origins(&e, frame);
            let glow =
                self.glows[usize::from(c.species().0)].map_or([0.0; 4], |(rgb, k, pulse)| {
                    let pulse = if pulse {
                        glow_pulse(tick + 37 * u64::from(c.id().0))
                    } else {
                        1.0
                    };
                    [rgb[0], rgb[1], rgb[2], k * pulse]
                });
            sprites.push(Sprite {
                centre: c.centre(),
                // On-screen size in cells: the body (half the frame's
                // texels for an adult, less while it grows).
                size: world.creature_size(c),
                origin,
                fine,
                frame: e.size,
                flip: c.facing_left(),
                turn: c.turn() % 4,
                look: [look, 0.0],
                depth: c.depth(),
                glow,
            });
        }
        for c in world.corpses() {
            let Some(e) = self.entry(world, c.species(), c.morph(), c.female()) else {
                continue;
            };
            if sprites.len() == MAX_SPRITES {
                break;
            }
            // Belly up: turned twice, mirrored back so it still faces the
            // way it did.
            let (origin, fine) = Self::origins(&e, 0);
            sprites.push(Sprite {
                centre: c.centre(),
                size: c.size(),
                origin,
                fine,
                frame: e.size,
                flip: !c.facing_left(),
                turn: 2,
                look: [LOOK_DEAD, c.decay()],
                depth: f32::from(c.band()),
                // The dead don't glow.
                glow: [0.0; 4],
            });
        }
        // Stable, so equal depths keep the world's order.
        sprites.sort_by(|a, b| b.depth.total_cmp(&a.depth));
        for s in sprites.iter() {
            s.push(out);
        }
        u32::try_from(sprites.len()).unwrap_or(u32::MAX)
    }
}

/// One sprite instance before it is packed into floats.
#[derive(Clone, Copy, Debug)]
pub struct Sprite {
    /// Body centre in cells.
    centre: (f32, f32),
    /// Upright body size in cells.
    size: (usize, usize),
    /// Atlas texel of the frame's top-left, its fine frame's top-left, and
    /// the frame's size in texels.
    origin: (usize, usize),
    fine: (usize, usize),
    frame: (usize, usize),
    flip: bool,
    turn: u8,
    look: [f32; 2],
    /// Depth in bands from the front glass (D21).
    depth: f32,
    /// Night glow colour and strength (0 for none).
    glow: [f32; 4],
}

impl Sprite {
    fn push(&self, out: &mut Vec<f32>) {
        // Sprite sides and atlas texels are small: exact in f32.
        #[allow(clippy::cast_precision_loss)]
        let f = |v: usize| v as f32;
        let (w, h) = (f(self.size.0), f(self.size.1));
        let (w, h) = if self.turn % 2 == 1 { (h, w) } else { (w, h) };
        let (cx, cy) = self.centre;
        // Fine texels are a quarter cell only on a full-size body; a growing
        // creature samples the coarser frame, which drops less of its art.
        let full = self.size.0 * TEXELS_PER_CELL == self.frame.0
            && self.size.1 * TEXELS_PER_CELL == self.frame.1;
        let fine = if full {
            [f(self.fine.0), f(self.fine.1)]
        } else {
            [-1.0; 2]
        };
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
            fine[0],
            fine[1],
            self.depth,
        ]);
        out.extend_from_slice(&self.glow);
    }
}

/// The slow glow pulse at `tick`: a sine between [`PULSE_LOW`] and 1 over
/// [`PULSE_TICKS`]. It follows the world clock, so a paused tank holds
/// still and needs no redraw.
#[must_use]
pub fn glow_pulse(tick: u64) -> f32 {
    // Under PULSE_TICKS: exact in f32.
    #[allow(clippy::cast_precision_loss)]
    let phase = (tick % PULSE_TICKS) as f32 / PULSE_TICKS as f32;
    let mid = f32::midpoint(1.0, PULSE_LOW);
    mid + (1.0 - mid) * (phase * std::f32::consts::TAU).sin()
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
    use desktank_sim::{Creature, MaterialTable, WorldConfig};

    fn world() -> World {
        let config = WorldConfig {
            width: 64,
            height: 48,
            seed: 1,
            ..WorldConfig::default()
        };
        World::new(config, MaterialTable::builtin().unwrap()).unwrap()
    }

    /// The instance in `out` centred on `centre` (instances are drawn back
    /// to front, not in creature order).
    fn instance_at(out: &[f32], centre: (f32, f32)) -> &[f32] {
        out.chunks(INSTANCE_FLOATS)
            .find(|i| {
                (i[0] + i[2] / 2.0 - centre.0).abs() < 1e-4
                    && (i[1] + i[3] / 2.0 - centre.1).abs() < 1e-4
            })
            .unwrap_or_else(|| panic!("no instance centred on {centre:?}"))
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
        assert_eq!(atlas.instances(&w, &mut Vec::new(), &mut out), 1);
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

    /// T9 part 2: each creature and corpse samples its own look (morph and
    /// sex), and the atlas stays within WebGL2's smallest texture limit.
    #[test]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn creatures_are_drawn_in_their_morph_and_sex() {
        let mut w = world();
        let atlas = Atlas::build(w.species());
        assert!(atlas.size.1 <= 2048, "{:?}", atlas.size);
        let (shrimp, guppy) = (
            w.species().id("cherry_shrimp").unwrap(),
            w.species().id("guppy").unwrap(),
        );
        let blue = w.species().get(shrimp).unwrap().morph("blue").unwrap();
        let red = w.species().get(guppy).unwrap().morph("red").unwrap();
        let add = |w: &mut World, s, x, female, m: u8| {
            let traits = desktank_sim::Traits {
                female: Some(female),
                alleles: Some([m, m]),
                ..desktank_sim::Traits::default()
            };
            w.add_creature_with_traits(s, x, 40, traits).unwrap()
        };
        add(&mut w, shrimp, 10, false, blue);
        add(&mut w, guppy, 30, false, red);
        add(&mut w, guppy, 50, true, red);
        let dying = add(&mut w, shrimp, 20, true, blue);
        let span = w.creature_lifespan(w.creature(dying).unwrap());
        w.set_creature_age(dying, span);
        w.tick();
        let mut out = Vec::new();
        assert_eq!(atlas.instances(&w, &mut Vec::new(), &mut out), 4);
        let centres: Vec<_> = w
            .creatures()
            .iter()
            .map(Creature::centre)
            .chain(w.corpses().iter().map(desktank_sim::Corpse::centre))
            .collect();
        let texel = |i: usize| {
            let inst = instance_at(&out, centres[i]);
            let (ox, oy) = (inst[4] as usize, inst[5] as usize);
            let o = (oy * ATLAS_WIDTH + ox) * 4;
            (ox, oy, o)
        };
        // Each instance's frame is its look's (frame by id and tick).
        let looks = [
            (shrimp, w.species().get(shrimp).unwrap().look(blue, false)),
            (guppy, w.species().get(guppy).unwrap().look(red, false)),
            (guppy, w.species().get(guppy).unwrap().look(red, true)),
            (shrimp, w.species().get(shrimp).unwrap().look(blue, true)),
        ];
        for (i, (s, look)) in looks.into_iter().enumerate() {
            let sp = w.species().get(s).unwrap();
            let (ox, oy, _) = texel(i);
            let frames = &sp.looks[look].frames;
            let matches = frames.iter().any(|f| {
                f.iter().enumerate().all(|(j, t)| {
                    let (x, y) = (ox + j % sp.sprite.width, oy + j / sp.sprite.width);
                    let o = (y * ATLAS_WIDTH + x) * 4;
                    &atlas.image[o..o + 4] == t
                })
            });
            assert!(matches, "instance {i}");
        }
        // The male and female guppies sample different shelves.
        assert_ne!(texel(1).1, texel(2).1);
    }

    #[test]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn every_creature_has_a_second_frame_origin_at_double_density() {
        let mut world = World::new(
            WorldConfig {
                width: 192,
                height: 144,
                ..WorldConfig::default()
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let ids: Vec<_> = world.species().iter().map(|s| s.id).collect();
        for (i, &id) in (0_i64..).zip(&ids) {
            world
                .add_creature(id, 20 + 36 * (i % 5), 24 + 20 * (i / 5))
                .unwrap();
        }
        let atlas = Atlas::build(world.species());
        let mut out = Vec::new();
        assert_eq!(
            atlas.instances(&world, &mut Vec::new(), &mut out) as usize,
            ids.len()
        );
        assert!(include_str!("tank.wgsl").contains("@location(6) fine_origin: vec2<f32>"));
        for c in world.creatures() {
            let inst = instance_at(&out, c.centre());
            let s = world.species().get(c.species()).unwrap();
            let look = &s.looks[s.look(world.creature_morph(c), c.female())];
            let frame = c.id().0 as usize % look.fine_frames.len();
            let (ox, oy) = (inst[12] as usize, inst[13] as usize);
            let width = s.sprite.width * 2;
            for (i, texel) in look.fine_frames[frame].iter().enumerate() {
                let offset = ((oy + i / width) * ATLAS_WIDTH + ox + i % width) * 4;
                // The colour, and opaque where it is (a glowing species'
                // alpha marks its eyes and pupils).
                let at = &atlas.image[offset..offset + 4];
                assert_eq!((&at[..3], at[3] > 0), (&texel[..3], texel[3] > 0));
            }
        }
    }

    /// The blinker's frames fit the atlas (two fine frames a row), its eye
    /// whites and pupils are marked in their alpha, and its instances carry
    /// its glow, pulsing; no other creature, nor its corpse, glows.
    #[test]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::many_single_char_names
    )]
    fn the_blinker_glows_and_marks_its_eyes() {
        let mut w = World::new(
            WorldConfig {
                width: 120,
                height: 60,
                seed: 1,
                ..WorldConfig::default()
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let atlas = Atlas::build(w.species());
        assert!(atlas.size.1 <= 2048, "{:?}", atlas.size);
        let blinker = w.species().id("blinker").unwrap();
        let tetra = w.species().id("neon_tetra").unwrap();
        let b = w.add_creature(blinker, 30, 25).unwrap();
        w.add_creature(tetra, 90, 25).unwrap();
        let e = atlas.entries[usize::from(blinker.0)][0];
        assert_eq!(e.size, (60, 44));
        // Every frame's texels: eyes and pupils marked, the rest opaque.
        let s = w.species().get(blinker).unwrap();
        let glow = s.glow.as_ref().unwrap();
        let (mut eyes, mut unlit) = (0, 0);
        for f in 0..e.frames {
            let (base, fine) = Atlas::origins(&e, f);
            for ((ox, oy), width, texels) in [
                (base, 60, &s.sprite.frames[f]),
                (fine, 120, &s.sprite.fine_frames[f]),
            ] {
                assert!(ox + width <= ATLAS_WIDTH);
                for (i, t) in texels.iter().enumerate() {
                    let o = ((oy + i / width) * ATLAS_WIDTH + ox + i % width) * 4;
                    let rgb = [t[0], t[1], t[2]];
                    let want = match t[3] {
                        0 => 0,
                        _ if glow.unlit.contains(&rgb) => ALPHA_UNLIT,
                        _ if glow.eyes.contains(&rgb) => ALPHA_EYE,
                        _ => 255,
                    };
                    eyes += usize::from(want == ALPHA_EYE);
                    unlit += usize::from(want == ALPHA_UNLIT);
                    assert_eq!(atlas.image[o + 3], want);
                }
            }
        }
        assert!(eyes > 0 && unlit > 0);
        // No other species is marked.
        let t = atlas.entries[usize::from(tetra.0)][0];
        for y in t.origin.1..t.fine.1 {
            for x in 0..ATLAS_WIDTH {
                let a = atlas.image[(y * ATLAS_WIDTH + x) * 4 + 3];
                assert!(a == 0 || a == 255, "({x}, {y}): {a}");
            }
        }
        let mut out = Vec::new();
        assert_eq!(atlas.instances(&w, &mut Vec::new(), &mut out), 2);
        let bi = instance_at(&out, w.creature(b).unwrap().centre()).to_vec();
        let colour = glow.colour.map(|c| f32::from(c) / 255.0);
        assert_eq!(bi[15..18], colour);
        assert!((0.35 * PULSE_LOW - 1e-4..=0.35 + 1e-4).contains(&bi[18]));
        let ti = instance_at(&out, w.creatures()[1].centre());
        assert_eq!(ti[15..19], [0.0; 4]);
        // The pulse: within 82–100% over its period.
        let pulses: Vec<f32> = (0..PULSE_TICKS).map(glow_pulse).collect();
        let (lo, hi) = pulses
            .iter()
            .fold((1.0_f32, 0.0_f32), |(l, h), &p| (l.min(p), h.max(p)));
        assert!(
            (lo - PULSE_LOW).abs() < 1e-3 && (hi - 1.0).abs() < 1e-3,
            "{lo} {hi}"
        );
        assert!((glow_pulse(0) - glow_pulse(PULSE_TICKS)).abs() < 1e-6);
        // Dead, it doesn't glow.
        let span = w.creature_lifespan(w.creature(b).unwrap());
        w.set_creature_age(b, span);
        w.tick();
        assert_eq!(w.corpses().len(), 1);
        atlas.instances(&w, &mut Vec::new(), &mut out);
        let dead = instance_at(&out, w.corpses()[0].centre());
        assert_eq!(dead[15..19], [0.0; 4]);
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
        atlas.instances(&w, &mut Vec::new(), &mut out);
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
        assert_eq!(atlas.instances(&w, &mut Vec::new(), &mut out), 3);
        let (fry_i, old_i, dead_i) = (
            instance_at(&out, w.creature(fry).unwrap().centre()),
            instance_at(&out, w.creature(old).unwrap().centre()),
            instance_at(&out, w.corpses()[0].centre()),
        );
        // The fry: its smaller body on screen, the whole frame sampled.
        let (bw, bh) = w.creature_size(w.creature(fry).unwrap());
        assert!(bw < full_w && bh <= full_h);
        assert_eq!(fry_i[2..4], [bw as f32, bh as f32]);
        assert_eq!(fry_i[8..10], frame);
        assert_eq!(fry_i[12..14], [-1.0; 2], "a fry samples the coarse frame");
        assert!(old_i[12] >= 0.0);
        assert_eq!(fry_i[10], LOOK_ALIVE);
        assert_eq!(old_i[10], LOOK_ELDER);
        assert_eq!(old_i[2..4], [full_w as f32, full_h as f32]);
        // The corpse: full size, upside down on frame 0, not yet rotting.
        assert_eq!(dead_i[7], 2.0);
        assert_eq!(dead_i[10], LOOK_DEAD);
        assert_eq!(dead_i[11], 0.0);
        let e = atlas.entries[usize::from(tetra.0)][0];
        assert_eq!(dead_i[4..6], [e.origin.0 as f32, e.origin.1 as f32]);
        assert_eq!(dead_i[12..14], [e.fine.0 as f32, e.fine.1 as f32]);
    }

    /// D21: sprites are drawn back to front, so a nearer creature covers a
    /// further one, and each instance carries its depth for the shader.
    #[test]
    fn creatures_are_drawn_back_to_front() {
        let mut w = world();
        let atlas = Atlas::build(w.species());
        let tetra = w.species().id("neon_tetra").unwrap();
        for i in 0..12 {
            w.add_creature(tetra, 6 + 5 * (i % 6), 12 + 14 * (i / 6))
                .unwrap();
        }
        for _ in 0..200 {
            w.tick();
        }
        let mut out = Vec::new();
        assert_eq!(atlas.instances(&w, &mut Vec::new(), &mut out), 12);
        let depths: Vec<f32> = out.chunks(INSTANCE_FLOATS).map(|i| i[14]).collect();
        assert!(
            depths.windows(2).all(|d| d[0] >= d[1]),
            "deepest first: {depths:?}"
        );
        let mut want: Vec<f32> = w.creatures().iter().map(Creature::depth).collect();
        want.sort_by(|a, b| b.total_cmp(a));
        assert_eq!(depths, want);
        assert!(
            want.first() > want.last(),
            "the tetras are at several depths: {want:?}"
        );
        assert!(include_str!("tank.wgsl").contains("@location(7) depth: f32"));
    }
}
