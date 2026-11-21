// The cell grid drawn as pixel art: each fragment finds its cell (integer
// scale, nearest by construction), looks up (material, variant) in the
// palette, darkens water with depth and tints by the time of day. Plant
// cells (T6b, T6d) are drawn from their species instead, at two texels per
// cell side like the sprites, or four at scales divisible by four: the
// cell's state bytes are its plant id (a rigid cell's are its sub-cell
// outline mask instead, see `outline_of`), whose texel in the plant texture
// holds its health, colouration and species id; the species texture holds
// the colour ramps, part shapes and colouration colour. A plant shifts
// towards its colouration colour (strong light, lean water), pales,
// yellows and then browns and turns translucent as its health falls. The interact overlay (palette and brush
// outline) is drawn on top. Creature sprites are instanced quads
// drawn after the grid (vs_sprite / fs_sprite), two atlas texels per cell
// side, or four from the fine frames at scales divisible by four.
// 2.5D depth (D21): sprites come back to front, and a sprite fragment over
// a covered texel of a plant cell in a nearer band is discarded, so the
// plant the grid drew there stays in front; plant cells behind it are
// simply drawn over. Creatures and plant cells fade a little towards the
// water colour with depth.
// Night glow (the blinker): a glowing creature's instance carries its glow
// colour and strength; fs_sprite adds it after the daylight tint, scaled
// by the darkness (view.daylight.w), weighted per texel (outline most, eye
// whites less, body faintly, pupils not at all), and at night its quad
// grows by HALO texels for a soft halo blended over the water.
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
    // 0 none, 1 nutrient, 2 CO2, 3 light, 4 flow.
    hud_size: vec4<f32>,
    // Brush outline: centre cell, radius in cells, and 1 if shown.
    cursor: vec4<f32>,
    // Day and night: a colour multiplier for the tank (rgb; 1 by day), and
    // how dark it is (w: 0 by day, 1 at night) for glowing creatures.
    daylight: vec4<f32>,
    // Target size in pixels (xy), sway phase in seconds (z), and sway
    // enabled (w: 1 when active, 0 when off).
    target_size: vec4<f32>,
    // Material ids of the plant parts root (x), stem (y) and leaf (z), and
    // of water (w); -1 if missing.
    plant_parts: vec4<f32>,
    // Material ids of the runner (x) and floating leaf (y) parts; -1 if
    // missing.
    plant_parts2: vec4<f32>,
    // Circular magnifier lens in target pixels: centre (xy), inner radius (z),
    // and bezel/handle pixel-art block size (w); z = 0 outside the lens pass.
    lens: vec4<f32>,
};

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var cells: texture_2d<f32>;
@group(0) @binding(2) var palette: texture_2d<f32>;
@group(0) @binding(3) var hud: texture_2d<f32>;
// Nutrient, CO2, light and current speed per 8x8-cell field cell, for the overlay.
@group(0) @binding(4) var fields: texture_2d<f32>;
// Creature sprite frames (sRGB-encoded RGBA; alpha 0 is transparent).
@group(0) @binding(5) var atlas: texture_2d<f32>;
// Per plant id (id & 255 across, id >> 8 down): health (r), colouration
// (g) and species id (a).
@group(0) @binding(6) var plant_tints: texture_2d<f32>;
// Per species id (down): RAMP shades each for root, stem and leaf, then the
// shapes texel (leaf, stem, root) and the sizes texel (stem height, root
// depth, leaf length; blade length, root depth, blade length for rosettes
// and carpets) and the colouration texel (colour, a = 1 if it colours), as
// bytes (plants.rs).
@group(0) @binding(7) var plant_species: texture_2d<f32>;
// Per cell: a plant cell's depth band (D21) in bands / 255 (0 elsewhere).
@group(0) @binding(8) var bands: texture_2d<f32>;

// The back band (DEPTH_BANDS - 1; renderer.rs checks it).
const BACK_BAND: f32 = 7.0;
// Depth cue at the back band: how far colours fade towards HAZE and how
// much darker they get (none at the front glass).
const HAZE_MIX: f32 = 0.22;
const HAZE_DIM: f32 = 0.1;
// Haze colour, sRGB-encoded: a deep water blue.
const HAZE: vec3<f32> = vec3<f32>(0.11, 0.27, 0.42);

// Ground styles stored in palette.a (palette.rs GroundStyle; renderer.rs checks them).
const STYLE_DEFAULT: i32 = 0;
const STYLE_WATER: i32 = 1;
const STYLE_AIR: i32 = 2;
const STYLE_FERTILISER: i32 = 3;
const STYLE_SAND: i32 = 4;
const STYLE_GRAVEL: i32 = 5;
const STYLE_AQUASOIL: i32 = 6;
const STYLE_ROCK: i32 = 7;
const STYLE_DRIFTWOOD: i32 = 8;
const STYLE_BUBBLE: i32 = 9;
const STYLE_ALGAE: i32 = 10;
const STYLE_DETRITUS: i32 = 11;

// Plant parts (desktank_sim::Part).
const PART_ROOT: i32 = 0;
const PART_STEM: i32 = 1;
const PART_LEAF: i32 = 2;
const PART_RUNNER: i32 = 3;
const PART_FLOAT: i32 = 4;
const PART_RHIZOME: i32 = 5;
// Species texture layout (plants.rs).
const RAMP: i32 = 8;
const SHAPES_COLUMN: i32 = 24;
const SIZES_COLUMN: i32 = 25;
const COLOUR_COLUMN: i32 = 26;
// Health (T6b, T6d): untinted at and above VIGOROUS, paling to YELLOW by
// YELLOW_AT (where leaves start to drop), then browning to DEAD_BROWN at 0,
// each shade keeping its lightness; below FADE_AT it turns translucent,
// down to MIN_OPACITY.
const VIGOROUS: f32 = 0.8;
const YELLOW_AT: f32 = 0.5;
const YELLOW: vec3<f32> = vec3<f32>(1.0, 0.92, 0.55);
const DEAD_BROWN: vec3<f32> = vec3<f32>(0.5, 0.36, 0.2);
const FADE_AT: f32 = 0.3;
const MIN_OPACITY: f32 = 0.35;
// Colouration (T6d): how much of a plant's colouration its darkest (vs
// lightest, newest) shades take, its stems' share of its leaves', and how
// far ahead of the colouration the shift runs.
const COLOUR_DARK: f32 = 0.5;
const COLOUR_STEM: f32 = 0.5;
const COLOUR_GAIN: f32 = 1.6;
// Shapes (desktank_sim::plants::{LeafShape, StemShape, RootShape}).
const LEAF_LANCEOLATE: i32 = 0;
const LEAF_NEEDLE: i32 = 1;
const LEAF_ROUND: i32 = 2;
const LEAF_FEATHER: i32 = 3;
const LEAF_RIBBON: i32 = 4;
const LEAF_SWORD: i32 = 5;
const LEAF_GRASS: i32 = 6;
const LEAF_PAD: i32 = 7;
const LEAF_LENS: i32 = 8;
const LEAF_DISC: i32 = 9;
const LEAF_LOTUS: i32 = 10;
const STEM_STALK: i32 = 0;
const STEM_THICK: i32 = 1;
const STEM_FUZZ: i32 = 2;
const STEM_FROND: i32 = 3;
const STEM_VELVET: i32 = 4;
const ROOT_STRAND: i32 = 0;
const ROOT_THICK: i32 = 1;

// The fields at `cell`, interpolated between field-cell centres.
fn field_at(cell: vec2<f32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(fields));
    let f = (cell + 0.5) / 8.0 - 0.5;
    let base = floor(f);
    let t = f - base;
    let lo = clamp(vec2<i32>(base), vec2<i32>(0), size - 1);
    let hi = clamp(vec2<i32>(base) + 1, vec2<i32>(0), size - 1);
    let a = textureLoad(fields, vec2<i32>(lo.x, lo.y), 0);
    let b = textureLoad(fields, vec2<i32>(hi.x, lo.y), 0);
    let c = textureLoad(fields, vec2<i32>(lo.x, hi.y), 0);
    let d = textureLoad(fields, vec2<i32>(hi.x, hi.y), 0);
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

// Where a grid fragment's plant art comes from: its own cell, or a swaying
// neighbour's leaf drawn over it (`from_sway`); `swayed_away` when its own
// swaying leaf moved off it, revealing what is behind. `part` is -1 when no
// plant is drawn here.
struct GridSource {
    at: vec2<i32>,
    texel: vec4<f32>,
    part: i32,
    quarter: vec2<i32>,
    from_sway: bool,
    swayed_away: bool,
};

// The palette pixel at `pos` if the palette is shown and covers it (alpha
// above 0.5), else alpha 0.
fn hud_at(pos: vec2<f32>) -> vec4<f32> {
    if view.hud.w > 0.5 {
        let p = floor((pos - view.hud.xy) / view.hud.z);
        if all(p >= vec2<f32>(0.0)) && all(p < view.hud_size.xy) {
            return textureLoad(hud, vec2<i32>(p), 0);
        }
    }
    return vec4<f32>(0.0);
}

// The quarter of its cell that grid position `g` falls in.
fn quarter_of(g: vec2<f32>, cell: vec2<f32>) -> vec2<i32> {
    return clamp(vec2<i32>(floor((g - cell) * 4.0)), vec2<i32>(0), vec2<i32>(3));
}

fn grid_source(grid_pos: vec2<f32>, at: vec2<i32>, texel: vec4<f32>, quarter: vec2<i32>) -> GridSource {
    let part = part_of(texel);
    var src = GridSource(at, texel, part, quarter, false, false);
    let sway_on = view.target_size.w > 0.5;
    let self_sways = sway_on && part >= 0 && is_swaying_part(part, texel);
    if sway_on && (part < 0 || self_sways) {
        let s_grid = vec2<f32>(grid_pos.x - sway_dx(grid_pos), grid_pos.y);
        let s_cell = floor(s_grid);
        let s_at = vec2<i32>(s_cell);
        var s_sways = false;
        var s_texel = vec4<f32>(0.0);
        var s_part = -1;
        if inside(s_at) {
            s_texel = textureLoad(cells, s_at, 0);
            s_part = part_of(s_texel);
            s_sways = s_part >= 0 && is_swaying_part(s_part, s_texel);
        }
        if s_sways && (self_sways || style_of(texel) == STYLE_WATER) {
            src = GridSource(s_at, s_texel, s_part, quarter_of(s_grid, s_cell), !self_sways, false);
        } else if self_sways {
            src.part = -1;
            src.swayed_away = true;
        }
    }
    return src;
}

