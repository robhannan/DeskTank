//! The creature sprite atlas and per-frame sprite instances (SPEC §4.2).
//!
//! Every species' frames are packed side by side, two shelves per look
//! (each colour morph, with optional female and carrying looks:
//! its frames, then its fine frames at twice the size),
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
//! shows, for its sex and, while alive, whether it carries young.
//!
//! Night glow: a species with a `[glow]` (the blinker) gives each of its
//! instances its glow colour and strength (times a slow pulse); the shader
//! adds it after the daylight tint, scaled by the darkness, rimming the
//! outline and spilling a short halo past the frame. The atlas marks the
//! texels that glow differently in their alpha (eye whites [`ALPHA_EYE`],
//! pupils [`ALPHA_UNLIT`]; still opaque to every other test).

use desktank_sim::{
    Creature, Device, DeviceKind, DeviceKinds, LifeStage, Pane, SpeciesId, SpeciesTable,
    TEXELS_PER_CELL, TICKS_PER_SECOND, World,
};

/// Atlas width in texels; species shelves stack downwards. 512 wide keeps
/// the stack within WebGL2's guaranteed 2048 texels tall now that species
/// carry several colour morphs of large art (the axolotl alone has three).
pub const ATLAS_WIDTH: usize = 512;
/// Maximum sprite slices reserved per device (and for the placement ghost),
/// covering telescoping tubes even in tall `HiDPI` tanks without displacing
/// creatures or corpses.
const MAX_DEVICE_SLICES: usize = 64;
/// Instance buffer capacity for creatures, food, corpses, devices and
/// the placement ghost.
pub const MAX_SPRITES: usize = desktank_sim::MAX_CREATURE_CAP
    + desktank_sim::MAX_LIVE_FOOD
    + desktank_sim::MAX_CORPSES
    + (desktank_sim::MAX_DEVICES + 1) * MAX_DEVICE_SLICES;
/// Ticks each animation frame shows for (4 frames a second).
const TICKS_PER_FRAME: u64 = 5;
/// Floats per instance: cell (2), on-screen size (2), atlas origin (2),
/// mirror and quarter turns clockwise (2), the frame's size in texels (2),
/// the look: 0 alive, 1 elder, 2 dead, then how far a corpse has rotted
/// or, alive, 1 for a burrower (2), the fine frame's atlas origin (2), or -1 for a body drawn
/// smaller than its frame, the depth in bands (1, D21), and the night glow
/// (4): its colour (sRGB-encoded) and strength times the pulse, 0 for none.
/// Last are the pitch's cosine and sine and the upright body size (4).
pub const INSTANCE_FLOATS: usize = 23;
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
/// A device's placement ghost (D27): drawn see-through, tinted red when
/// it doesn't fit (the look's second float is 1).
pub const LOOK_GHOST: f32 = 3.0;
/// The depth devices are drawn at, in bands (D21): just behind the front
/// band, so a creature or plant there passes in front of the glass-mounted
/// body while the rest of the tank is behind it.
pub const DEVICE_DEPTH: f32 = 1.0;
/// The look's second float for a living burrower (a corpse's is how far it
/// has rotted): hidden where loose substrate covers it.
pub const LOOK_BURROWER: f32 = 1.0;

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
    /// By device kind, in [`DeviceKind::ALL`] order (D27).
    devices: Vec<Entry>,
    /// By species id: the glow colour (sRGB-encoded, 0–1), strength and
    /// whether it pulses, for species that glow.
    glows: Vec<Option<([f32; 3], f32, bool)>>,
}

