// The cell grid drawn as pixel art: each fragment finds its cell (integer
// scale, nearest by construction), looks up (material, variant) in the
// palette, darkens water with depth and tints by the time of day. Plant
// cells (T6b, T6d) are drawn from their species instead, at two texels per
// cell side like the sprites: the cell's state bytes are its plant id, whose
// texel in the plant texture holds the health tint and the species id; the
// species texture holds the colour ramps and part shapes. The interact overlay (palette
// and brush outline) is drawn on top. Creature sprites are instanced quads
// drawn after the grid (vs_sprite / fs_sprite), two atlas texels per cell side.
// WebGL2-safe: textureLoad on 8-bit unorm textures, one uniform buffer, a
// per-instance vertex buffer, no storage buffers.

struct View {
    // Grid size in cells.
    grid: vec2<f32>,
    // Top-left of the grid in target pixels.
    offset: vec2<f32>,
    // Target pixels per cell.
    scale: f32,
    // Top water row; depth is measured from here to the floor.
    surface_row: f32,
    // Darkening of tinted materials at the floor (0 = none).
    depth_dim: f32,
    // 1 if the target is sRGB (it encodes on write, so the shader, which
    // shades sRGB-encoded palette values, outputs linear); 0 if not.
    srgb_target: f32,
    // Letterbox colour, sRGB-encoded.
    backdrop: vec4<f32>,
    // Palette: top-left in target pixels, target pixels per palette pixel,
    // and 1 if shown.
    hud: vec4<f32>,
    // Palette image size in palette pixels (xy), and the field overlay (z):
    // 0 none, 1 nutrient, 2 CO2, 3 light.
    hud_size: vec4<f32>,
    // Brush outline: centre cell, radius in cells, and 1 if shown.
    cursor: vec4<f32>,
    // Day and night: a colour multiplier for the tank (rgb; 1 by day).
    daylight: vec4<f32>,
    // Target size in pixels (xy).
    target_size: vec4<f32>,
    // Material ids of the plant parts root (x), stem (y) and leaf (z), and
    // of water (w); -1 if missing.
    plant_parts: vec4<f32>,
};

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var cells: texture_2d<f32>;
@group(0) @binding(2) var palette: texture_2d<f32>;
@group(0) @binding(3) var hud: texture_2d<f32>;
// Nutrient, CO2 and light per 8x8-cell field cell, for the overlay.
@group(0) @binding(4) var fields: texture_2d<f32>;
// Creature sprite frames (sRGB-encoded RGBA; alpha 0 is transparent).
@group(0) @binding(5) var atlas: texture_2d<f32>;
// Per plant id (id & 255 across, id >> 8 down): health tint (rgb) and
// species id (a).
@group(0) @binding(6) var plant_tints: texture_2d<f32>;
// Per species id (down): RAMP shades each for root, stem and leaf, then the
// shapes texel (leaf, stem, root) and the sizes texel (stem height, root
// depth, leaf length), as bytes (plants.rs).
@group(0) @binding(7) var plant_species: texture_2d<f32>;

// Plant parts (desktank_sim::Part).
const PART_ROOT: i32 = 0;
const PART_STEM: i32 = 1;
const PART_LEAF: i32 = 2;
// Species texture layout (plants.rs).
const RAMP: i32 = 8;
const SHAPES_COLUMN: i32 = 24;
const SIZES_COLUMN: i32 = 25;
// Shapes (desktank_sim::plants::{LeafShape, StemShape, RootShape}).
const LEAF_LANCEOLATE: i32 = 0;
const LEAF_NEEDLE: i32 = 1;
const LEAF_ROUND: i32 = 2;
const LEAF_FEATHER: i32 = 3;
const STEM_STALK: i32 = 0;
const STEM_THICK: i32 = 1;
const STEM_FUZZ: i32 = 2;
const STEM_FROND: i32 = 3;
const ROOT_STRAND: i32 = 0;
const ROOT_THICK: i32 = 1;