// What every grid colour gets last: the time of day, the field overlay and
// the brush outline. Each is affine in `rgb` with weights that depend only
// on the cell, so it commutes with blending a plant over the ground (the
// split grid relies on this).
fn grid_post(rgb_in: vec3<f32>, cell: vec2<f32>) -> vec3<f32> {
    var rgb = rgb_in * view.daylight.rgb;
    // Debug overlay: tint by a field's value.
    let mode = view.hud_size.z;
    if mode > 0.5 {
        let f = field_at(cell);
        if mode < 1.5 {
            rgb = mix(rgb, vec3<f32>(0.35, 1.0, 0.3), 0.75 * f.r);
        } else if mode < 2.5 {
            rgb = mix(rgb, vec3<f32>(0.85, 0.5, 1.0), 0.75 * f.g);
        } else if mode < 3.5 {
            rgb = rgb * mix(0.15, 1.0, f.b);
        } else {
            let tint = mix(vec3<f32>(0.05, 0.8, 0.85), vec3<f32>(1.0, 0.94, 0.35), f.a);
            rgb = mix(rgb, tint, 0.75 * f.a);
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
    return rgb;
}

// True during the magnifier pass when `pos` lies outside the circular glass
// lens (snapped to the bezel's pixel-art grid so the rim meets the glass
// cleanly with no gaps). Always false in the main pass (`view.lens.z == 0`).
fn outside_lens(pos: vec2<f32>) -> bool {
    if view.lens.z <= 0.0 {
        return false;
    }
    let step = max(view.lens.w, 1.0);
    let p = floor((pos - view.lens.xy) / step) + 0.5;
    let r = view.lens.z / step;
    return dot(p, p) > r * r;
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    if outside_lens(pos.xy) {
        discard;
    }
    let h = hud_at(pos.xy);
    if h.a > 0.5 {
        return out(h.rgb);
    }
    let grid_pos = (pos.xy - view.offset) / view.scale;
    let cell = floor(grid_pos);
    if any(cell < vec2<f32>(0.0)) || any(cell >= view.grid) {
        return out(view.backdrop.rgb);
    }
    let at = vec2<i32>(cell);
    let texel = textureLoad(cells, at, 0);
    let quarter = quarter_of(grid_pos, cell);
    var rgb = vec3<f32>(0.0);
    var ground_texel = texel;
    var need_ground = true;
    var plant_rgb = vec3<f32>(0.0);
    var plant_alpha = 0.0;
    let src = grid_source(grid_pos, at, texel, quarter);
    if src.part >= 0 {
        let band = band_at(src.at);
        let pt = plant_texel(src.at, src.texel, src.part, src.quarter, band);
        if pt.a >= 0.999 {
            rgb = pt.rgb;
            need_ground = false;
        } else if src.from_sway {
            plant_rgb = pt.rgb;
            plant_alpha = pt.a;
        } else {
            let nbr = behind_cell(at, quarter);
            if nbr.x >= 0 {
                ground_texel = textureLoad(cells, nbr, 0);
                plant_rgb = pt.rgb;
                plant_alpha = pt.a;
            } else {
                let shadow = plant_shadow_of(src.texel, src.part, band);
                rgb = mix(shadow, pt.rgb, pt.a);
                need_ground = false;
            }
        }
    } else if src.swayed_away {
        // The swaying leaf moved away from `at`, revealing what is behind.
        let nbr = behind_cell(at, quarter);
        if nbr.x >= 0 {
            ground_texel = textureLoad(cells, nbr, 0);
        } else {
            rgb = water_bg(at, quarter, FINE_PLANTS && fine_scale());
            need_ground = false;
        }
    }
    if need_ground {
        let bg = ground(at, ground_texel, quarter);
        rgb = mix(bg, plant_rgb, plant_alpha);
    }
    return out(grid_post(rgb, cell));
}

// The split grid (`GpuOptions::split_grid`, for GPUs that can't compile
// fs_main in one piece, such as Chrome 68's SwiftShader with its 4096
// temporary registers): fs_ground draws everything but the plant art, then
// fs_leaves and fs_stems blend the plant cells over it. Together they draw what fs_main
// draws: blending in the target (linear on sRGB targets) instead of in
// encoded space differs only where a plant texel is translucent.

// Pass 1: the palette, the letterbox, and the ground or water each pixel
// shows behind any plant art (or anything, where a plant pass draws opaque).
@fragment
fn fs_ground(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    if outside_lens(pos.xy) {
        discard;
    }
    let h = hud_at(pos.xy);
    if h.a > 0.5 {
        return out(h.rgb);
    }
    let grid_pos = (pos.xy - view.offset) / view.scale;
    let cell = floor(grid_pos);
    if any(cell < vec2<f32>(0.0)) || any(cell >= view.grid) {
        return out(view.backdrop.rgb);
    }
    let at = vec2<i32>(cell);
    let texel = textureLoad(cells, at, 0);
    let quarter = quarter_of(grid_pos, cell);
    var ground_texel = texel;
    let src = grid_source(grid_pos, at, texel, quarter);
    if (src.part >= 0 && !src.from_sway) || src.swayed_away {
        let nbr = behind_cell(at, quarter);
        if nbr.x >= 0 {
            ground_texel = textureLoad(cells, nbr, 0);
        } else if src.swayed_away {
            return out(grid_post(water_bg(at, quarter, FINE_PLANTS && fine_scale()), cell));
        }
    }
    return out(grid_post(ground(at, ground_texel, quarter), cell));
}

// Passes 2 and 3: plant art in leafy parts (fs_leaves) and the others
// (fs_stems), each with its coverage as alpha, blended over fs_ground's
// colour. A fragment shows one part, so at most one of them draws it; the
// rest are discarded.
@fragment
fn fs_leaves(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    if outside_lens(pos.xy) {
        discard;
    }
    let src = plant_source(pos.xy);
    if src.part < 0 || !is_leafy(src.part) {
        discard;
    }
    let band = band_at(src.at);
    return plant_pass_colour(pos.xy, src, leaf_texel(src.at, src.texel, src.part, src.quarter, band), band);
}

@fragment
fn fs_stems(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    if outside_lens(pos.xy) {
        discard;
    }
    let src = plant_source(pos.xy);
    if src.part < 0 || is_leafy(src.part) {
        discard;
    }
    let band = band_at(src.at);
    return plant_pass_colour(pos.xy, src, stem_texel(src.at, src.texel, src.part, src.quarter, band), band);
}

// The plant art the grid shows at pixel `pos`, as fs_main finds it; part -1
// where there is none (or the palette or letterbox is there).
fn plant_source(pos: vec2<f32>) -> GridSource {
    let none = GridSource(vec2<i32>(0), vec4<f32>(0.0), -1, vec2<i32>(0), false, false);
    if hud_at(pos).a > 0.5 {
        return none;
    }
    let grid_pos = (pos - view.offset) / view.scale;
    let cell = floor(grid_pos);
    if any(cell < vec2<f32>(0.0)) || any(cell >= view.grid) {
        return none;
    }
    let at = vec2<i32>(cell);
    return grid_source(grid_pos, at, textureLoad(cells, at, 0), quarter_of(grid_pos, cell));
}

// Plant texel `pt` from `src` as the plant passes blend it: opaque where
// fs_main draws it opaque (over its shadow where nothing is behind), else
// with its alpha over the ground. Transparent fragments are discarded.
fn plant_pass_colour(pos: vec2<f32>, src: GridSource, pt: vec4<f32>, band: f32) -> vec4<f32> {
    let grid_pos = (pos - view.offset) / view.scale;
    let cell = floor(grid_pos);
    var rgb = pt.rgb;
    var alpha = pt.a;
    if pt.a >= 0.999 {
        alpha = 1.0;
    } else if !src.from_sway && behind_cell(vec2<i32>(cell), quarter_of(grid_pos, cell)).x < 0 {
        rgb = mix(plant_shadow_of(src.texel, src.part, band), pt.rgb, pt.a);
        alpha = 1.0;
    }
    if alpha <= 0.0 {
        discard;
    }
    return vec4<f32>(out(grid_post(rgb, cell)).rgb, alpha);
}

// Ground style of a cell texel (from the palette alpha byte).
fn style_of(t: vec4<f32>) -> i32 {
    let mat = i32(round(t.r * 255.0));
    return i32(round(textureLoad(palette, vec2<i32>(mat, 0), 0).a * 255.0));
}

// Ground style of the cell at `at`, or STYLE_DEFAULT outside the grid.
fn style_at(at: vec2<i32>) -> i32 {
    if !inside(at) {
        return STYLE_DEFAULT;
    }
    return style_of(textureLoad(cells, at, 0));
}

// Whether `at` holds the same material id as `mat`.
fn same_mat(at: vec2<i32>, mat: i32) -> bool {
    return inside(at) && i32(round(textureLoad(cells, at, 0).r * 255.0)) == mat;
}

// Whether `at` is an open fluid (water, air, liquid fertiliser or bubble), so
// light reaches an adjacent substrate or hardscape face. Skips the palette read
// when the neighbour holds the caller's own non-fluid material `self_mat`.
fn is_open_fluid(at: vec2<i32>, self_mat: i32) -> bool {
    if !inside(at) {
        return false;
    }
    let nt = textureLoad(cells, at, 0);
    if i32(round(nt.r * 255.0)) == self_mat {
        return false;
    }
    let s = style_of(nt);
    return s == STYLE_WATER || s == STYLE_AIR || s == STYLE_FERTILISER || s == STYLE_BUBBLE;
}

// Per-texel hash mixed with a cell's variant byte so moving powders carry
// their grain pattern while settled cells stay static.
fn hash_grain(p: vec2<i32>, v: u32) -> u32 {
    let ox = i32((v & 15u) * 7u);
    let oy = i32(((v >> 4u) & 15u) * 11u);
    return hash(p + vec2<i32>(ox, oy)) ^ (v * 0x9e3779b1u);
}

// Depth darkening multiplier for water/fertiliser at vertical position `y`.
fn depth_factor(y: f32) -> f32 {
    let depth = clamp((y - view.surface_row) / max(view.grid.y - view.surface_row, 1.0), 0.0, 1.0);
    return 1.0 - view.depth_dim * depth;
}

// Sub-cell background water colour at `(at, quarter)` (used behind bubbles,
// algae fringes and detritus edges).
fn water_bg(at: vec2<i32>, quarter: vec2<i32>, fine: bool) -> vec3<f32> {
    let mat = max(i32(round(view.plant_parts.w)), 0);
    let base = textureLoad(palette, vec2<i32>(mat, 0), 0).rgb;
    let sub = quarter / 2;
    let p = select(at * 2 + sub, at * 4 + quarter, fine);
    let sub_y = select((f32(sub.y) + 0.5) * 0.5, (f32(quarter.y) + 0.5) * 0.25, fine);
    let h = hash(vec2<i32>(p.x / 2, p.y));
    let shimmer = 1.0 + (f32(h & 7u) - 3.5) * 0.006;
    return base * depth_factor(f32(at.y) + sub_y) * shimmer;
}

// Sub-cell rows trimmed off the top of an exposed loose-material cell (sand,
// gravel, aquasoil, detritus) so 4x cells form 1-texel slopes and soft crests
// instead of 4x4 stair-steps (2x: 1-sub-cell bevels). In quarter rows when
// `fine`, else in 2x sub rows; 0 when the cell is buried. Callers pass the
// open-fluid flags they already read above, left and right, so no
// neighbour is read twice.
fn loose_top(open_up: bool, open_l: bool, open_r: bool, at: vec2<i32>, quarter: vec2<i32>, fine: bool) -> i32 {
    if !open_up {
        return 0;
    }
    let sub = quarter / 2;
    if fine {
        let px = at.x * 4 + quarter.x;
        let ripple = i32((hash(vec2<i32>(px, at.y)) & 3u) == 0u);
        if open_l && open_r {
            // Peak or stray grain: low rounded mound.
            let dist = select(1 - quarter.x, quarter.x - 2, quarter.x >= 2);
            return 1 + dist + ripple;
        } else if open_l {
            return 3 - quarter.x;
        } else if open_r {
            return quarter.x;
        }
        // Flat top: subtle 1-texel ripple.
        return ripple;
    }
    if open_l && open_r {
        return 1;
    } else if open_l {
        return 1 - sub.x;
    } else if open_r {
        return sub.x;
    }
    return 0;
}

// `loose_top` for a cell whose caller only read the cell above: the side
// neighbours are read only when it is open.
fn loose_top_at(at: vec2<i32>, mat: i32, open_up: bool, quarter: vec2<i32>, fine: bool) -> i32 {
    if !open_up {
        return 0;
    }
    let open_l = is_open_fluid(at + vec2<i32>(-1, 0), mat);
    let open_r = is_open_fluid(at + vec2<i32>(1, 0), mat);
    return loose_top(true, open_l, open_r, at, quarter, fine);
}

// A rigid cell's (rock, driftwood) sub-cell outline: bit `qy * 4 + qx` per
// quarter, 0xffff when whole. The renderer packs it into the state bytes in
// place of the body id (grid.rs `pack`, the sim's `world::hardscape`).
fn outline_of(t: vec4<f32>) -> u32 {
    return u32(round(t.b * 255.0)) | (u32(round(t.a * 255.0)) << 8u);
}

// The outline of neighbour `at` if it holds rigid material `mat`, else 0.
fn outline_at(at: vec2<i32>, mat: i32) -> u32 {
    if !inside(at) {
        return 0u;
    }
    let t = textureLoad(cells, at, 0);
    if i32(round(t.r * 255.0)) != mat {
        return 0u;
    }
    return outline_of(t);
}

fn quarter_set(m: u32, q: vec2<i32>) -> bool {
    return ((m >> u32(q.y * 4 + q.x)) & 1u) != 0u;
}

// Bits set in a 16-bit mask (portable: GLSL ES 3.00 lacks bitCount).
fn ones16(m: u32) -> i32 {
    var v = m - ((m >> 1u) & 0x5555u);
    v = (v & 0x3333u) + ((v >> 2u) & 0x3333u);
    v = (v + (v >> 4u)) & 0x0f0fu;
    return i32((v + (v >> 8u)) & 0x1fu);
}

// Whether the rigid texel at `quarter` of a partial cell (outline `own`) is
// drawn: its quarter is covered (at 2x, at least two of its sub-cell's four),
// or the side(s) the outline is cut back from face no open fluid (so a round
// stone bedded in sand stays square against the sand rather than showing
// water). The cut faces the half of the cell with fewer covered quarters,
// up or down and left or right; a balanced axis checks the texel's own half.
fn rigid_drawn(at: vec2<i32>, mat: i32, own: u32, quarter: vec2<i32>, fine: bool) -> bool {
    var covered = quarter_set(own, quarter);
    if !fine {
        let b = (quarter / 2) * 2;
        let n = u32(quarter_set(own, b)) + u32(quarter_set(own, b + vec2<i32>(1, 0)))
            + u32(quarter_set(own, b + vec2<i32>(0, 1))) + u32(quarter_set(own, b + vec2<i32>(1, 1)));
        covered = n >= 2u;
    }
    if covered {
        return true;
    }
    let tb = ones16(own & 0x00ffu) - ones16(own & 0xff00u);
    let lr = ones16(own & 0x3333u) - ones16(own & 0xccccu);
    let sy = select(select(1, -1, tb < 0), select(1, -1, quarter.y < 2), tb == 0);
    let sx = select(select(1, -1, lr < 0), select(1, -1, quarter.x < 2), lr == 0);
    return !is_open_fluid(at + vec2<i32>(sx, 0), mat) && !is_open_fluid(at + vec2<i32>(0, sy), mat);
}

// Whether quarter `q` (up to two quarters outside the cell on one axis) is
// covered, given the cell's outline `own` and those of the neighbours above,
// below, left and right (only the ones `q` can reach are read).
fn rigid_cover(own: u32, n: vec4<u32>, q: vec2<i32>) -> bool {
    if q.y < 0 {
        return quarter_set(n.x, q + vec2<i32>(0, 4));
    }
    if q.y > 3 {
        return quarter_set(n.y, q - vec2<i32>(0, 4));
    }
    if q.x < 0 {
        return quarter_set(n.z, q + vec2<i32>(4, 0));
    }
    if q.x > 3 {
        return quarter_set(n.w, q - vec2<i32>(4, 0));
    }
    return quarter_set(own, q);
}

// At 4x, how far the texel at `q` is from the rigid outline's edge above,
// below, left and right: 1 on the edge, 2 one texel in, 3 farther. Reads at
// most the neighbour above or below and the one to the side at the cell's
// edge, as the cell-edge bevels did.
fn rigid_edges(at: vec2<i32>, mat: i32, own: u32, q: vec2<i32>) -> vec4<i32> {
    var n = vec4<u32>(0xffffu);
    if q.y < 2 {
        n.x = outline_at(at + vec2<i32>(0, -1), mat);
    } else {
        n.y = outline_at(at + vec2<i32>(0, 1), mat);
    }
    if q.x == 0 {
        n.z = outline_at(at + vec2<i32>(-1, 0), mat);
    } else if q.x == 3 {
        n.w = outline_at(at + vec2<i32>(1, 0), mat);
    }
    let up = select(select(3, 2, !rigid_cover(own, n, q + vec2<i32>(0, -2))), 1, !rigid_cover(own, n, q + vec2<i32>(0, -1)));
    let down = select(select(3, 2, !rigid_cover(own, n, q + vec2<i32>(0, 2))), 1, !rigid_cover(own, n, q + vec2<i32>(0, 1)));
    let left = select(3, 1, !rigid_cover(own, n, q + vec2<i32>(-1, 0)));
    let right = select(3, 1, !rigid_cover(own, n, q + vec2<i32>(1, 0)));
    return vec4<i32>(up, down, left, right);
}

// The colour of a non-plant cell: at scale 1 a flat palette lookup with
// whole-cell depth darkening; at scale >= 2 (2x2) and fine scales (4x4),
// sub-cell background gradients, sand/gravel/aquasoil grains, stone/wood
// textures and refined bubbles/algae/detritus (before the daylight tint).
fn ground(at: vec2<i32>, t: vec4<f32>, quarter: vec2<i32>) -> vec3<f32> {
    let mat = i32(round(t.r * 255.0));
    let v = u32(round(t.g * 255.0));
    if view.scale < 2.0 {
        let c = textureLoad(palette, vec2<i32>(mat, i32(v)), 0);
        let s = i32(round(c.a * 255.0));
        let tinted = select(0.0, 1.0, s == STYLE_WATER || s == STYLE_FERTILISER);
        let depth = clamp((f32(at.y) - view.surface_row) / max(view.grid.y - view.surface_row, 1.0), 0.0, 1.0);
        return c.rgb * (1.0 - view.depth_dim * depth * tinted);
    }

    let fine = FINE_PLANTS && fine_scale();
    let sub = quarter / 2;
    let p = select(at * 2 + sub, at * 4 + quarter, fine);
    // Variant 0 is unglued base shade 0; variant 1 is glued base shade 0
    // (for rigid bodies, palette.rs shades odd variants amber).
    let base_v = i32(v & 1u);
    let base_texel = textureLoad(palette, vec2<i32>(mat, base_v), 0);
    let base = base_texel.rgb;
    let style = i32(round(base_texel.a * 255.0));

    if style == STYLE_WATER || style == STYLE_FERTILISER {
        let sub_y = select((f32(sub.y) + 0.5) * 0.5, (f32(quarter.y) + 0.5) * 0.25, fine);
        let d = depth_factor(f32(at.y) + sub_y);
        var rgb: vec3<f32>;
        if style == STYLE_WATER {
            // Subtle horizontal-biased pixel-art water grain.
            let h1 = hash(vec2<i32>(i32(u32(p.x) >> 1u), p.y));
            let h2 = hash(p);
            let grain = 1.0 + (f32(h1 & 7u) - 3.5) * 0.005 + (f32(h2 & 3u) - 1.5) * 0.004;
            rgb = base * d * grain;
        } else {
            // Liquid fertiliser: wispy plume variation driven by cell variant.
            let h = hash_grain(vec2<i32>(i32(u32(p.x) >> 1u), p.y), v);
            let plume = 1.0 + (f32(h & 15u) - 7.5) * 0.008;
            rgb = base * d * plume;
        }
        // Crisp water-surface highlight where water meets air. Only cells near
        // the water surface can touch air above, so skip the neighbour read
        // everywhere else in the water column.
        if at.y > 0 && abs(f32(at.y) - view.surface_row) <= 8.0 && (!fine || quarter.y < 3) {
            let above = textureLoad(cells, at + vec2<i32>(0, -1), 0);
            if i32(round(above.r * 255.0)) != mat && style_of(above) == STYLE_AIR {
                let ripple = hash(vec2<i32>(p.x / 2, at.y)) & 3u;
                let surface_tint = vec3<f32>(0.72, 0.90, 0.98);
                if fine {
                    if quarter.y == 0 {
                        let k = select(0.32, 0.42, ripple == 0u);
                        rgb = mix(rgb, surface_tint, k);
                    } else if quarter.y == 1 {
                        let k = select(0.10, 0.16, ripple < 2u);
                        rgb = mix(rgb, surface_tint, k);
                    } else if quarter.y == 2 {
                        rgb = mix(rgb, surface_tint, 0.04);
                    }
                } else {
                    if sub.y == 0 {
                        let k = select(0.24, 0.34, ripple == 0u);
                        rgb = mix(rgb, surface_tint, k);
                    } else {
                        rgb = mix(rgb, surface_tint, 0.06);
                    }
                }
            }
        }
        return clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    }

    if style == STYLE_AIR {
        let sub_y = select((f32(sub.y) + 0.5) * 0.5, (f32(quarter.y) + 0.5) * 0.25, fine);
        let t_sky = clamp((f32(at.y) + sub_y) / max(view.surface_row, 1.0), 0.0, 1.0);
        // Gentle top-to-surface air lift + super subtle sub-cell grain.
        let lift = mix(1.03, 0.97, t_sky);
        let h = hash(p);
        let grain = 1.0 + (f32(h & 7u) - 3.5) * 0.004;
        return clamp(base * lift * grain, vec3<f32>(0.0), vec3<f32>(1.0));
    }

    if style == STYLE_SAND {
        let open_up = is_open_fluid(at + vec2<i32>(0, -1), mat);
        let open_l = is_open_fluid(at + vec2<i32>(-1, 0), mat);
        let open_r = is_open_fluid(at + vec2<i32>(1, 0), mat);
        let h = hash_grain(p, v);
        let top_row = loose_top(open_up, open_l, open_r, at, quarter, fine);
        if select(sub.y, quarter.y, fine) < top_row {
            return water_bg(at, quarter, fine);
        }
        let r = h & 31u;
        // Multi-grain quartz palette: warm base, soft tan shadow, bright glint,
        // pale quartz sparkle and occasional dark mineral fleck.
        var k = 1.0;
        var tint = vec3<f32>(1.0);
        if r < 2u {
            k = 0.80;
            tint = vec3<f32>(0.95, 0.93, 0.90);
        } else if r < 9u {
            k = 0.91;
        } else if r < 15u {
            k = 1.08;
        } else if r < 18u {
            k = 1.13;
            tint = vec3<f32>(1.02, 1.02, 1.00);
        }
        // Subtle horizontal bedding strata.
        let band = (u32(p.y) >> 1u) & 3u;
        k = k * select(1.0, 0.975, band == 0u);
        // Top-edge water lighting along the actual exposed contour.
        if open_up {
            if fine {
                let dy = quarter.y - top_row;
                if dy == 0 {
                    k = k * 1.13;
                } else if dy == 1 {
                    k = k * 1.04;
                } else if quarter.y == 3 {
                    k = k * 0.95;
                }
            } else {
                k = k * select(0.96, 1.10, sub.y == top_row);
            }
        } else {
            k = k * 0.96;
        }
        if select(sub.x == 0, quarter.x == 0, fine) && open_l {
            k = k * 1.07;
        } else if select(sub.x == 1, quarter.x == 3, fine) && open_r {
            k = k * 0.93;
        }
        return clamp(base * tint * k, vec3<f32>(0.0), vec3<f32>(1.0));
    }

    if style == STYLE_GRAVEL {
        // Whole rounded river pebbles laid out per cell from its variant (so
        // moving gravel carries them): mostly one stone filling the cell, else
        // a stone beside a dark gap row or column, flipped at random so gaps
        // don't line up. Each stone's top follows the trimmed contour
        // (`loose_top`), so a sloped or rippled surface shows lit, rounded
        // stones rather than sliced ones.
        let open_up = is_open_fluid(at + vec2<i32>(0, -1), mat);
        let top_row = loose_top_at(at, mat, open_up, quarter, fine);
        let q = select(sub, quarter, fine);
        if q.y < top_row {
            return water_bg(at, quarter, fine);
        }
        // Cell-constant layout bits (glue bit dropped so gluing keeps the look).
        let cv = hash(vec2<i32>(i32(v >> 1u), 0x4b1d));
        let cell_n = (f32((v >> 3u) & 3u) - 1.5) * 0.025;
        let spek = (hash_grain(p, v) & 7u) == 0u;
        var k: f32;
        var warm: vec3<f32>;
        if fine {
            let fx = select(q.x, 3 - q.x, ((cv >> 8u) & 1u) != 0u);
            // Stone box (x0, y0, x1, y1) within the cell, or none (gap).
            var b = vec4<i32>(0, 0, 3, 3);
            switch cv & 7u {
                case 4u: {
                    // A wide stone over a gap row.
                    b = vec4<i32>(0, 0, 3, 2);
                }
                case 5u: {
                    // A tall stone beside a gap column.
                    b = vec4<i32>(0, 0, 2, 3);
                }
                case 6u: {
                    // A wide stone under a gap row.
                    b = vec4<i32>(0, 1, 3, 3);
                }
                default: {
                    // One stone filling the cell.
                }
            }
            let inside_b = fx >= b.x && fx <= b.z && q.y >= b.y && q.y <= b.w;
            // The stone's visible top in this column: its own top or the
            // trimmed contour, whichever is lower.
            let top = max(b.y, top_row);
            if !inside_b {
                // Dark grit between stones.
                k = select(0.64, 0.70, spek) + cell_n;
                warm = vec3<f32>(0.97, 0.98, 1.02);
            } else {
                let ph = hash(vec2<i32>(i32(cv >> 3u), 1));
                k = 0.92 + f32(ph & 7u) * 0.04 + cell_n;
                let tone = (ph >> 3u) & 3u;
                warm = vec3<f32>(0.96, 0.99, 1.04);
                if tone == 1u {
                    warm = vec3<f32>(1.06, 1.00, 0.91);
                } else if tone == 2u {
                    warm = vec3<f32>(1.02, 1.00, 0.97);
                }
                let left = fx == b.x;
                let right = fx == b.z;
                let crown = q.y == top;
                let bottom = q.y == b.w;
                if right && bottom {
                    k = k * 0.62;
                } else if (left || right) && (crown || bottom) {
                    // Rounded corners, nearly as dark as the gaps.
                    k = k * select(0.68, 0.74, crown);
                } else if bottom || right {
                    k = k * 0.86;
                } else if crown && fx == b.x + 1 {
                    // Specular glint near the top-left.
                    k = k * 1.18;
                } else if (crown && fx == b.x + 2) || (q.y == top + 1 && fx == b.x + 1) {
                    k = k * 1.08;
                } else if spek {
                    k = k * 0.95;
                }
                if crown && open_up {
                    // Lit by the water above along the exposed contour.
                    k = k * 1.06;
                }
            }
        } else {
            // 2x: one stone per cell, lit top-left and shadowed bottom-right;
            // one cell in four leaves its bottom-right as a dark gap.
            let ph = hash(vec2<i32>(i32(cv >> 3u), 0));
            k = 0.88 + f32(ph & 7u) * 0.032 + cell_n;
            warm = select(
                vec3<f32>(0.96, 0.99, 1.04),
                vec3<f32>(1.05, 1.00, 0.93),
                ((ph >> 3u) & 1u) != 0u,
            );
            let flip = (cv & 4u) != 0u;
            let lx = select(q.x, 1 - q.x, flip);
            if q.y == top_row {
                k = k * select(1.08, 1.14, open_up && lx == 0);
            } else if lx == 1 && q.y == 1 {
                k = k * select(0.84, 0.66, (cv & 3u) == 0u);
            }
        }
        if !open_up {
            k = k * 0.96;
        }
        return clamp(base * warm * k, vec3<f32>(0.0), vec3<f32>(1.0));
    }

    if style == STYLE_AQUASOIL {
        // Staggered dark rounded matte soil pellets (3x3 texels at 4x,
        // offset 2x2 at 2x): smaller than gravel pebbles, breaking the cell grid.
        let open_up = is_open_fluid(at + vec2<i32>(0, -1), mat);
        let top_row = loose_top_at(at, mat, open_up, quarter, fine);
        if select(sub.y, quarter.y, fine) < top_row {
            return water_bg(at, quarter, fine);
        }
        var k: f32;
        if fine {
            let ry = p.y / 3;
            let shift = (ry & 1) + ((ry / 2) & 1);
            let sx = p.x + shift;
            let rx = sx / 3;
            let lx = sx - rx * 3;
            let ly = p.y - ry * 3;
            let ph = hash(vec2<i32>(rx, ry));
            let cell_n = (f32((v >> 3u) & 3u) - 1.5) * 0.02;
            k = 0.90 + f32(ph & 7u) * 0.028 + cell_n;
            if lx == 2 && ly == 2 {
                k = k * 0.74;
            } else if lx == 0 && ly == 0 {
                k = k * 1.15;
            } else if (lx == 1 && ly == 0) || (lx == 0 && ly == 1) {
                k = k * 1.06;
            } else if lx == 2 || ly == 2 {
                k = k * 0.85;
            }
            if open_up && quarter.y == top_row {
                k = k * 1.15;
            }
        } else {
            let ry = p.y / 2;
            let sx = p.x + (ry & 1);
            let rx = sx / 2;
            let lx = sx - rx * 2;
            let ly = p.y - ry * 2;
            let ph = hash(vec2<i32>(rx, ry));
            let cell_n = (f32((v >> 3u) & 3u) - 1.5) * 0.02;
            k = 0.91 + f32(ph & 7u) * 0.026 + cell_n;
            let tl = lx == 0 && ly == 0;
            let br = lx == 1 && ly == 1;
            k = k * select(select(0.99, 0.82, br), 1.11, tl);
            if open_up && sub.y == top_row {
                k = k * 1.11;
            }
        }
        if !open_up {
            k = k * 0.95;
        }
        return clamp(base * k, vec3<f32>(0.0), vec3<f32>(1.0));
    }

    if style == STYLE_ROCK {
        // Sub-cell outline: round brushes draw round stones.
        let own = outline_of(t);
        if own != 0xffffu && !rigid_drawn(at, mat, own, quarter, fine) {
            return water_bg(at, quarter, fine);
        }
        // World-space faceted stone strata + fine mineral grain.
        let facet = hash(vec2<i32>((p.x + p.y / 2) / 3, (p.y - p.x / 4) / 3));
        let grain = hash(p);
        let cell_n = (f32((v >> 2u) & 3u) - 1.5) * 0.018;
        var k = 1.0 + (f32(facet & 7u) - 3.5) * 0.028 + (f32(grain & 7u) - 3.5) * 0.012 + cell_n;
        // Occasional dark micro-crack along a facet seam.
        if ((facet >> 4u) & 15u) == 0u && ((grain >> 3u) & 3u) == 0u {
            k = k * 0.86;
        }
        // Crisp edge bevels on exposed faces (read only the neighbours this texel can touch).
        if fine {
            // Bevels along the outline: lit top, shadowed underside.
            let e = rigid_edges(at, mat, own, quarter);
            if e.x <= 2 {
                k = k * select(1.06, 1.15, e.x == 1);
            }
            if e.z == 1 {
                k = k * 1.08;
            }
            if e.y <= 2 {
                k = k * select(0.92, 0.82, e.y == 1);
            }
            if e.w == 1 {
                k = k * 0.88;
            }
        } else {
            if sub.y == 0 && !same_mat(at + vec2<i32>(0, -1), mat) {
                k = k * 1.12;
            }
            if sub.x == 0 && !same_mat(at + vec2<i32>(-1, 0), mat) {
                k = k * 1.06;
            }
            if sub.y == 1 && !same_mat(at + vec2<i32>(0, 1), mat) {
                k = k * 0.86;
            }
            if sub.x == 1 && !same_mat(at + vec2<i32>(1, 0), mat) {
                k = k * 0.90;
            }
        }
        return clamp(base * k, vec3<f32>(0.0), vec3<f32>(1.0));
    }

    if style == STYLE_DRIFTWOOD {
        let own = outline_of(t);
        if own != 0xffffu && !rigid_drawn(at, mat, own, quarter, fine) {
            return water_bg(at, quarter, fine);
        }
        // Flowing horizontal wood grain fibres with gentle organic wave.
        let wave = i32(hash(vec2<i32>(p.x / 6, p.y / 5)) & 3u) - 1;
        let row = u32(p.y + wave + 1024);
        let stripe = row % 4u;
        var k = 1.0;
        if stripe == 0u {
            k = 0.86;
        } else if stripe == 1u {
            k = 1.08;
        } else if stripe == 2u {
            k = 0.98;
        } else {
            k = 1.03;
        }
        let fibre = hash(vec2<i32>(p.x / 2, p.y));
        k = k * (1.0 + (f32(fibre & 7u) - 3.5) * 0.015);
        // Bark top highlight, deep underside shadow, slightly darker ends.
        if fine {
            let e = rigid_edges(at, mat, own, quarter);
            if e.x <= 2 {
                k = k * select(1.05, 1.14, e.x == 1);
            }
            if e.y <= 2 {
                k = k * select(0.90, 0.78, e.y == 1);
            }
            if e.z == 1 || e.w == 1 {
                k = k * 0.88;
            }
        } else {
            if sub.y == 0 && !same_mat(at + vec2<i32>(0, -1), mat) {
                k = k * 1.10;
            }
            if sub.y == 1 && !same_mat(at + vec2<i32>(0, 1), mat) {
                k = k * 0.82;
            }
            if (sub.x == 0 && !same_mat(at + vec2<i32>(-1, 0), mat))
                || (sub.x == 1 && !same_mat(at + vec2<i32>(1, 0), mat))
            {
                k = k * 0.90;
            }
        }
        return clamp(base * k, vec3<f32>(0.0), vec3<f32>(1.0));
    }

    if style == STYLE_BUBBLE {
        let same_u = style_at(at + vec2<i32>(0, -1)) == STYLE_BUBBLE;
        let same_d = style_at(at + vec2<i32>(0, 1)) == STYLE_BUBBLE;
        let same_l = style_at(at + vec2<i32>(-1, 0)) == STYLE_BUBBLE;
        let same_r = style_at(at + vec2<i32>(1, 0)) == STYLE_BUBBLE;
        let isolated = !same_u && !same_d && !same_l && !same_r;
        let bg = water_bg(at, quarter, fine);
        if isolated {
            if fine {
                let corner = (quarter.x == 0 || quarter.x == 3) && (quarter.y == 0 || quarter.y == 3);
                if corner {
                    return bg;
                }
                if quarter.x == 1 && quarter.y == 1 {
                    return mix(base, vec3<f32>(1.0), 0.65);
                }
                let rim = quarter.x == 0 || quarter.x == 3 || quarter.y == 0 || quarter.y == 3;
                if rim {
                    let lit = quarter.x == 0 || quarter.y == 0;
                    return mix(bg, base * select(0.82, 1.05, lit), select(0.55, 0.80, lit));
                }
                // Translucent interior showing water behind.
                return mix(bg, base, 0.35);
            } else {
                if sub.x == 0 && sub.y == 0 {
                    return mix(base, vec3<f32>(1.0), 0.55);
                }
                if sub.x == 1 && sub.y == 1 {
                    return mix(bg, base * 0.80, 0.50);
                }
                return mix(bg, base, 0.68);
            }
        }
        // Multi-cell bubble: round outer convex corners and add a glossy rim.
        if fine {
            let cut_tl = !same_u && !same_l && quarter.x == 0 && quarter.y == 0;
            let cut_tr = !same_u && !same_r && quarter.x == 3 && quarter.y == 0;
            let cut_bl = !same_d && !same_l && quarter.x == 0 && quarter.y == 3;
            let cut_br = !same_d && !same_r && quarter.x == 3 && quarter.y == 3;
            if cut_tl || cut_tr || cut_bl || cut_br {
                return bg;
            }
            let top_rim = (!same_u && quarter.y == 0) || (!same_l && quarter.x == 0);
            let bot_rim = (!same_d && quarter.y == 3) || (!same_r && quarter.x == 3);
            if top_rim {
                return mix(base, vec3<f32>(1.0), 0.55);
            }
            if bot_rim {
                return mix(bg, base * 0.84, 0.72);
            }
            return mix(bg, base, 0.72);
        }
        let top_rim = (!same_u && sub.y == 0) || (!same_l && sub.x == 0);
        let bot_rim = (!same_d && sub.y == 1) || (!same_r && sub.x == 1);
        if top_rim {
            return mix(base, vec3<f32>(1.0), 0.45);
        }
        if bot_rim {
            return mix(bg, base * 0.85, 0.75);
        }
        return mix(bg, base, 0.78);
    }

    if style == STYLE_ALGAE {
        let d = sub * 2 - 1;
        let wx = is_water(at + vec2<i32>(d.x, 0));
        let wy = is_water(at + vec2<i32>(0, d.y));
        let exposed = wx || wy;
        let h = hash_grain(p, v);
        if exposed {
            let outer = select(
                true,
                (wx && (quarter.x == 0 || quarter.x == 3))
                    || (wy && (quarter.y == 0 || quarter.y == 3)),
                fine,
            );
            if outer && (h & 3u) == 0u {
                return water_bg(at, quarter, fine);
            }
        }
        let k = 0.86 + f32((h >> 2u) & 7u) * 0.04;
        return clamp(base * k, vec3<f32>(0.0), vec3<f32>(1.0));
    }

    if style == STYLE_DETRITUS {
        let open_up = is_open_fluid(at + vec2<i32>(0, -1), mat);
        let water_up = is_water(at + vec2<i32>(0, -1));
        let h = hash_grain(p, v);
        // Detritus trims only under water (in air it is a resting heap).
        if water_up && select(sub.y, quarter.y, fine) < loose_top_at(at, mat, open_up, quarter, fine) {
            return water_bg(at, quarter, fine);
        }
        let k = 0.84 + f32((h >> 2u) & 7u) * 0.042 + select(0.0, 0.06, open_up && sub.y == 0);
        return clamp(base * k, vec3<f32>(0.0), vec3<f32>(1.0));
    }

    // STYLE_DEFAULT: clean sub-cell grain with gentle top/bottom edge cues.
    let h = hash_grain(p, v);
    var k = 1.0 + (f32(h & 7u) - 3.5) * 0.015;
    if select(sub.y == 0, quarter.y == 0, fine) && is_open_fluid(at + vec2<i32>(0, -1), mat) {
        k = k * 1.07;
    } else if select(sub.y == 1, quarter.y == 3, fine) && is_open_fluid(at + vec2<i32>(0, 1), mat) {
        k = k * 0.93;
    }
    return clamp(base * k, vec3<f32>(0.0), vec3<f32>(1.0));
}

// A plant cell's depth band (D21).
fn band_at(at: vec2<i32>) -> f32 {
    return round(textureLoad(bands, at, 0).r * 255.0);
}

// `rgb` seen through `depth` bands of water (D21): faded towards the haze
// and dimmed a little, more towards the back, unchanged at the front.
fn depth_cue(rgb: vec3<f32>, depth: f32) -> vec3<f32> {
    let t = clamp(depth / BACK_BAND, 0.0, 1.0);
    return mix(rgb, HAZE, HAZE_MIX * t) * (1.0 - HAZE_DIM * t);
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
    if m == view.plant_parts2.x {
        return PART_RUNNER;
    }
    if m == view.plant_parts2.y {
        return PART_FLOAT;
    }
    if m == view.plant_parts2.z {
        return PART_RHIZOME;
    }
    return -1;
}

// Whether a plant part cell sways when optional sway is active (T6d):
// leaves and upper stems sway; roots, runners, rhizomes, floaters, crowns
// (pos 0) and mats (pos 0..3) stay anchored.
fn is_swaying_part(part: i32, t: vec4<f32>) -> bool {
    return part == PART_LEAF || (part == PART_STEM && position_of(t) >= 4);
}

// Gentle horizontal sway offset in cells (-0.5..+0.5, 1 cell peak-to-peak)
// as a smooth wave of grid position and sway phase (view.target_size.z).
fn sway_dx(grid_pos: vec2<f32>) -> f32 {
    let phase = view.target_size.z;
    let w = sin(phase * 1.6 + grid_pos.y * 0.14 + grid_pos.x * 0.05) * 0.65
        + sin(phase * 0.9 - grid_pos.y * 0.08 + grid_pos.x * 0.11) * 0.35;
    return w * 0.5;
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

// How many cells of the same plant as `me` follow on from `at` along `d`,
// up to 3 (how far a ball runs on that way). `STEM_VELVET` calls it four
// times, so a ball's fragments make up to 12 extra loads; only marimo cells
// pay this, and a ball is at most 50 cells.
fn ball_run(at: vec2<i32>, d: vec2<i32>, me: vec4<f32>) -> i32 {
    var n = 0;
    for (var k = 1; k <= 3; k++) {
        if kin(at, d * k, me).x < 0 {
            break;
        }
        n = k;
    }
    return n;
}

// Whether the cell at `at` is water (what a mat's fringe frays into).
fn is_water(at: vec2<i32>) -> bool {
    return inside(at) && round(textureLoad(cells, at, 0).r * 255.0) == view.plant_parts.w;
}

// A static per-texel hash (texel coordinates in the grid's 2x or 4x texel space).
fn hash(p: vec2<i32>) -> u32 {
    var h = (u32(p.x) * 0x8da6b343u) ^ (u32(p.y) * 0xd8163841u);
    h = (h ^ (h >> 13u)) * 0x5bd1e995u;
    return h ^ (h >> 15u);
}

// The first non-plant neighbour on `quarter`'s side (across, then above or
// below, then diagonally), or (-1, -1) when the plant fills all three.
fn behind_cell(at: vec2<i32>, quarter: vec2<i32>) -> vec2<i32> {
    let sub = quarter / 2;
    let d = sub * 2 - 1;
    let a = at + vec2<i32>(d.x, 0);
    if inside(a) && part_of(textureLoad(cells, a, 0)) < 0 {
        return a;
    }
    let b = at + vec2<i32>(0, d.y);
    if inside(b) && part_of(textureLoad(cells, b, 0)) < 0 {
        return b;
    }
    let c = at + d;
    if inside(c) && part_of(textureLoad(cells, c, 0)) < 0 {
        return c;
    }
    return vec2<i32>(-1, -1);
}

// Whether the pipeline may draw four-texel plants: the renderer compiles a
// second grid pipeline with this false for scales that don't, so the fine
// code is dropped there rather than skipped per fragment (renderer.rs).
override FINE_PLANTS: bool = true;

// Quarter-cell art is shown only when each texel covers whole screen pixels.
fn fine_scale() -> bool {
    return u32(view.scale) % 4u == 0u;
}

// A plant cell's colour (T6d), in band `band` (D21): its species' shape
// decides whether the texel is covered, and a shade from the part's ramp by
// position along the plant and within the shape; the plant's health tint
// multiplies it, and the depth cue fades it. Uncovered texels show what is
// behind. Scale 1 fills every texel. Alpha is 1 where the texel is
// covered, 0 where it shows what is behind (fs_sprite draws a creature
// there even when the plant cell is in front of it).
fn plant_texel(at: vec2<i32>, me: vec4<f32>, part: i32, quarter: vec2<i32>, band: f32) -> vec4<f32> {
    let plant = plant_of(me);
    var cover: vec2<i32>;
    if is_leafy(part) {
        cover = leaf_cover(at, me, part, quarter, plant);
    } else {
        cover = stem_cover(at, me, part, quarter, plant);
    }
    return plant_colour(plant, part, cover, band);
}

// The split grid's plant passes draw leafy and other parts separately, each
// reaching only its own shape code (fs_leaves, fs_stems).
fn leaf_texel(at: vec2<i32>, me: vec4<f32>, part: i32, quarter: vec2<i32>, band: f32) -> vec4<f32> {
    let plant = plant_of(me);
    return plant_colour(plant, part, leaf_cover(at, me, part, quarter, plant), band);
}

fn stem_texel(at: vec2<i32>, me: vec4<f32>, part: i32, quarter: vec2<i32>, band: f32) -> vec4<f32> {
    let plant = plant_of(me);
    return plant_colour(plant, part, stem_cover(at, me, part, quarter, plant), band);
}

// The plant texture's texel for plant cell `me`: health (r), colouration
// (g) and species id (a).
fn plant_of(me: vec4<f32>) -> vec4<f32> {
    return textureLoad(plant_tints, vec2<i32>(round(me.ba * 255.0)), 0);
}

// Leaves and floating leaves, drawn from the leaf ramp.
fn is_leafy(part: i32) -> bool {
    return part == PART_LEAF || part == PART_FLOAT;
}

// Whether a leafy part's texel is covered (x: 1 or 0) and its ramp shade
// before clamping (y), from the species' leaf shape.
fn leaf_cover(at: vec2<i32>, me: vec4<f32>, part: i32, quarter: vec2<i32>, plant: vec4<f32>) -> vec2<i32> {
    let sub = quarter / 2;
    let species = i32(round(plant.a * 255.0));
    let shapes = vec4<i32>(round(textureLoad(plant_species, vec2<i32>(SHAPES_COLUMN, species), 0) * 255.0));
    let sizes = round(textureLoad(plant_species, vec2<i32>(SIZES_COLUMN, species), 0) * 255.0);
    let pos = position_of(me);
    let fine = FINE_PLANTS && fine_scale();
    let top = sub.y == 0;
    var covered = true;
    var step = noise_step(me);
    if part == PART_LEAF && shapes.x >= LEAF_RIBBON {
        // A rosette's or carpet's blade (T6c), growing up from the crown and
        // stepping sideways where it leans: lighter towards its tip.
        let up = kin(at, vec2<i32>(0, -1), me);
        let left = kin(at, vec2<i32>(-1, 0), me);
        let right = kin(at, vec2<i32>(1, 0), me);
        let next = pos + 1;
        let tip = !((up.x == PART_LEAF && up.y == next) || (left.x == PART_LEAF && left.y == next)
            || (right.x == PART_LEAF && right.y == next));
        step += 1 + i32(round(f32(pos) / max(sizes.z, 1.0) * 4.0)) + select(0, 1, top);
        let shape = shapes.x;
        if shape == LEAF_GRASS {
            // One texel wide, the tip a texel lighter.
            covered = sub.x == 0;
            step += select(0, 1, tip);
            if fine {
                // A hair-thin blade with a shorter, darker one beside it.
                let second = quarter.x == 3 && (!tip || quarter.y >= 2);
                covered = quarter.x == 1 || second;
                step -= select(0, 2, second);
            }
        } else if shape == LEAF_PAD {
            // A short stalk to a round pad, its lower rim darker.
            if tip {
                covered = true;
                step += select(-1, 1, top);
            } else {
                covered = sub.x == 0;
                step -= 1;
            }
            if fine {
                if tip {
                    // A round pad: a lit crown and highlight, a dark rim
                    // below and the stalk's notch at the bottom.
                    let edge_x = quarter.x == 0 || quarter.x == 3;
                    let edge_y = quarter.y == 0 || quarter.y == 3;
                    covered = !(edge_x && edge_y) && !(quarter.y == 3 && quarter.x == 2);
                    step += select(0, 1, all(quarter == vec2<i32>(1, 1)))
                        - select(0, 1, quarter.y == 3)
                        - select(0, 1, quarter.x == 3);
                } else {
                    // A thread-thin stalk.
                    covered = quarter.x == 1;
                }
            }
        } else if shape == LEAF_LOTUS {
            // A thin petiole up to a broad leaf pad spread sideways from its
            // tip (sizes.x is the petiole length, sizes.w the pad width;
            // pad cells sit past the length, a narrower upper row at
            // length + width + 1): green mottled with maroon blotches, lit
            // along its upper edge, darker beneath, rounded at its ends.
            let len = i32(sizes.x);
            let pad_l = left.x >= 0 && left.y > len;
            let pad_r = right.x >= 0 && right.y > len;
            let pad_u = up.x >= 0 && up.y > len;
            if pos > len || pad_l || pad_r || pad_u {
                let upper = pos == len + i32(sizes.w) + 1;
                let open_l = left.x < 0;
                let open_r = right.x < 0;
                let open_u = up.x < 0;
                let open_d = kin(at, vec2<i32>(0, 1), me).x < 0;
                let blotch = (hash(at * 2 + sub) & 7u) < 2u || (hash(at) & 7u) == 0u;
                let corner = (top && open_u) || (!top && open_d && !upper);
                covered = !(corner && ((open_l && sub.x == 0) || (open_r && sub.x == 1)));
                step = 5 + select(0, 1, top && open_u) - select(0, 1, !top && open_d)
                    + noise_step(me);
                if blotch {
                    step = 1 + i32(hash(at * 2 + sub) >> 9u) % 2;
                }
                if fine {
                    // Quarter detail: rounded ends, a lit rim along the
                    // top, a dark rim under the lower row, and maroon
                    // flecks two quarters across.
                    let end = (open_l && (quarter.x == 0)) || (open_r && (quarter.x == 3));
                    let rim_u = open_u && quarter.y == 0;
                    let rim_d = open_d && quarter.y == 3;
                    covered = !(end && (rim_u || rim_d));
                    let fleck = (hash(at * 4 + quarter / 2 * 2) & 7u) < 2u;
                    step = 5 + select(0, 2, rim_u) - select(0, 2, rim_d) + noise_step(me);
                    if blotch || fleck {
                        step = 1 + i32(rim_u) + i32(hash(at) >> 9u) % 2;
                    }
                }
            } else {
                // The petiole: a bronze stalk one texel wide.
                covered = sub.x == 0;
                step = 3 + noise_step(me);
                if fine {
                    // Two quarters wide, lit on its left.
                    covered = quarter.x == 1 || quarter.x == 2;
                    step += select(0, 1, quarter.x == 1);
                }
            }
        } else if shape == LEAF_SWORD {
            // A narrow stalk, then a broad blade with a pale midrib (the
            // left texel), tapering to a point.
            if pos <= 2 && !tip {
                covered = sub.x == 0;
                step -= 1;
            } else if tip {
                covered = !top || sub.x == 0;
            } else {
                step += select(-1, 1, sub.x == 0);
            }
            if fine {
                if pos <= 2 && !tip {
                    // A slender petiole, paler on its left.
                    covered = quarter.x == 1 || quarter.x == 2;
                    step += select(0, 1, quarter.x == 1);
                } else {
                    // A pale midrib (second column), lateral veins rising
                    // to the edge every third cell, a dark rim, a blade
                    // that flares from the petiole and a pointed tip.
                    let rib = quarter.x == 1;
                    let vein = pos % 3 == 0 && quarter.x >= 2 && quarter.x + quarter.y == 4;
                    let flare = pos == 3 && quarter.y == 3;
                    let edge = quarter.x == 0 || quarter.x == 3;
                    let narrow = tip && quarter.y < 2 && edge;
                    let point = tip && quarter.y == 0 && quarter.x == 2;
                    covered = !narrow && !point && (!flare || rib || quarter.x == 2);
                    step = step - select(0, 1, tip) + select(0, 2, rib) + select(0, 2, vein)
                        - select(0, 1, quarter.x == 3) + select(0, 1, quarter.x == 0);
                }
            }
        } else {
            // Ribbon: a strap one texel wide that twists every four cells,
            // crossing to the other side where it shows its full face.
            let side = (pos / 4) % 2;
            let face = pos % 4 == 0 && !top && !tip;
            covered = sub.x == side || face;
            step += select(0, 1, face) - select(0, 1, sub.x == 1);
            if fine {
                // A strap two quarters wide, lit along one edge, that
                // slides across the cell where it twists, showing its
                // broader, paler face mid-twist.
                let twist = pos % 4 == 0 && pos > 0 && !tip;
                let after = side * 2;
                let before = 2 - after;
                let left_edge = select(after, before + (after - before) * (3 - quarter.y) / 3, twist);
                let wide = twist && (quarter.y == 1 || quarter.y == 2);
                let width = select(1, 2, wide);
                covered = quarter.x >= left_edge && quarter.x <= left_edge + width;
                step = step - select(0, 1, face) + select(0, 1, wide)
                    + select(-1, 1, quarter.x == left_edge);
            }
        }
        // Night folding on rosette, carpet and epiphyte blades (T6d): broad
        // blades and pads tuck narrower at night (view.daylight.w: 0 day, 1 night).
        let night = view.daylight.w;
        if night > 0.01 {
            if fine {
                if shape == LEAF_SWORD && night >= 0.6 && pos > 2 && (quarter.x == 0 || quarter.x == 3) {
                    covered = false;
                } else if shape == LEAF_PAD && night >= 0.6 && tip && (quarter.x == 3 || quarter.y == 3) {
                    covered = false;
                } else if shape == LEAF_GRASS && night >= 0.75 && quarter.x == 3 {
                    covered = false;
                }
            } else {
                if shape == LEAF_SWORD && night >= 0.75 && pos > 2 && sub.x == 1 {
                    covered = false;
                } else if shape == LEAF_PAD && night >= 0.75 && tip && sub.x == 1 && !top {
                    covered = false;
                }
            }
        }
    } else if part == PART_FLOAT {
        // A floating leaf (T6c part 2) lying on the water line, lit on top,
        // rounded where the row of leaves ends.
        // A tiger lotus type's floating pad joins its petiole's tip.
        let lotus = shapes.x == LEAF_LOTUS;
        let kl = kin(at, vec2<i32>(-1, 0), me).x;
        let kr = kin(at, vec2<i32>(1, 0), me).x;
        let left = kl == PART_FLOAT || (lotus && kl >= 0);
        let right = kr == PART_FLOAT || (lotus && kr >= 0);
        let end = select(!right, !left, sub.x == 0);
        let disc = shapes.x == LEAF_DISC || lotus;
        step += 2 + select(-1, 1, top) + select(0, 1, (hash(at) & 3u) == 0u);
        if lotus {
            // Green, mottled maroon.
            step = select(4, 6, top);
        }
        if disc {
            // A round pad over a dark, spongy underside.
            covered = !(top && end);
            step -= select(0, 2, !top);
        } else {
            // A small flat frond, its lower edge rounded at the ends.
            covered = top || !end;
        }
        if fine {
            let edge = (quarter.x == 0 && !left) || (quarter.x == 3 && !right);
            if disc {
                // A glossy pad: a lit rim, a pale vein, a dark spongy
                // underside, rounded at both ends.
                covered = !(edge && (quarter.y == 0 || quarter.y == 3));
                step = step + select(0, 1, quarter.y == 0) - select(0, 3, quarter.y == 3)
                    + select(0, 2, quarter.y == 1 && quarter.x == 1 && !lotus);
            } else {
                // A flat oval frond three quarters thick, tapered at its
                // ends, the water showing under it but for the root's
                // stalk.
                let stalk = quarter.y == 3 && (quarter.x == 1 || quarter.x == 2)
                    && kin(at, vec2<i32>(0, 1), me).x == PART_ROOT;
                covered = (quarter.y <= 2 && !(edge && quarter.y != 1)) || stalk;
                step += select(0, 1, quarter.y == 0) - select(0, 1, quarter.y == 2) - select(0, 2, stalk);
            }
        }
        if lotus && ((hash(at * 2 + sub) & 7u) < 2u || (hash(at) & 7u) == 0u) {
            step = 1 + i32(top) + i32(hash(at) >> 9u) % 2;
        }
    } else {
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
            if fine {
                // A slim needle rising a quarter-cell at a time from the
                // node to its tip, lit above, with a darker underside short of the tip.
                let along = (pos - 1) * 4 + select(3 - quarter.x, quarter.x, from_left);
                let span = 4 * max(i32(sizes.z), 1);
                let line = 3 - along * 4 / span;
                let under = quarter.y == line + 1 && along > 0 && along < span - 2;
                covered = quarter.y == line || under;
                step += select(0, 1, quarter.y == line) - select(0, 2, under);
            }
        } else if shape == LEAF_ROUND {
            // Broad to a blunt tip, with a darker rim at the tip's foot.
            covered = !(base && u == 0 && top);
            step -= select(0, 2, tip && u == 1 && !top);
            if fine {
                // A short petiole into an oval blade with blunt, rounded
                // ends, a pale midrib, a lit upper half and a darker rim.
                let x = select(3 - quarter.x, quarter.x, from_left);
                let ends = min((pos - 1) * 4 + x - 2, select(4, 3 - x, tip));
                let rib = quarter.y == 2;
                let rim = quarter.y == 3 || ends == 0;
                covered = select(rib, select(quarter.y == 1 || rib, true, ends > 0), ends >= 0);
                step = step + select(0, 1, rib) + select(0, 1, quarter.y == 0) - select(0, 2, rim);
            }
        } else if shape == LEAF_FEATHER {
            // A midrib along the bottom, fine teeth above every other texel.
            covered = !top || u == 1;
            step += select(0, 1, top);
            if fine {
                // Fishbone: a thin rachis with filaments rising from every
                // other quarter, short ones hanging between them, and a
                // forked tip; the filament ends are lightest.
                let along = (pos - 1) * 4 + select(3 - quarter.x, quarter.x, from_left);
                let rachis = quarter.y == 2;
                let rising = quarter.y < 2 && along % 2 == 1;
                let hanging = quarter.y == 3 && along % 2 == 0 && along > 1;
                covered = rachis || rising || hanging;
                step += select(0, 1, quarter.y == 0) - select(0, 1, hanging) - select(1, 0, rachis);
            }
        } else {
            // Lanceolate: a petiole, a broad blade and a tip that rises.
            covered = !(base && u == 0 && top) && !(tip && u == 1 && !top);
            if fine {
                // From petiole to tip, the pale midrib rises through a broad
                // blade. The darker underside ends one texel before the rib.
                let x = select(3 - quarter.x, quarter.x, from_left);
                let rib = select(select(2, 1, x >= 2), 2, base);
                let upper = select(select(0, 2, base && x < 2), 1, tip && x >= 2);
                let lower = select(select(3, 2, tip && x >= 1), 1, tip && x == 3);
                covered = quarter.y >= upper && quarter.y <= lower;
                step += select(-2, 2, quarter.y <= rib)
                    + select(0, 1, quarter.y == rib)
                    - select(0, 2, base && x < 2);
            }
        }
        // Night folding on stem leaves (T6d): leaves tuck inward toward the
        // stem and fold upward as darkness rises (view.daylight.w: 0 day, 1 night).
        let night = view.daylight.w;
        if night > 0.01 {
            if fine {
                let x = select(3 - quarter.x, quarter.x, from_left);
                let along = (pos - 1) * 4 + x;
                let span = max(i32(sizes.z), 1) * 4;
                if tip && !base {
                    let tuck = i32(round(night * 2.0));
                    if x >= 4 - tuck {
                        covered = false;
                    }
                } else if tip && base && night >= 0.75 && x == 3 {
                    covered = false;
                }
                if quarter.y == 3 && night >= 0.5 && along >= 2 {
                    covered = false;
                }
                if quarter.y == 2 && night >= 0.8 && along >= span / 2 {
                    covered = false;
                }
            } else {
                if tip && !base {
                    if (night >= 0.75 && u == 1) || (night >= 0.5 && u == 1 && !top) {
                        covered = false;
                    }
                } else if night >= 0.75 && u == 1 && !top {
                    covered = false;
                }
            }
        }
    }
    return vec2<i32>(select(0, 1, covered), step);
}

// Whether a stem's, runner's, rhizome's or root's texel is covered (x: 1
// or 0) and its ramp shade before clamping (y), as leaf_cover.
fn stem_cover(at: vec2<i32>, me: vec4<f32>, part: i32, quarter: vec2<i32>, plant: vec4<f32>) -> vec2<i32> {
    let sub = quarter / 2;
    let species = i32(round(plant.a * 255.0));
    let shapes = vec4<i32>(round(textureLoad(plant_species, vec2<i32>(SHAPES_COLUMN, species), 0) * 255.0));
    let sizes = round(textureLoad(plant_species, vec2<i32>(SIZES_COLUMN, species), 0) * 255.0);
    let pos = position_of(me);
    let fine = FINE_PLANTS && fine_scale();
    let top = sub.y == 0;
    var covered = true;
    var step = noise_step(me);
    if part == PART_RHIZOME {
        // A creeping epiphyte rhizome hugging rock or wood: thick, slightly
        // rounded along its run, darker underneath, with tiny gripping
        // rootlets at 4x.
        let up = kin(at, vec2<i32>(0, -1), me).x >= 0;
        let down = kin(at, vec2<i32>(0, 1), me).x == PART_RHIZOME;
        let left = kin(at, vec2<i32>(-1, 0), me).x == PART_RHIZOME;
        let right = kin(at, vec2<i32>(1, 0), me).x == PART_RHIZOME;
        let end_l = !left && !up && !down;
        let end_r = !right && !up && !down;
        covered = !(top && ((end_l && sub.x == 0) || (end_r && sub.x == 1)));
        step += 2 + select(-1, 1, top) + select(0, 1, up);
        if fine {
            let rootlet = !down && ((hash(at) & 1u) == 0u) && quarter.y == 3
                && (quarter.x == 1 || quarter.x == 2);
            let body = (quarter.y >= 1 && quarter.y <= 2)
                || (up && quarter.y == 0 && (quarter.x == 1 || quarter.x == 2))
                || (down && quarter.y == 3 && (quarter.x == 1 || quarter.x == 2));
            covered = body || rootlet;
            step += select(0, 1, quarter.y == 1) - select(0, 2, rootlet);
        }
    } else if part == PART_RUNNER {
        // A stolon along the top of its cell, joining runner cells above or
        // below where it climbs or drops, lighter where it pokes out.
        let up = kin(at, vec2<i32>(0, -1), me).x == PART_RUNNER;
        let down = kin(at, vec2<i32>(0, 1), me).x == PART_RUNNER;
        covered = top || (sub.x == 0 && (up || down));
        step += 2 + select(0, 1, is_water(at + vec2<i32>(0, -1)));
        if fine {
            // A rounded stolon, lit along its top, dropping a thin link to
            // the runner below; every few cells a node puts out a root hair.
            let hair = !down && (hash(at) & 3u) == 0u && quarter.x == 2 && quarter.y == 2;
            covered = quarter.y <= 1 || (quarter.x == 1 && down) || hair;
            step += select(0, 1, quarter.y == 0) - select(0, 2, hair);
        }
    } else if part == PART_STEM {
        let shape = shapes.y;
        if shape == STEM_FUZZ || shape == STEM_FROND {
            // A mat: texels facing water fray, and growing tips are lighter.
            let d = sub * 2 - 1;
            let water_x = is_water(at + vec2<i32>(d.x, 0));
            let water_y = is_water(at + vec2<i32>(0, d.y));
            let exposed = water_x || water_y;
            var h = hash(at * 2 + sub);
            // How far into the cell a quarter lies from the water it faces
            // (0 outermost, 1 next; 2 when not exposed).
            let depth_x = select(2, select(3 - quarter.x, quarter.x, d.x < 0), water_x);
            let depth_y = select(2, select(3 - quarter.y, quarter.y, d.y < 0), water_y);
            let depth = min(depth_x, depth_y);
            if fine {
                h = hash(at * 4 + quarter);
            }
            if shape == STEM_FUZZ {
                covered = !exposed || (h & 7u) < 5u;
                step = 1 + i32((h >> 3u) % 4u) + select(0, 3, exposed);
                if fine {
                    // Tangled strands thin out towards the water: sparse
                    // lit wisps on the outer quarter, denser just inside.
                    let keep = select(select(8u, 6u, depth == 1), 4u, depth == 0);
                    covered = (h & 7u) < keep;
                    step = 2 + i32((h >> 3u) % 4u) + select(0, 3, depth < 2) + select(0, 1, depth == 0);
                }
            } else {
                // Layered fronds: exposed edges break into regular tiers (a
                // gap every third texel along the edge), each tier's first
                // texel the lightest, over a dark inner row, where java moss's
                // fuzz frays at random.
                let t = at * 2 + sub;
                let k = select(t.x, t.y, water_x && !water_y) % 3;
                covered = !exposed || k != 2;
                step = i32((h >> 3u) & 1u) + select(0, 3, exposed) + select(0, 3, exposed && k == 0);
                if fine {
                    // Tiered fronds: along the edge each four-quarter tier
                    // tapers to a pale tip on the outer quarter, with a gap
                    // between tiers; inside, side branches angle off a
                    // darker axis.
                    let f = at * 4 + quarter;
                    let m = select(f.x, f.y, water_x && !water_y) % 4;
                    covered = select(select(true, m != 3, depth == 1), m == 1, depth == 0);
                    let branch = (f.x + f.y) % 4 == 0;
                    step = i32((h >> 3u) & 1u) + select(0, 3, depth < 2) + select(0, 3, depth == 0)
                        + select(0, 2, depth == 2 && branch) + select(0, 2, depth == 1 && m == 0);
                }
            }
        } else if shape == STEM_VELVET {
            // A ball (marimo): a dense velvet pile shaded as a sphere lit
            // from above and a little to the left, by how far the ball
            // runs on each way from the cell, its rim softly fuzzed and its
            // corners rounded where it meets the water.
            let du = ball_run(at, vec2<i32>(0, -1), me);
            let dd = ball_run(at, vec2<i32>(0, 1), me);
            let dl = ball_run(at, vec2<i32>(-1, 0), me);
            let dr = ball_run(at, vec2<i32>(1, 0), me);
            let open_u = du == 0;
            let open_d = dd == 0;
            let open_l = dl == 0;
            let open_r = dr == 0;
            let rim = (top && open_u) || (!top && open_d) || (sub.x == 0 && open_l)
                || (sub.x == 1 && open_r);
            var h = hash(at * 2 + sub);
            let lit = 3.6 + f32(dd - du) * 0.75 + f32(dr - dl) * 0.3
                - select(0.0, 0.6, min(dl, dr) == 0);
            step = i32(round(lit + select(-0.3, 0.3, top) + select(-0.15, 0.15, sub.x == 0)))
                + i32((h >> 3u) & 1u) - 1 + noise_step(me);
            // Corners open on both sides are rounded off.
            let corner = select(open_d, open_u, top) && select(open_r, open_l, sub.x == 0);
            covered = (!rim || (h & 3u) != 0u) && !corner;
            if fine {
                // Fine pile: the outer quarter facing the water is a
                // sparse fringe of lit tufts; the corners round off.
                h = hash(at * 4 + quarter);
                let fringe = (quarter.y == 0 && open_u) || (quarter.y == 3 && open_d)
                    || (quarter.x == 0 && open_l) || (quarter.x == 3 && open_r);
                let cx = select(select(-1, 3, open_r), 0, open_l);
                let cy = select(select(-1, 3, open_d), 0, open_u);
                let near = abs(quarter.x - cx) + abs(quarter.y - cy) <= 1 && cx >= 0 && cy >= 0;
                covered = (!fringe || (h & 3u) == 0u) && !near;
                let q = vec2<f32>(quarter) - 1.5;
                step = i32(round(lit - q.y * 0.25 - q.x * 0.1)) + i32((h >> 3u) & 1u) - 1
                    + noise_step(me) + select(0, 1, fringe);
            }
        } else {
            let left = kin(at, vec2<i32>(-1, 0), me);
            let right = kin(at, vec2<i32>(1, 0), me);
            let up_stem = kin(at, vec2<i32>(0, -1), me).x == PART_STEM;
            let down_stem = kin(at, vec2<i32>(0, 1), me).x == PART_STEM;
            let horiz = (left.x == PART_STEM || right.x == PART_STEM) && !up_stem && !down_stem;
            let node = left.x == PART_LEAF || right.x == PART_LEAF;
            let tip = !up_stem && (!horiz || (left.x == PART_STEM && right.x != PART_STEM) || (right.x == PART_STEM && left.x != PART_STEM));
            step += 1 + i32(round(f32(pos) / max(sizes.x, 1.0) * 5.0)) - select(0, 1, node) + select(0, 2, tip);
            if shape == STEM_THICK {
                if horiz {
                    if fine {
                        let shoulder = tip && (quarter.y == 0 || quarter.y == 3)
                            && ((left.x == PART_STEM && quarter.x == 3)
                                || (right.x == PART_STEM && quarter.x == 0));
                        covered = !shoulder;
                        step += select(0, 1, quarter.y == 1) - select(0, 1, quarter.y == 3);
                    } else {
                        step -= select(0, 1, !top);
                    }
                } else if fine {
                    // A round stem, brightest left of centre and darkest on
                    // the right, a dark joint line at each node and a
                    // rounded growing tip.
                    let shoulder = tip && quarter.y == 0 && (quarter.x == 0 || quarter.x == 3);
                    covered = !shoulder;
                    step += select(0, 1, quarter.x == 1) - select(0, 1, quarter.x == 3)
                        - select(0, 1, node && quarter.y == 3);
                } else {
                    step -= select(0, 1, sub.x == 1);
                }
            } else if horiz {
                // Horizontal branch arm or surface-trailing stem.
                covered = top;
                if fine {
                    covered = quarter.y == 1 || quarter.y == 2;
                    step += select(-1, 2, quarter.y == 1);
                }
            } else {
                // Stalk: one texel wide, full at nodes and the growing tip.
                covered = node || tip || sub.x == 0;
                if fine {
                    // A lit stalk with a flared collar joining each leaf pair.
                    let collar = node && quarter.y == 2;
                    covered = quarter.x == 1 || quarter.x == 2 || collar
                        || (tip && quarter.y >= 2 && quarter.x == 0);
                    step += select(-1, 2, quarter.x == 1) + select(0, 1, collar);
                }
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
            if fine {
                // Each root meets its neighbours at their shared edge centre.
                // The junction wanders a quarter-cell, leaving fine root hairs.
                let joint = vec2<i32>(1 + col, 1 + row);
                let dx = abs(quarter.x - joint.x);
                let dy = abs(quarter.y - joint.y);
                let edge_x = select(joint.x, 1, quarter.y == 0 || quarter.y == 3);
                let edge_y = select(joint.y, 2, quarter.x == 0 || quarter.x == 3);
                covered = all(quarter == joint)
                    || (quarter.x == edge_x && select(down, up, quarter.y < joint.y))
                    || (quarter.y == edge_y && select(right, left, quarter.x < joint.x))
                    || (dx == 1 && dy == 1 && quarter.x == 0 && row == 1);
                step += select(-1, 1, dx == 0);
            }
        } else if fine {
            // Thick roots: rounded where they end or bend, fibrous along
            // their free edges, with a pale core along their run.
            let up = kin(at, vec2<i32>(0, -1), me).x >= 0;
            let down = kin(at, vec2<i32>(0, 1), me).x >= 0;
            let left = kin(at, vec2<i32>(-1, 0), me).x >= 0;
            let right = kin(at, vec2<i32>(1, 0), me).x >= 0;
            let free_y = (quarter.y == 0 && !up) || (quarter.y == 3 && !down);
            let free_x = (quarter.x == 0 && !left) || (quarter.x == 3 && !right);
            let fibre = (hash(at * 4 + quarter) & 3u) != 0u;
            covered = !(free_x && free_y) && (!(free_x || free_y) || fibre);
            let core = select(quarter.y == 1, quarter.x == 1, up || down);
            step += select(0, 1, core) - select(0, 1, free_x || free_y);
        }
    }
    return vec2<i32>(select(0, 1, covered), step);
}

// A plant texel's colour from its coverage and shade (`cover`, see
// leaf_cover), or alpha 0 where uncovered.
fn plant_colour(plant: vec4<f32>, part: i32, cover: vec2<i32>, band: f32) -> vec4<f32> {
    let species = i32(round(plant.a * 255.0));
    let covered = cover.x != 0;
    let step = cover.y;
    // Runners and rhizomes are drawn from the stem's ramp, floating leaves
    // from the leaf's.
    let ramp = select(
        select(part, PART_STEM, part == PART_RUNNER || part == PART_RHIZOME),
        PART_LEAF,
        part == PART_FLOAT,
    );
    let health = plant.r;
    if covered || view.scale < 2.0 {
        let shade = clamp(step, 0, RAMP - 1);
        let base = textureLoad(plant_species, vec2<i32>(ramp * RAMP + shade, species), 0).rgb;
        var rgb = depth_cue(health_tint(coloured(base, shade, ramp, plant.g, species), health), band);
        // Subtle night dimming on folded leaves (T6d).
        if view.daylight.w > 0.01 && (part == PART_LEAF || part == PART_FLOAT) {
            let dim = select(
                mix(1.0, 0.84, view.daylight.w),
                mix(1.0, 0.90, view.daylight.w),
                part == PART_FLOAT,
            );
            rgb = rgb * dim;
        }
        let opacity = select(mix(MIN_OPACITY, 1.0, health / FADE_AT), 1.0, health >= FADE_AT);
        return vec4<f32>(rgb, opacity);
    }
    return vec4<f32>(0.0, 0.0, 0.0, 0.0);
}

// What shows through a plant cell's empty or translucent texel where the
// plant fills all three neighbours: the ramp's darkest shade, darkened.
fn plant_shadow_of(me: vec4<f32>, part: i32, band: f32) -> vec3<f32> {
    let plant = textureLoad(plant_tints, vec2<i32>(round(me.ba * 255.0)), 0);
    let species = i32(round(plant.a * 255.0));
    let ramp = select(
        select(part, PART_STEM, part == PART_RUNNER || part == PART_RHIZOME),
        PART_LEAF,
        part == PART_FLOAT,
    );
    let rgb = textureLoad(plant_species, vec2<i32>(ramp * RAMP, species), 0).rgb;
    return depth_cue(health_tint(coloured(rgb, 0, ramp, plant.g, species), plant.r) * 0.6, band);
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.299, 0.587, 0.114));
}

// `rgb` (ramp `ramp`'s shade `shade`) shifted towards the species'
// colouration colour by `amount` (the plant's colouration, 0-1): the
// lighter, newer shades most, stems half as much, roots not at all. The
// colour keeps the shade's lightness.
fn coloured(rgb: vec3<f32>, shade: i32, ramp: i32, amount: f32, species: i32) -> vec3<f32> {
    if amount <= 0.0 || ramp == PART_ROOT {
        return rgb;
    }
    let c = textureLoad(plant_species, vec2<i32>(COLOUR_COLUMN, species), 0);
    if c.a < 0.5 {
        return rgb;
    }
    let shifted = min(c.rgb * (luma(rgb) / max(luma(c.rgb), 0.05)), vec3<f32>(1.0));
    let lift = mix(COLOUR_DARK, 1.0, f32(shade) / f32(RAMP - 1));
    // Complementary greens and reds mix to a dull tan halfway, so the
    // shift runs ahead of the colouration and saturates.
    let k = amount * lift * select(1.0, COLOUR_STEM, ramp == PART_STEM) * COLOUR_GAIN;
    return mix(rgb, shifted, min(k, 1.0));
}

// `rgb` tinted for a plant of `health` (0-1): untinted while vigorous,
// paling to yellow as health falls to YELLOW_AT, then browning as it dies.
fn health_tint(rgb: vec3<f32>, health: f32) -> vec3<f32> {
    if health >= VIGOROUS {
        return rgb;
    }
    let pale = rgb * mix(vec3<f32>(1.0), YELLOW, clamp((VIGOROUS - health) / (VIGOROUS - YELLOW_AT), 0.0, 1.0));
    if health >= YELLOW_AT {
        return pale;
    }
    let brown = DEAD_BROWN * (0.6 + 0.9 * luma(rgb));
    return mix(pale, brown, (YELLOW_AT - health) / YELLOW_AT);
}

// An sRGB-encoded colour as the target expects it. Shading happens in
// encoded space on every target, so captures, windows and the web agree.
fn out(c: vec3<f32>) -> vec4<f32> {
    let linear = select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
    return vec4<f32>(select(c, linear, view.srgb_target > 0.5), 1.0);
}

struct SpriteOut {
    @builtin(position) pos: vec4<f32>,
    // Texel coordinate within the frame, interpolated across the quad
    // (outside 0..frame size in a glow halo).
    @location(0) local: vec2<f32>,
    // 0 alive, 1 elder, 2 dead; how far a corpse has rotted (0..1).
    @location(1) @interpolate(flat) look: vec2<f32>,
    // Depth in bands from the front glass (D21).
    @location(2) @interpolate(flat) depth: f32,
    // The sampled frame: its top-left atlas texel (xy) and size in texels
    // (zw).
    @location(3) @interpolate(flat) frame: vec4<f32>,
    // Glow colour (rgb, sRGB-encoded) and strength times the darkness (w;
    // 0 for none).
    @location(4) @interpolate(flat) glow: vec4<f32>,
};

// Glow halo radius past the sprite's opaque texels, in texels.
const HALO: f32 = 3.0;
// Atlas alphas marking a glowing species' eye whites and unlit texels
// (sprites.rs ALPHA_EYE and ALPHA_UNLIT; renderer.rs checks them).
const ALPHA_EYE: f32 = 254.0;
const ALPHA_UNLIT: f32 = 253.0;

// One creature's quad: two triangles over its sprite, the top-left snapped
// to a target pixel and each texel half a cell wide (a quarter from the fine
// frames).
@vertex
fn vs_sprite(
    @builtin(vertex_index) i: u32,
    // Top-left in cells (fractional), on-screen size in cells (the frame's
    // size, swapped when turned a quarter), the frame's top-left texel in
    // the atlas, and orient: x > 0.5 to mirror (facing left in its own
    // frame), y the quarter turns clockwise applied after the mirror; the
    // frame's size in texels (a growing creature's body is drawn smaller
    // than it), the look (see SpriteOut) and the fine frame's top-left texel
    // (the same pose at twice the size; negative for a body drawn smaller
    // than its frame, which keeps the coarser art); its depth in bands; its
    // night glow colour and strength (0 for none).
    @location(0) cell: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) origin: vec2<f32>,
    @location(3) orient: vec2<f32>,
    @location(4) frame_size: vec2<f32>,
    @location(5) look: vec2<f32>,
    @location(6) fine_origin: vec2<f32>,
    @location(7) depth: f32,
    @location(8) glow: vec4<f32>,
) -> SpriteOut {
    // Corners (0,0) (1,0) (0,1) | (0,1) (1,0) (1,1).
    let corner = vec2<f32>(
        select(0.0, 1.0, i == 1u || i == 4u || i == 5u),
        select(0.0, 1.0, i == 2u || i == 3u || i == 5u),
    );
    let fine = fine_origin.x >= 0.0 && fine_scale();
    let frame = frame_size * select(1.0, 2.0, fine);
    let turn = u32(round(orient.y)) % 4u;
    // A glow's halo, at night only: the quad grows by HALO texels a side
    // (in cells: HALO times the cells per texel along each screen axis).
    let k = glow.w * view.daylight.w;
    let halo = select(0.0, HALO, k > 0.0);
    let margin = halo * size / select(frame, frame.yx, turn % 2u == 1u);
    let top_left = round(view.offset + cell * view.scale) - margin * view.scale;
    let px = top_left + corner * (size + 2.0 * margin) * view.scale;
    let ndc = vec2<f32>(px.x / view.target_size.x * 2.0 - 1.0, 1.0 - px.y / view.target_size.y * 2.0);
    // Undo the turn: the frame point (0..1) this screen corner shows. One
    // clockwise turn takes frame (a, b) to screen (1 - b, a).
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
    var o: SpriteOut;
    o.pos = vec4<f32>(ndc, 0.0, 1.0);
    o.local = q * (frame + 2.0 * halo) - halo;
    o.frame = vec4<f32>(select(origin, fine_origin, fine), frame);
    o.look = look;
    o.depth = depth;
    o.glow = vec4<f32>(glow.rgb, k);
    return o;
}