impl Atlas {
    /// Packs every species' frames and every device kind's (D27) onto
    /// shelves, tallest first. Frames that don't fit the width are wrapped
    /// onto further rows of the same strip.
    #[must_use]
    #[allow(clippy::many_single_char_names)] // texel maths
    pub fn build(species: &SpeciesTable, device_kinds: &DeviceKinds) -> Self {
        let mut entries = vec![Vec::new(); 256];
        let mut glows = vec![None; 256];
        // Every species' looks, then every device kind's art: frames, fine
        // frames, frame size, glow, and whose entry it is.
        let mut arts = Vec::new();
        for s in species.iter() {
            glows[usize::from(s.id.0)] = s
                .glow
                .as_ref()
                .map(|g| (g.colour.map(|c| f32::from(c) / 255.0), g.strength, g.pulse));
            // Its looks, then its pane looks (#371) after them.
            for look in s.looks.iter().chain(&s.pane_looks) {
                let owner = Some(usize::from(s.id.0));
                arts.push((
                    &look.frames,
                    &look.fine_frames,
                    look.size,
                    s.glow.as_ref(),
                    owner,
                ));
            }
        }
        for art in device_kinds.iter() {
            let size = (art.sprite.width, art.sprite.height);
            arts.push((
                &art.sprite.frames,
                &art.sprite.fine_frames,
                size,
                None,
                None,
            ));
        }
        // Shelf packing: the tallest strips first, so each row's strips are
        // of similar height (base and fine strips are packed separately).
        let mut strips: Vec<(usize, bool)> = (0..arts.len())
            .flat_map(|i| [(i, false), (i, true)])
            .collect();
        let strip_size = |&(i, fine): &(usize, bool)| {
            let (w, h) = arts[i].2;
            if fine { (w * 2, h * 2) } else { (w, h) }
        };
        strips.sort_by_key(|s| std::cmp::Reverse(strip_size(s).1));
        let mut origins = vec![((0, 0), (0, 0)); arts.len()];
        let mut shelf = Shelf::new();
        for strip in &strips {
            let at = shelf.alloc(arts[strip.0].0.len(), strip_size(strip));
            if strip.1 {
                origins[strip.0].1 = at;
            } else {
                origins[strip.0].0 = at;
            }
        }
        let mut devices = Vec::new();
        let mut placed = Vec::new();
        for (&(base, fine_frames, size, glow, owner), &(origin, fine)) in arts.iter().zip(&origins)
        {
            let entry = Entry {
                origin,
                fine,
                size,
                frames: base.len(),
            };
            match owner {
                Some(id) => entries[id].push(entry),
                None => devices.push(entry),
            }
            placed.push((base, fine_frames, size, origin, fine, glow));
        }
        let height = shelf.finish().max(1);
        let mut image = vec![0; ATLAS_WIDTH * height * 4];
        for (base, fine_frames, (w, h), origin, fine, glow) in placed {
            for (frames, origin, size) in
                [(base, origin, (w, h)), (fine_frames, fine, (w * 2, h * 2))]
            {
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
            devices,
            glows,
        }
    }

    /// Where `species`' look for this morph, sex and carrying state is.
    fn entry(
        &self,
        table: &SpeciesTable,
        species: SpeciesId,
        morph: u8,
        female: bool,
        carrying: bool,
    ) -> Option<Entry> {
        let look = table.get(species)?.look(morph, female, carrying);
        self.entries[usize::from(species.0)].get(look).copied()
    }

    /// Where a creature's look is: its pane look while it crawls the front
    /// or back pane (from below or above, #371), else as [`Atlas::entry`].
    fn creature_entry(&self, world: &World, c: &Creature) -> Option<Entry> {
        let table = world.species();
        let morph = world.creature_morph(c);
        if c.pane() != Pane::Off {
            let s = table.get(c.species())?;
            let look = s.pane_look(morph, c.pane() == Pane::Front)?;
            return self.entries[usize::from(c.species().0)]
                .get(s.looks.len() + look)
                .copied();
        }
        self.entry(table, c.species(), morph, c.female(), c.carrying() > 0)
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
    #[cfg(test)]
    pub fn instances(&self, world: &World, sprites: &mut Vec<Sprite>, out: &mut Vec<f32>) -> u32 {
        self.instances_with(world, None, sprites, out)
    }

    /// [`Atlas::instances`], then slices for each device and
    /// the placement `ghost` if any (whether it fits), drawn last.
    pub fn instances_with(
        &self,
        world: &World,
        ghost: Option<(Device, bool)>,
        sprites: &mut Vec<Sprite>,
        out: &mut Vec<f32>,
    ) -> u32 {
        out.clear();
        sprites.clear();
        // Give the ghost enough room for all its slices, then move it to
        // the end after sorting the placed sprites.
        if let Some((d, fits)) = ghost {
            self.push_device(world, &d, Some(fits), sprites);
        }
        let ghost_count = sprites.len();
        let tick = world.tick_count();
        for c in world.creatures() {
            let Some(e) = self.creature_entry(world, c) else {
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
                pitch: c.pitch(),
                look: [look, 0.0],
                depth: c.depth(),
                glow,
                burrower: world
                    .species()
                    .get(c.species())
                    .is_some_and(|s| s.burrow_every.is_some()),
            });
        }
        for c in world.corpses() {
            let Some(e) = self.entry(world.species(), c.species(), c.morph(), c.female(), false)
            else {
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
                pitch: 0,
                look: [LOOK_DEAD, c.decay()],
                depth: f32::from(c.band()),
                // The dead don't glow.
                glow: [0.0; 4],
                burrower: false,
            });
        }
        for d in world.devices() {
            self.push_device(world, d, None, sprites);
        }
        // Stable, so equal depths keep the world's order.
        sprites[ghost_count..].sort_by(|a, b| b.depth.total_cmp(&a.depth));
        sprites.rotate_left(ghost_count);
        for s in sprites.iter() {
            s.push(out);
        }
        u32::try_from(sprites.len()).unwrap_or(u32::MAX)
    }
}