// The fields at `cell`, interpolated between field-cell centres.
fn field_at(cell: vec2<f32>) -> vec3<f32> {
    let size = vec2<i32>(textureDimensions(fields));
    let f = (cell + 0.5) / 8.0 - 0.5;
    let base = floor(f);
    let t = f - base;
    let lo = clamp(vec2<i32>(base), vec2<i32>(0), size - 1);
    let hi = clamp(vec2<i32>(base) + 1, vec2<i32>(0), size - 1);
    let a = textureLoad(fields, vec2<i32>(lo.x, lo.y), 0).rgb;
    let b = textureLoad(fields, vec2<i32>(hi.x, lo.y), 0).rgb;
    let c = textureLoad(fields, vec2<i32>(lo.x, hi.y), 0).rgb;
    let d = textureLoad(fields, vec2<i32>(hi.x, hi.y), 0).rgb;
    return mix(mix(a, b, t.x), mix(c, d, t.x), t.y);
}

// Whether `cell` is inside the brush disc (the same test as World::paint).
fn in_brush(cell: vec2<f32>) -> bool {
    let d = cell - view.cursor.xy;
    let r = view.cursor.z;
    return dot(d, d) <= r * r + r;
}

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    // One triangle covering the target.
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    if view.hud.w > 0.5 {
        let p = floor((pos.xy - view.hud.xy) / view.hud.z);
        if all(p >= vec2<f32>(0.0)) && all(p < view.hud_size.xy) {
            let h = textureLoad(hud, vec2<i32>(p), 0);
            if h.a > 0.5 {
                return out(h.rgb);
            }
        }
    }
    let cell = floor((pos.xy - view.offset) / view.scale);
    if any(cell < vec2<f32>(0.0)) || any(cell >= view.grid) {
        return out(view.backdrop.rgb);
    }
    let at = vec2<i32>(cell);
    let texel = textureLoad(cells, at, 0);
    var rgb: vec3<f32>;
    let part = part_of(texel);
    if part >= 0 {
        // Which of the cell's 2x2 texels this pixel shows. Below scale 2 a
        // pixel is a whole cell, so the shape is dropped (filled).
        let sub = vec2<i32>(floor(fract((pos.xy - view.offset) / view.scale) * 2.0));
        rgb = plant_colour(at, texel, part, sub, view.scale < 2.0);
    } else {
        rgb = ground(at, texel);
    }
    rgb = rgb * view.daylight.rgb;
    // Debug overlay: tint by a field's value.
    let mode = view.hud_size.z;
    if mode > 0.5 {
        let f = field_at(cell);
        if mode < 1.5 {
            rgb = mix(rgb, vec3<f32>(0.35, 1.0, 0.3), 0.75 * f.r);
        } else if mode < 2.5 {
            rgb = mix(rgb, vec3<f32>(0.85, 0.5, 1.0), 0.75 * f.g);
        } else {
            rgb = rgb * mix(0.15, 1.0, f.b);
        }
    }
    // Brush outline: disc cells with a 4-neighbour outside the disc.
    if view.cursor.w > 0.5 && in_brush(cell) {
        let edge = !(in_brush(cell + vec2<f32>(1.0, 0.0)) && in_brush(cell - vec2<f32>(1.0, 0.0))
            && in_brush(cell + vec2<f32>(0.0, 1.0)) && in_brush(cell - vec2<f32>(0.0, 1.0)));
        if edge {
            rgb = mix(rgb, vec3<f32>(1.0), 0.6);
        }
    }
    return out(rgb);
}

// The colour of a non-plant cell: its palette entry, darkened with depth if
// it is water (before the daylight tint).
fn ground(at: vec2<i32>, t: vec4<f32>) -> vec3<f32> {
    let c = textureLoad(palette, vec2<i32>(round(t.rg * 255.0)), 0);
    // Palette alpha flags: 1 darkens with depth, 0 not.
    let tinted = select(0.0, 1.0, c.a > 0.75);
    let depth = clamp((f32(at.y) - view.surface_row) / max(view.grid.y - view.surface_row, 1.0), 0.0, 1.0);
    return c.rgb * (1.0 - view.depth_dim * depth * tinted);
}