// Whether a plant cell in a band nearer the glass than `depth` covers the
// pixel at `pos` (D21): the grid already drew the plant there, so a sprite
// fragment behind it is dropped. Texels the plant's shape leaves open, and
// plant cells level with or behind the creature, don't hide it.
fn hidden_by_plant(pos: vec2<f32>, depth: f32) -> bool {
    let src = plant_source(pos);
    if src.part < 0 {
        return false;
    }
    let band = band_at(src.at);
    return band + 0.5 < depth && plant_texel(src.at, src.texel, src.part, src.quarter, band).a > 0.5;
}

// hidden_by_plant for the split sprite passes, which reach only their own
// part's shape code: fs_sprite_leaves draws the pixels that show a leafy
// part or no plant, fs_sprite_stems those that show another part. The split
// is by pixel, not by sprite, so every sprite at a pixel draws in the same
// pass and their back-to-front order holds.
fn hidden_by_leaf(pos: vec2<f32>, depth: f32) -> bool {
    let src = plant_source(pos);
    if src.part < 0 {
        return false;
    }
    if !is_leafy(src.part) {
        discard;
    }
    let band = band_at(src.at);
    return band + 0.5 < depth && leaf_texel(src.at, src.texel, src.part, src.quarter, band).a > 0.5;
}