impl Atlas {
    fn push_device(
        &self,
        world: &World,
        d: &Device,
        ghost: Option<bool>,
        sprites: &mut Vec<Sprite>,
    ) {
        let art = world.device_kinds().get(d.kind);
        let height = art.size.1;
        let span = d.span(art);
        let (look, depth) = match ghost {
            Some(fits) => ([LOOK_GHOST, if fits { 0.0 } else { 1.0 }], 0.0),
            None => ([LOOK_ALIVE, 0.0], DEVICE_DEPTH),
        };
        let available = MAX_SPRITES - sprites.len();
        let mut push = |rows, world_y, depth| {
            sprites.push(self.device_sprite_slice(world, d, rows, world_y, look, depth));
        };
        if let Some((top_end, bottom_start)) = art.tube_stretch
            && span > height
        {
            let tile_height = bottom_start - top_end;
            let middle_height = span - height + tile_height;
            if middle_height.div_ceil(tile_height) + 2 > available {
                return;
            }
            push(0..top_end, d.y, depth);
            let bottom_y = d.y + span - (height - bottom_start);
            for y in (d.y + top_end..bottom_y).step_by(tile_height) {
                let rows = tile_height.min(bottom_y - y);
                push(top_end..top_end + rows, y, depth);
            }
            push(bottom_start..height, bottom_y, depth);
        } else if let Some(split) = art.back_band_from
            && ghost.is_none()
            && split > 0
            && split < height
        {
            if available >= 2 {
                push(0..split, d.y, depth);
                push(
                    split..height,
                    d.y + split,
                    f32::from(desktank_sim::DEPTH_BANDS - 1),
                );
            }
        } else if available > 0 {
            push(0..height, d.y, depth);
        }
    }

    /// A vertical cell-aligned slice, with its top at an explicit world row.
    fn device_sprite_slice(
        &self,
        world: &World,
        d: &Device,
        rows: std::ops::Range<usize>,
        world_y: usize,
        look: [f32; 2],
        depth: f32,
    ) -> Sprite {
        let art = world.device_kinds().get(d.kind);
        let k = DeviceKind::ALL
            .iter()
            .position(|&k| k == d.kind)
            .unwrap_or(0);
        let entry = self.devices[k];
        let (base_origin, base_fine) = Self::origins(&entry, 0);
        let w = art.size.0;
        let h = rows.len();
        let dy_coarse = rows.start * TEXELS_PER_CELL;
        let dy_fine = dy_coarse * 2;
        // Device boxes are small: exact in f32.
        #[allow(clippy::cast_precision_loss)]
        let centre = ((d.x * 2 + w) as f32 / 2.0, (world_y * 2 + h) as f32 / 2.0);
        Sprite {
            centre,
            size: (w, h),
            origin: (base_origin.0, base_origin.1 + dy_coarse),
            fine: (base_fine.0, base_fine.1 + dy_fine),
            frame: (entry.size.0, h * TEXELS_PER_CELL),
            flip: d.facing_left,
            turn: 0,
            pitch: 0,
            look,
            depth,
            glow: [0.0; 4],
            burrower: false,
        }
    }
}