fn inside(at: vec2<i32>) -> bool {
    return all(at >= vec2<i32>(0)) && all(vec2<f32>(at) < view.grid);
}

// The plant part of a cell texel, or -1 if it isn't one.
fn part_of(t: vec4<f32>) -> i32 {
    let m = round(t.r * 255.0);
    if m == view.plant_parts.x {
        return PART_ROOT;
    }
    if m == view.plant_parts.y {
        return PART_STEM;
    }
    if m == view.plant_parts.z {
        return PART_LEAF;
    }
    return -1;
}

// A plant cell's part-local position (the variant's low 6 bits).
fn position_of(t: vec4<f32>) -> i32 {
    return i32(round(t.g * 255.0)) & 63;
}

// A plant cell's colour noise (the variant's top 2 bits) as a ramp step:
// mostly 0, sometimes one darker or lighter.
fn noise_step(t: vec4<f32>) -> i32 {
    let n = i32(round(t.g * 255.0)) >> 6u;
    return select(select(1, -1, n == 2), 0, n < 2);
}

// The neighbour at `at + d` if it is a part of the same plant as `me`:
// (part, position), or (-1, 0).
fn kin(at: vec2<i32>, d: vec2<i32>, me: vec4<f32>) -> vec2<i32> {
    let c = at + d;
    if !inside(c) {
        return vec2<i32>(-1, 0);
    }
    let t = textureLoad(cells, c, 0);
    let part = part_of(t);
    if part < 0 || any(abs(t.ba - me.ba) > vec2<f32>(0.5 / 255.0)) {
        return vec2<i32>(-1, 0);
    }
    return vec2<i32>(part, position_of(t));
}

// Whether the cell at `at` is water (what a mat's fringe frays into).
fn is_water(at: vec2<i32>) -> bool {
    return inside(at) && round(textureLoad(cells, at, 0).r * 255.0) == view.plant_parts.w;
}

// A static per-texel hash (texel coordinates in the grid's 2x texel space).
fn hash(p: vec2<i32>) -> u32 {
    var h = u32(p.x) * 0x8da6b343u ^ u32(p.y) * 0xd8163841u;
    h = (h ^ (h >> 13u)) * 0x5bd1e995u;
    return h ^ (h >> 15u);
}

// What shows through an empty texel of a plant cell: the first non-plant
// neighbour on that texel's side (across, then above or below, then
// diagonally), or `shadow` when the plant fills all three.
fn behind(at: vec2<i32>, sub: vec2<i32>, shadow: vec3<f32>) -> vec3<f32> {
    let d = sub * 2 - 1;
    let a = at + vec2<i32>(d.x, 0);
    if inside(a) {
        let t = textureLoad(cells, a, 0);
        if part_of(t) < 0 {
            return ground(a, t);
        }
    }
    let b = at + vec2<i32>(0, d.y);
    if inside(b) {
        let t = textureLoad(cells, b, 0);
        if part_of(t) < 0 {
            return ground(b, t);
        }
    }
    let c = at + d;
    if inside(c) {
        let t = textureLoad(cells, c, 0);
        if part_of(t) < 0 {
            return ground(c, t);
        }
    }
    return shadow;
}