fn hidden_by_stem(pos: vec2<f32>, depth: f32) -> bool {
    let src = plant_source(pos);
    if src.part < 0 || is_leafy(src.part) {
        discard;
    }
    let band = band_at(src.at);
    return band + 0.5 < depth && stem_texel(src.at, src.texel, src.part, src.quarter, band).a > 0.5;
}

// Whether loose substrate (sand, gravel, aquasoil) covers the pixel at
// `pos`: a burrower in the bed shows only where the ground drawn there is
// open, so the cell's trimmed top (`loose_top`) shows it too.
fn hidden_by_substrate(pos: vec2<f32>) -> bool {
    let grid_pos = (pos - view.offset) / view.scale;
    let cell = floor(grid_pos);
    if any(cell < vec2<f32>(0.0)) || any(cell >= view.grid) {
        return false;
    }
    let at = vec2<i32>(cell);
    let t = textureLoad(cells, at, 0);
    let style = style_of(t);
    if style != STYLE_SAND && style != STYLE_GRAVEL && style != STYLE_AQUASOIL {
        return false;
    }
    let mat = i32(round(t.r * 255.0));
    let quarter = vec2<i32>(floor(fract(grid_pos) * 4.0));
    let fine = fine_scale();
    let open_up = is_open_fluid(at - vec2<i32>(0, 1), mat);
    let trim = loose_top_at(at, mat, open_up, quarter, fine);
    return select(quarter.y / 2, quarter.y, fine) >= trim;
}