/// Packs narrow creature and device strips side-by-side onto shared rows
/// within [`ATLAS_WIDTH`] so the atlas stays inside WebGL2's 2048-texel limit.
struct Shelf {
    x: usize,
    y: usize,
    height: usize,
}

impl Shelf {
    fn new() -> Self {
        Self {
            x: 0,
            y: 0,
            height: 0,
        }
    }

    /// Allocates a `frames`-frame strip of `w × h` texels. Single-row strips
    /// (`frames * w <= ATLAS_WIDTH`) pack side-by-side so `frame_origin` never
    /// wraps (`frame < frames <= ATLAS_WIDTH / w`); multi-row strips always
    /// start at `x = 0` on a fresh row.
    fn alloc(&mut self, frames: usize, (w, h): (usize, usize)) -> (usize, usize) {
        let strip_w = w.saturating_mul(frames);
        let strip_h = h * frames.div_ceil((ATLAS_WIDTH / w).max(1));
        if strip_w <= ATLAS_WIDTH {
            if self.x + strip_w > ATLAS_WIDTH {
                self.y += self.height;
                self.x = 0;
                self.height = 0;
            }
            let origin = (self.x, self.y);
            self.x += strip_w;
            debug_assert!(self.x <= ATLAS_WIDTH);
            self.height = self.height.max(strip_h);
            origin
        } else {
            if self.height > 0 {
                self.y += self.height;
                self.x = 0;
                self.height = 0;
            }
            let origin = (0, self.y);
            self.y += strip_h;
            origin
        }
    }

    fn finish(self) -> usize {
        self.y + self.height
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
    pitch: i8,
    look: [f32; 2],
    /// Depth in bands from the front glass (D21).
    depth: f32,
    /// Night glow colour and strength (0 for none).
    glow: [f32; 4],
    /// A burrower: the shader hides it wherever loose substrate covers it
    /// (its look's second float is 1).
    burrower: bool,
}

impl Sprite {
    fn push(&self, out: &mut Vec<f32>) {
        // Sprite sides and atlas texels are small: exact in f32.
        #[allow(clippy::cast_precision_loss)]
        let f = |v: usize| v as f32;
        let (body_w, body_h) = (f(self.size.0), f(self.size.1));
        let diagonal = std::f32::consts::FRAC_1_SQRT_2;
        let (cos, sin) = match self.pitch.abs() {
            1 => (diagonal, diagonal * f32::from(self.pitch.signum())),
            2 => (0.0, f32::from(self.pitch.signum())),
            _ => (1.0, 0.0),
        };
        // The rotation follows the mirror, so its direction reverses on
        // a left-facing sprite. The bounding box is the same either way.
        // (Not a negative zero when level: some GPUs' compares tell it apart.)
        let sin = if self.flip && sin != 0.0 { -sin } else { sin };
        let (w, h) = if sin != 0.0 {
            (
                cos * body_w + sin.abs() * body_h,
                sin.abs() * body_w + cos * body_h,
            )
        } else if self.turn % 2 == 1 {
            (body_h, body_w)
        } else {
            (body_w, body_h)
        };
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
            if self.burrower {
                LOOK_BURROWER
            } else {
                self.look[1]
            },
            fine[0],
            fine[1],
            self.depth,
        ]);
        out.extend_from_slice(&self.glow);
        out.extend_from_slice(&[cos, sin, body_w, body_h]);
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
#[must_use]
pub fn icon(
    species: &SpeciesTable,
    id: desktank_sim::SpeciesId,
    x: u32,
    y: u32,
) -> Option<[u8; 3]> {
    centred_icon(&species.get(id)?.sprite.icon, x, y)
}

/// Pixel `(x, y)` of an 8×8 palette icon for species `id` in colour morph
/// `morph` (the base icon for an unknown morph), centred, `None` where
/// transparent.
#[must_use]
pub fn morph_icon(
    species: &SpeciesTable,
    id: desktank_sim::SpeciesId,
    morph: u8,
    x: u32,
    y: u32,
) -> Option<[u8; 3]> {
    centred_icon(species.get(id)?.morph_icon(morph), x, y)
}

/// Pixel `(x, y)` of `icon` centred in an 8×8 slot.
#[allow(clippy::many_single_char_names)] // pixel maths
fn centred_icon(icon: &desktank_sim::species::Icon, x: u32, y: u32) -> Option<[u8; 3]> {
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
        let atlas = Atlas::build(w.species(), w.device_kinds());
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
        let c = &w.creatures()[0];
        let art = &s.looks[s.look(w.creature_morph(c), c.female(), c.carrying() > 0)];
        let frame = usize::try_from(c.id().0).unwrap() % art.frames.len();
        let width = art.size.0;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (ox, oy) = (out[4] as usize, out[5] as usize);
        for (i, t) in art.frames[frame].iter().enumerate() {
            let (x, y) = (ox + i % width, oy + i / width);
            let o = (y * ATLAS_WIDTH + x) * 4;
            assert_eq!(&atlas.image[o..o + 4], t);
        }
        // Centred on the body.
        let (cx, cy) = w.creatures()[0].centre();
        assert!((out[0] + out[2] / 2.0 - cx).abs() < 1e-6);
        assert!((out[1] + out[3] / 2.0 - cy).abs() < 1e-6);
    }