// A plant cell's colour at texel `sub` (0 or 1 across and down) of its 2x2
// block (T6d): its species' shape for the part decides whether the texel is
// covered, and a shade from the part's ramp by position along the plant and
// within the shape; the plant's health tint multiplies it. Uncovered texels
// show what is behind. `full` fills every texel (scale 1).
fn plant_colour(at: vec2<i32>, me: vec4<f32>, part: i32, sub: vec2<i32>, full: bool) -> vec3<f32> {
    let plant = textureLoad(plant_tints, vec2<i32>(round(me.ba * 255.0)), 0);
    let species = i32(round(plant.a * 255.0));
    let shapes = vec4<i32>(round(textureLoad(plant_species, vec2<i32>(SHAPES_COLUMN, species), 0) * 255.0));
    let sizes = round(textureLoad(plant_species, vec2<i32>(SIZES_COLUMN, species), 0) * 255.0);
    let pos = position_of(me);
    let top = sub.y == 0;
    var covered = true;
    var step = noise_step(me);
    if part == PART_LEAF {
        let left = kin(at, vec2<i32>(-1, 0), me);
        let right = kin(at, vec2<i32>(1, 0), me);
        // Leaves grow sideways from a stem node; the side it grows from
        // holds the stem or this leaf's previous cell.
        let from_left = left.x == PART_STEM || (left.x == PART_LEAF && left.y == pos - 1);
        let dir = select(-1, 1, from_left);
        let outer = select(left, right, from_left);
        let tip = !(outer.x == PART_LEAF && outer.y == pos + 1);
        let base = pos <= 1;
        // 0: the texel nearer the stem, 1: the outer one.
        let u = select(1 - sub.x, sub.x, from_left);
        // Higher leaves are newer: lighter (or redder, by the ramp).
        let node = kin(at, vec2<i32>(-dir * pos, 0), me);
        let h = select(0.5, f32(node.y) / max(sizes.x, 1.0), node.x == PART_STEM);
        step += 1 + i32(round(h * 5.0)) + select(0, 1, top) + select(0, 1, tip && u == 1);
        let shape = shapes.x;
        if shape == LEAF_NEEDLE {
            // One texel thick, rising from the stem to the tip.
            let rising = (u == 0 && !top) || (u == 1 && top);
            if base && !tip {
                covered = !top;
            } else if tip && !base {
                covered = top;
            } else {
                covered = rising;
            }
        } else if shape == LEAF_ROUND {
            // Broad to a blunt tip, with a darker rim at the tip's foot.
            covered = !(base && u == 0 && top);
            step -= select(0, 2, tip && u == 1 && !top);
        } else if shape == LEAF_FEATHER {
            // A midrib along the bottom, fine teeth above every other texel.
            covered = !top || u == 1;
            step += select(0, 1, top);
        } else {
            // Lanceolate: a petiole, a broad blade and a tip that rises.
            covered = !(base && u == 0 && top) && !(tip && u == 1 && !top);
        }
    } else if part == PART_STEM {
        let shape = shapes.y;
        if shape == STEM_FUZZ || shape == STEM_FROND {
            // A mat: texels facing water fray, and growing tips are lighter.
            let d = sub * 2 - 1;
            let exposed = is_water(at + vec2<i32>(d.x, 0)) || is_water(at + vec2<i32>(0, d.y));
            let h = hash(at * 2 + sub);
            if shape == STEM_FUZZ {
                covered = !exposed || (h & 7u) < 5u;
                step = 1 + i32((h >> 3u) % 4u) + select(0, 3, exposed);
            } else {
                // Layered fronds: a dark lower row under a lighter, ragged upper one.
                covered = !top || !exposed || (h & 3u) != 0u;
                step = select(1, 4, top) + i32((h >> 3u) % 3u) + select(0, 1, exposed);
            }
        } else {
            let left = kin(at, vec2<i32>(-1, 0), me);
            let right = kin(at, vec2<i32>(1, 0), me);
            let node = left.x == PART_LEAF || right.x == PART_LEAF;
            let tip = kin(at, vec2<i32>(0, -1), me).x != PART_STEM;
            step += 1 + i32(round(f32(pos) / max(sizes.x, 1.0) * 5.0)) - select(0, 1, node) + select(0, 2, tip);
            if shape == STEM_THICK {
                step -= select(0, 1, sub.x == 1);
            } else {
                // Stalk: one texel wide, full at nodes and the growing tip.
                covered = node || tip || sub.x == 0;
            }
        }
    } else {
        step += 1 + i32(round(f32(pos) / max(sizes.y, 1.0) * 4.0));
        if shapes.z == ROOT_STRAND {
            // A strand one texel wide through a texel picked per cell,
            // reaching towards each neighbouring root or stem.
            let k = hash(at);
            let col = i32(k & 1u);
            let row = i32((k >> 1u) & 1u);
            let up = kin(at, vec2<i32>(0, -1), me).x >= 0;
            let down = kin(at, vec2<i32>(0, 1), me).x >= 0;
            let left = kin(at, vec2<i32>(-1, 0), me).x >= 0;
            let right = kin(at, vec2<i32>(1, 0), me).x >= 0;
            let vertical = sub.x == col && select(down, up, top);
            let across = sub.y == row && select(right, left, sub.x == 0);
            covered = vertical || across || (sub.x == col && sub.y == row);
        }
    }
    let tint = plant.rgb;
    if covered || full {
        let shade = clamp(step, 0, RAMP - 1);
        return textureLoad(plant_species, vec2<i32>(part * RAMP + shade, species), 0).rgb * tint;
    }
    let shadow = textureLoad(plant_species, vec2<i32>(part * RAMP, species), 0).rgb * tint * 0.6;
    return behind(at, sub, shadow);
}