// The atlas texel at `p` within `frame` (transparent outside it).
fn frame_texel(frame: vec4<f32>, p: vec2<f32>) -> vec4<f32> {
    if any(p < vec2<f32>(0.0)) || any(p >= frame.zw) {
        return vec4<f32>(0.0);
    }
    return textureLoad(atlas, vec2<i32>(frame.xy + p), 0);
}

// The glow halo over transparent texel `p`: alpha by the rounded distance
// to the nearest opaque texel within HALO (1, 2, 3: 0.42, 0.24, 0.11,
// the faintest ring checker-dithered), times the glow; 0 past it.
fn halo_alpha(frame: vec4<f32>, p: vec2<f32>, k: f32) -> f32 {
    var best = 99;
    for (var dy = -3; dy <= 3; dy++) {
        for (var dx = -3; dx <= 3; dx++) {
            let d2 = dx * dx + dy * dy;
            if d2 < best && d2 <= 9 && frame_texel(frame, p + vec2<f32>(f32(dx), f32(dy))).a > 0.5 {
                best = d2;
            }
        }
    }
    if best > 9 {
        return 0.0;
    }
    let ring = round(sqrt(f32(best)));
    var a = select(select(0.11, 0.24, ring < 2.5), 0.42, ring < 1.5);
    let cell = vec2<i32>(p);
    if ring > 2.5 && ((cell.x + cell.y) & 1) == 0 {
        a = a * 0.4;
    }
    return min(1.0, a * k * 2.2);
}