    #[test]
    fn pitched_instance_bounds_keep_the_body_centre_and_follow_the_mirror() {
        let mut w = world();
        let tetra = w.species().id("neon_tetra").unwrap();
        w.add_creature(tetra, 30, 30).unwrap();
        let atlas = Atlas::build(w.species(), w.device_kinds());
        let (mut sprites, mut out) = (Vec::new(), Vec::new());
        atlas.instances(&w, &mut sprites, &mut out);
        let mut sprite = sprites[0];
        let (width, height) = (out[2], out[3]);
        let diagonal = std::f32::consts::FRAC_1_SQRT_2;

        for flip in [false, true] {
            sprite.flip = flip;
            for pitch in [-1, 2] {
                sprite.pitch = pitch;
                out.clear();
                sprite.push(&mut out);
                assert_eq!(out.len(), 23);
                assert_eq!(out.len(), INSTANCE_FLOATS);
                let instance = instance_at(&out, sprite.centre);
                let side = (width + height) * diagonal;
                let (box_w, box_h, cos, sin) = if pitch == -1 {
                    (side, side, diagonal, -diagonal)
                } else {
                    (height, width, 0.0, 1.0)
                };
                let sin = if flip { -sin } else { sin };
                for (actual, expected) in instance[2..4]
                    .iter()
                    .chain(&instance[19..23])
                    .zip([box_w, box_h, cos, sin, width, height])
                {
                    assert!((actual - expected).abs() < 1e-5);
                }
            }
        }
    }

    #[test]
    fn a_level_sprite_has_a_positive_zero_pitch_sine_mirrored_or_not() {
        let mut w = world();
        let tetra = w.species().id("neon_tetra").unwrap();
        w.add_creature(tetra, 30, 30).unwrap();
        let atlas = Atlas::build(w.species(), w.device_kinds());
        let (mut sprites, mut out) = (Vec::new(), Vec::new());
        atlas.instances(&w, &mut sprites, &mut out);
        let mut sprite = sprites[0];
        sprite.pitch = 0;
        for flip in [false, true] {
            sprite.flip = flip;
            out.clear();
            sprite.push(&mut out);
            // Chromium 68's SwiftShader reads -0.0 as non-zero, which sent
            // mirrored level fish down the pitched path in the shader.
            assert_eq!(out[20].to_bits(), 0.0f32.to_bits(), "flip {flip}");
        }
    }