// An sRGB-encoded colour as the target expects it. Shading happens in
// encoded space on every target, so captures, windows and the web agree.
fn out(c: vec3<f32>) -> vec4<f32> {
    let linear = select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
    return vec4<f32>(select(c, linear, view.srgb_target > 0.5), 1.0);
}

// Sprite texels per cell side (desktank_sim::TEXELS_PER_CELL).
const TEXELS_PER_CELL: f32 = 2.0;

struct SpriteOut {
    @builtin(position) pos: vec4<f32>,
    // Atlas texel coordinate, interpolated across the quad.
    @location(0) texel: vec2<f32>,
};

// One creature's quad: two triangles over its sprite, the top-left snapped
// to a target pixel and each texel half a cell wide.
@vertex
fn vs_sprite(
    @builtin(vertex_index) i: u32,
    // Top-left in cells (fractional), on-screen size in cells (the frame's
    // size, swapped when turned a quarter), the frame's top-left texel in
    // the atlas, and orient: x > 0.5 to mirror (facing left in its own
    // frame), y the quarter turns clockwise applied after the mirror.
    @location(0) cell: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) origin: vec2<f32>,
    @location(3) orient: vec2<f32>,
) -> SpriteOut {
    // Corners (0,0) (1,0) (0,1) | (0,1) (1,0) (1,1).
    let corner = vec2<f32>(
        select(0.0, 1.0, i == 1u || i == 4u || i == 5u),
        select(0.0, 1.0, i == 2u || i == 3u || i == 5u),
    );
    let top_left = round(view.offset + cell * view.scale);
    let px = top_left + corner * size * view.scale;
    let ndc = vec2<f32>(px.x / view.target_size.x * 2.0 - 1.0, 1.0 - px.y / view.target_size.y * 2.0);
    // Undo the turn: the frame point (0..1) this screen corner shows. One
    // clockwise turn takes frame (a, b) to screen (1 - b, a).
    let turn = u32(round(orient.y)) % 4u;
    var q = corner;
    if turn == 1u {
        q = vec2<f32>(corner.y, 1.0 - corner.x);
    } else if turn == 2u {
        q = vec2<f32>(1.0 - corner.x, 1.0 - corner.y);
    } else if turn == 3u {
        q = vec2<f32>(1.0 - corner.y, corner.x);
    }
    if orient.x > 0.5 {
        q.x = 1.0 - q.x;
    }
    // The frame's size in texels.
    let frame = select(size, size.yx, turn % 2u == 1u) * TEXELS_PER_CELL;
    var o: SpriteOut;
    o.pos = vec4<f32>(ndc, 0.0, 1.0);
    o.texel = origin + q * frame;
    return o;
}

@fragment
fn fs_sprite(v: SpriteOut) -> @location(0) vec4<f32> {
    // The palette stays on top (where drawn: creatures show beside its
    // shorter row).
    if view.hud.w > 0.5 {
        let p = floor((v.pos.xy - view.hud.xy) / view.hud.z);
        if all(p >= vec2<f32>(0.0)) && all(p < view.hud_size.xy)
            && textureLoad(hud, vec2<i32>(p), 0).a > 0.5 {
            discard;
        }
    }
    let t = textureLoad(atlas, vec2<i32>(floor(v.texel)), 0);
    if t.a < 0.5 {
        discard;
    }
    return out(t.rgb * view.daylight.rgb);
}