// How strongly opaque texel `p` (atlas texel `t`) glows: none for an unlit
// texel (a pupil), 0.45 for an eye white, 0.4 on the outline (a 4-neighbour
// is transparent) and 0.07 for the rest of the body.
fn glow_weight(frame: vec4<f32>, p: vec2<f32>, t: vec4<f32>) -> f32 {
    let a = round(t.a * 255.0);
    if a == ALPHA_UNLIT {
        return 0.0;
    }
    if a == ALPHA_EYE {
        return 0.45;
    }
    let edge = frame_texel(frame, p + vec2<f32>(1.0, 0.0)).a < 0.5
        || frame_texel(frame, p - vec2<f32>(1.0, 0.0)).a < 0.5
        || frame_texel(frame, p + vec2<f32>(0.0, 1.0)).a < 0.5
        || frame_texel(frame, p - vec2<f32>(0.0, 1.0)).a < 0.5;
    return select(0.07, 0.4, edge);
}

@fragment
fn fs_sprite(v: SpriteOut) -> @location(0) vec4<f32> {
    if sprite_skipped(v) {
        discard;
    }
    return sprite_colour(v, hidden_by_plant(v.pos.xy, v.depth));
}

@fragment
fn fs_sprite_leaves(v: SpriteOut) -> @location(0) vec4<f32> {
    if sprite_skipped(v) {
        discard;
    }
    return sprite_colour(v, hidden_by_leaf(v.pos.xy, v.depth));
}