    /// T9 part 2: each creature and corpse samples its own look (morph and
    /// sex), and the atlas stays within WebGL2's smallest texture limit.
    #[test]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn creatures_are_drawn_in_their_morph_and_sex() {
        let mut w = world();
        let atlas = Atlas::build(w.species(), w.device_kinds());
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
            (
                shrimp,
                w.species().get(shrimp).unwrap().look(blue, false, false),
            ),
            (
                guppy,
                w.species().get(guppy).unwrap().look(red, false, false),
            ),
            (
                guppy,
                w.species().get(guppy).unwrap().look(red, true, false),
            ),
            (
                shrimp,
                w.species().get(shrimp).unwrap().look(blue, true, false),
            ),
        ];
        for (i, (s, look)) in looks.into_iter().enumerate() {
            let sp = w.species().get(s).unwrap();
            let (ox, oy, _) = texel(i);
            let art = &sp.looks[look];
            let width = art.size.0;
            let matches = art.frames.iter().any(|f| {
                f.iter().enumerate().all(|(j, t)| {
                    let (x, y) = (ox + j % width, oy + j / width);
                    let o = (y * ATLAS_WIDTH + x) * 4;
                    &atlas.image[o..o + 4] == t
                })
            });
            assert!(matches, "instance {i}");
        }
        // The male and female guppies sample different art.
        let origin = |i| {
            let (x, y, _) = texel(i);
            (x, y)
        };
        assert_ne!(origin(1), origin(2));
    }

    #[test]
    fn carrying_art_has_a_separate_shelf_at_the_females_size() {
        let world = world();
        let frames = |colour: char, w, h| {
            let row = format!("\"{}\"", colour.to_string().repeat(w));
            format!("[[{}]]", vec![row; h].join(", "))
        };
        let src = format!(
            r##"
            id = 0
            name = "test_shrimp"
            scientific_name = "Caridina ficta"
            description = "A small shrimp for sprite tests."
            zone = "bottom"
            depth = [0.5, 1.0]
            min_depth = 2
            temperament = "peaceful"
            length_cm = 0.5
            lifespan_days = 100
            maturity_days = 10
            speed = 2.0
            max_speed = 4.0
            diet = []
            [breeding]
            mode = "livebearer"
            days = 30
            brood = [3, 8]
            rest_days = 7
            [sprite]
            colours = {{ a = "#102030", b = "#405060" }}
            icon = ["a"]
            frames = {}
            fine_frames = {}
            female_frames = {}
            female_fine_frames = {}
            carrying_frames = {}
            carrying_fine_frames = {}
            "##,
            frames('a', 4, 2),
            frames('a', 8, 4),
            frames('a', 6, 4),
            frames('a', 12, 8),
            frames('b', 6, 4),
            frames('b', 12, 8),
        );
        let table = SpeciesTable::from_toml_sources(&[&src], world.materials()).unwrap();
        let atlas = Atlas::build(&table, world.device_kinds());
        let id = table.id("test_shrimp").unwrap();
        let male = atlas.entry(&table, id, 0, false, false).unwrap();
        let female = atlas.entry(&table, id, 0, true, false).unwrap();
        let carrying = atlas.entry(&table, id, 0, true, true).unwrap();
        assert_eq!(male.size, (4, 2));
        assert_eq!(female.size, (6, 4));
        assert_eq!(carrying.size, female.size);
        assert_ne!(carrying.origin, female.origin);
        assert_ne!(carrying.fine, female.fine);
        let rgba = |(x, y)| {
            let i = (y * ATLAS_WIDTH + x) * 4;
            &atlas.image[i..i + 4]
        };
        for (entry, colour) in [(female, [0x10, 0x20, 0x30]), (carrying, [0x40, 0x50, 0x60])] {
            assert_eq!(
                rgba((entry.origin.0 + 5, entry.origin.1 + 3)),
                [colour[0], colour[1], colour[2], 255]
            );
            assert_eq!(
                rgba((entry.fine.0 + 11, entry.fine.1 + 7)),
                [colour[0], colour[1], colour[2], 255]
            );
        }
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
        let atlas = Atlas::build(world.species(), world.device_kinds());
        let mut out = Vec::new();
        assert_eq!(
            atlas.instances(&world, &mut Vec::new(), &mut out) as usize,
            ids.len()
        );
        assert!(include_str!("tank.wgsl").contains("@location(6) fine_origin: vec2<f32>"));
        for c in world.creatures() {
            let inst = instance_at(&out, c.centre());
            let s = world.species().get(c.species()).unwrap();
            let look = &s.looks[s.look(world.creature_morph(c), c.female(), c.carrying() > 0)];
            let frame = c.id().0 as usize % look.fine_frames.len();
            let (ox, oy) = (inst[12] as usize, inst[13] as usize);
            let width = look.size.0 * 2;
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
        let atlas = Atlas::build(w.species(), w.device_kinds());
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
    fn only_burrowers_are_hidden_by_the_substrate() {
        let mut w = world();
        let atlas = Atlas::build(w.species(), w.device_kinds());
        let loach = w.species().id("kuhli_loach").unwrap();
        let cory = w.species().id("corydoras").unwrap();
        w.add_creature(loach, 20, 30).unwrap();
        w.add_creature(cory, 45, 30).unwrap();
        let mut out = Vec::new();
        assert_eq!(atlas.instances(&w, &mut Vec::new(), &mut out), 2);
        let (l, c) = (
            instance_at(&out, w.creatures()[0].centre()),
            instance_at(&out, w.creatures()[1].centre()),
        );
        assert_eq!(l[10..12], [LOOK_ALIVE, LOOK_BURROWER]);
        assert_eq!(c[10..12], [LOOK_ALIVE, 0.0]);
    }

    #[test]
    fn a_crawler_on_the_glass_is_turned_with_its_quad_swapped() {
        let mut w = world();
        let atlas = Atlas::build(w.species(), w.device_kinds());
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
        let s = w.species().get(nerite).unwrap();
        let (fw, fh) = s.size_of(c.female());
        // Quads are in cells: the body, half the sprite's texels.
        let size = s.looks[s.look(w.creature_morph(c), c.female(), c.carrying() > 0)].size;
        assert_eq!((fw * 2, fh * 2), size);
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
        let atlas = Atlas::build(w.species(), w.device_kinds());
        let tetra = w.species().id("neon_tetra").unwrap();
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
        let fry = w.creature(fry).unwrap();
        let species = w.species().get(tetra).unwrap();
        let (full_w, full_h) = species.size_of(fry.female());
        let frame = [(full_w * 2) as f32, (full_h * 2) as f32];
        let (bw, bh) = w.creature_size(fry);
        assert!(bw < full_w && bh <= full_h);
        assert_eq!(fry_i[2..4], [bw as f32, bh as f32]);
        assert_eq!(fry_i[8..10], frame);
        assert_eq!(fry_i[12..14], [-1.0; 2], "a fry samples the coarse frame");
        assert!(old_i[12] >= 0.0);
        assert_eq!(fry_i[10], LOOK_ALIVE);
        assert_eq!(old_i[10], LOOK_ELDER);
        let (old_w, old_h) = species.size_of(w.creature(old).unwrap().female());
        assert_eq!(old_i[2..4], [old_w as f32, old_h as f32]);
        // The corpse: full size, upside down on frame 0, not yet rotting.
        assert_eq!(dead_i[7], 2.0);
        assert_eq!(dead_i[10], LOOK_DEAD);
        assert_eq!(dead_i[11], 0.0);
        let corpse = &w.corpses()[0];
        let e = atlas
            .entry(w.species(), tetra, corpse.morph(), corpse.female(), false)
            .unwrap();
        assert_eq!(dead_i[4..6], [e.origin.0 as f32, e.origin.1 as f32]);
        assert_eq!(dead_i[12..14], [e.fine.0 as f32, e.fine.1 as f32]);
    }

    /// D21: sprites are drawn back to front, so a nearer creature covers a
    /// further one, and each instance carries its depth for the shader.
    #[test]
    fn creatures_are_drawn_back_to_front() {
        let mut w = world();
        let atlas = Atlas::build(w.species(), w.device_kinds());
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

    #[test]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn devices_pack_within_webgl2_limits_at_their_instance_origins() {
        let mut w = World::new(
            WorldConfig {
                width: 160,
                height: 80,
                seed: 1,
                water_level: 1.0,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let atlas = Atlas::build(w.species(), w.device_kinds());
        assert!(
            atlas.size.0 <= 2048 && atlas.size.1 <= 2048,
            "atlas exceeds WebGL2 limit: {:?}",
            atlas.size
        );
        for (kind, x, y) in [
            (DeviceKind::Bubbler, 30, 40),
            (DeviceKind::InternalFilter, 4, 40),
            (DeviceKind::SpongeFilter, 60, 40),
            (DeviceKind::HobFilter, 74, 0),
            (DeviceKind::UnderGravelFilter, 135, 40),
            (DeviceKind::UnderGravelPanel, 108, 76),
        ] {
            assert!(
                w.add_device(kind, x, y, false).unwrap().is_some(),
                "{kind:?}"
            );
        }
        assert_eq!(w.devices().count(), DeviceKind::ALL.len());
        let mut out = Vec::new();
        let mut sprites = Vec::new();
        // Six devices, one extra HOB slice and seven extra telescoping slices.
        assert_eq!(atlas.instances(&w, &mut sprites, &mut out), 14);
        for d in w.devices() {
            let art = w.device_kinds().get(d.kind);
            let (cw, ch) = art.size;
            let slices = if d.kind == DeviceKind::UnderGravelFilter {
                assert_eq!(d.y, w.surface_row() + 1);
                assert_eq!(d.y + d.span(art), w.height());
                vec![
                    (0, 5, d.y, DEVICE_DEPTH),
                    (5, 17, 6, DEVICE_DEPTH),
                    (5, 17, 18, DEVICE_DEPTH),
                    (5, 17, 30, DEVICE_DEPTH),
                    (5, 17, 42, DEVICE_DEPTH),
                    (5, 17, 54, DEVICE_DEPTH),
                    (5, 8, 66, DEVICE_DEPTH),
                    (17, 28, 69, DEVICE_DEPTH),
                ]
            } else if let Some(split) = art.back_band_from {
                vec![
                    (0, split, d.y, DEVICE_DEPTH),
                    (
                        split,
                        ch,
                        d.y + split,
                        f32::from(desktank_sim::DEPTH_BANDS - 1),
                    ),
                ]
            } else {
                vec![(0, ch, d.y, DEVICE_DEPTH)]
            };
            for (row0, row1, world_y, expected_depth) in slices {
                let sh = row1 - row0;
                #[allow(clippy::cast_precision_loss)]
                let centre = ((d.x * 2 + cw) as f32 / 2.0, (world_y * 2 + sh) as f32 / 2.0);
                let inst = instance_at(&out, centre);
                assert_eq!((inst[2] as usize, inst[3] as usize), (cw, sh));
                assert_eq!(inst[9] as usize, sh * TEXELS_PER_CELL);
                assert_eq!(inst[14], expected_depth);
                let (ox, oy) = (inst[4] as usize, inst[5] as usize);
                let (fx, fy) = (inst[12] as usize, inst[13] as usize);
                assert!(ox + art.sprite.width <= ATLAS_WIDTH);
                assert!(fx + art.sprite.width * 2 <= ATLAS_WIDTH);
                let y0_c = row0 * TEXELS_PER_CELL;
                let y1_c = row1 * TEXELS_PER_CELL;
                for y in y0_c..y1_c {
                    for x in 0..art.sprite.width {
                        let t = art.sprite.frames[0][y * art.sprite.width + x];
                        let o = ((oy + y - y0_c) * ATLAS_WIDTH + ox + x) * 4;
                        assert_eq!(&atlas.image[o..o + 4], &t);
                    }
                }
                let fw = art.sprite.width * 2;
                let y0_f = y0_c * 2;
                let y1_f = y1_c * 2;
                for y in y0_f..y1_f {
                    for x in 0..fw {
                        let t = art.sprite.fine_frames[0][y * fw + x];
                        let o = ((fy + y - y0_f) * ATLAS_WIDTH + fx + x) * 4;
                        assert_eq!(&atlas.image[o..o + 4], &t);
                    }
                }
            }
        }
    }

    #[test]
    fn telescoping_placement_ghost_matches_the_placed_filter() {
        let mut w = world();
        let atlas = Atlas::build(w.species(), w.device_kinds());
        let kind = DeviceKind::UnderGravelFilter;
        let (ghost, fits) = w.device_placement(kind, 32, 40, true);
        assert!(fits);
        assert!(ghost.span(w.device_kinds().get(kind)) > 28);
        w.add_device(kind, 32, 40, true).unwrap().unwrap();

        let mut sprites = Vec::new();
        let mut out = Vec::new();
        let count = atlas.instances(&w, &mut sprites, &mut out);
        assert!(count > 2);
        assert_eq!(
            atlas.instances_with(&w, Some((ghost, false)), &mut sprites, &mut out),
            count * 2
        );
        let (instances, remainder) = out.as_chunks::<INSTANCE_FLOATS>();
        assert_eq!(remainder, []);
        let (placed, ghosts) = instances.split_at(count as usize);
        for (real, ghost) in placed.iter().zip(ghosts) {
            assert_eq!(&ghost[..10], &real[..10]);
            assert_eq!(&ghost[10..12], &[LOOK_GHOST, 1.0]);
            assert_eq!(&ghost[12..14], &real[12..14]);
            assert_eq!(ghost[14], 0.0);
        }
    }
}