@fragment
fn fs_sprite_stems(v: SpriteOut) -> @location(0) vec4<f32> {
    if sprite_skipped(v) {
        discard;
    }
    return sprite_colour(v, hidden_by_stem(v.pos.xy, v.depth));
}

// Sprite fragments drawn without looking at plants: under the palette
// (it stays on top; creatures show beside its shorter row), and open
// texels without a glow's halo.
fn sprite_skipped(v: SpriteOut) -> bool {
    if outside_lens(v.pos.xy) {
        return true;
    }
    if view.hud.w > 0.5 {
        let p = floor((v.pos.xy - view.hud.xy) / view.hud.z);
        if all(p >= vec2<f32>(0.0)) && all(p < view.hud_size.xy)
            && textureLoad(hud, vec2<i32>(p), 0).a > 0.5 {
            return true;
        }
    }
    return frame_texel(v.frame, floor(v.local)).a < 0.5 && v.glow.w <= 0.0;
}

// A sprite fragment's colour that sprite_skipped kept, `hidden` if a nearer
// plant covers it.
fn sprite_colour(v: SpriteOut, hidden: bool) -> vec4<f32> {
    let p = floor(v.local);
    let texel = vec2<i32>(v.frame.xy + p);
    let t = frame_texel(v.frame, p);
    let k = v.glow.w;
    if t.a < 0.5 {
        // Only a glowing creature at night draws past its texels: the halo.
        if k <= 0.0 || hidden {
            discard;
        }
        let a = halo_alpha(v.frame, p, k);
        if a <= 0.0 {
            discard;
        }
        return vec4<f32>(out(v.glow.rgb).rgb, a);
    }
    if hidden || (v.look.x < 1.5 && v.look.y > 0.5 && hidden_by_substrate(v.pos.xy)) {
        discard;
    }
    var rgb = t.rgb;
    let grey = vec3<f32>(dot(rgb, vec3<f32>(0.299, 0.587, 0.114)));
    if v.look.x > 1.5 {
        // A corpse: pale and brownish, losing texels as it rots.
        if f32(hash(texel) & 1023u) < v.look.y * 1024.0 {
            discard;
        }
        rgb = mix(rgb, grey * vec3<f32>(0.85, 0.8, 0.7), 0.7);
    } else if v.look.x > 0.5 {
        // An elder: a little greyer.
        rgb = mix(rgb, grey, 0.3);
    }
    var lit = depth_cue(rgb, v.depth) * view.daylight.rgb;
    if k > 0.0 {
        // Emissive, after the tint: greener than the glow colour itself.
        let e = glow_weight(v.frame, p, t) * k * 1.6;
        lit = min(lit + v.glow.rgb * e * vec3<f32>(0.55, 1.0, 0.55), vec3<f32>(1.0));
    }
    return out(lit);
}

// Pixel-art magnifying glass overlay: circular metallic bezel, subtle glass
// glint, 4-tick centre reticle, and a 45-degree diagonal handle extending from
// the bottom-right of the rim. Drawn after the circular magnified grid and
// sprites, staying behind the palette strip when the two overlap.
@fragment
fn fs_lens_frame(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    if hud_at(pos.xy).a > 0.5 {
        discard;
    }
    let step = max(view.lens.w, 1.0);
    let p = floor((pos.xy - view.lens.xy) / step) + 0.5;
    let r = view.lens.z / step;
    let dist = length(p);

    // Inside the circular glass: subtle edge tint, upper-left curved glass
    // glint, and a 4-tick centre reticle leaving the centred cell clear.
    if dist < r {
        let rel = floor(pos.xy) - view.lens.xy;
        let dc = abs(rel);
        let gap = max(3.0, floor(view.scale * 0.5) + 2.0);
        let tick = gap + 4.0;
        let on_x = dc.y < 0.5 && dc.x >= gap && dc.x < tick;
        let on_y = dc.x < 0.5 && dc.y >= gap && dc.y < tick;
        if on_x || on_y {
            return vec4<f32>(out(vec3<f32>(0.957, 0.914, 0.784)).rgb, 0.52);
        }
        // Upper-left curved glass glint (primary crescent + small specular dot).
        let glint_band = dist >= r - 4.8 && dist <= r - 2.6;
        let main_arc = glint_band && p.x < -0.18 * r && p.y < -0.18 * r && abs(p.x - p.y) < r * 0.48;
        let spec_dot = dist >= r - 4.6 && dist <= r - 3.0 && p.x < -0.58 * r && p.y > -0.22 * r && p.y < -0.06 * r;
        if main_arc || spec_dot {
            return vec4<f32>(out(vec3<f32>(0.92, 0.97, 1.0)).rgb, select(0.32, 0.42, spec_dot));
        }
        // Subtle cool inner rim sheen right inside the bezel.
        if dist >= r - 1.0 {
            return vec4<f32>(out(vec3<f32>(0.76, 0.91, 1.0)).rgb, 0.16);
        }
        discard;
    }

    // Circular metallic bezel (4 art pixels thick: dark inner lip, 2-px
    // top-left lit metallic ring, dark outer outline).
    if dist <= r + 4.0 {
        let light = -(p.x + p.y) / max(dist * 1.4142135, 1.0);
        var rgb = vec3<f32>(0.043, 0.071, 0.110); // dark outline (#0b121c)
        if dist < r + 1.0 {
            // Inner recessed lip: darker bottom-right, subtle highlight top-left.
            rgb = select(vec3<f32>(0.075, 0.125, 0.188), vec3<f32>(0.24, 0.34, 0.45), light > 0.35);
        } else if dist < r + 3.0 {
            // Brushed silver-steel bezel with warm ivory top-left highlight.
            if light > 0.38 {
                rgb = vec3<f32>(0.957, 0.914, 0.784);
            } else if light > -0.15 {
                rgb = vec3<f32>(0.580, 0.678, 0.765);
            } else {
                rgb = vec3<f32>(0.235, 0.310, 0.404);
            }
        }
        return vec4<f32>(out(rgb).rgb, 1.0);
    }

    // 45-degree diagonal handle extending down-right from the rim: metallic
    // ferrule collar + shaded walnut grip + metallic pommel ring.
    let along = (p.x + p.y) * 0.70710678;
    let across = (p.x - p.y) * 0.70710678;
    let handle_len = clamp(r * 0.72, 14.0, 44.0);
    let collar_end = r + 7.5;
    let handle_end = r + 3.5 + handle_len;
    let cap_centre = handle_end - 3.6;
    if along >= r + 2.5 && along <= handle_end + 1.0 {
        let in_collar = along < collar_end;
        let half_w = select(3.6, 2.6, in_collar);
        let a_abs = abs(across);
        let in_body = select(
            a_abs <= half_w,
            length(vec2<f32>(along - cap_centre, across)) <= half_w,
            along > cap_centre,
        );
        if in_body {
            let on_edge = select(
                a_abs > half_w - 1.05,
                length(vec2<f32>(along - cap_centre, across)) > half_w - 1.05,
                along > cap_centre,
            );
            var rgb = vec3<f32>(0.043, 0.071, 0.110); // dark outline
            if !on_edge {
                let pommel = along > cap_centre - 1.8 && along < cap_centre + 0.4;
                if in_collar || pommel {
                    // Metallic collar / pommel band matching the bezel.
                    if across > 0.6 {
                        rgb = vec3<f32>(0.92, 0.88, 0.76);
                    } else if across > -0.8 {
                        rgb = vec3<f32>(0.56, 0.66, 0.75);
                    } else {
                        rgb = vec3<f32>(0.24, 0.31, 0.40);
                    }
                } else {
                    // Warm dark walnut grip, lit along upper-right edge.
                    if across > 0.9 {
                        rgb = vec3<f32>(0.58, 0.38, 0.25);
                    } else if across > -1.0 {
                        rgb = vec3<f32>(0.38, 0.23, 0.15);
                    } else {
                        rgb = vec3<f32>(0.22, 0.13, 0.08);
                    }
                }
            }
            return vec4<f32>(out(rgb).rgb, 1.0);
        }
        // Soft drop shadow along the lower-left underside of the handle.
        let in_shadow = across < -half_w && across >= -(half_w + 1.6) && along < handle_end;
        if in_shadow {
            return vec4<f32>(out(vec3<f32>(0.02, 0.04, 0.07)).rgb, 0.42);
        }
    }

    // Soft drop shadow hugging the bottom-right outer rim of the bezel.
    if dist <= r + 5.6 && (p.x + p.y) > 0.0 {
        return vec4<f32>(out(vec3<f32>(0.02, 0.04, 0.07)).rgb, 0.42);
    }

    discard;
    // HLSL FXC requires a return statement on all control paths even after discard.
    return vec4<f32>(0.0);
}
