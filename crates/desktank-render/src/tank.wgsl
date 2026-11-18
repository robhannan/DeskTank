// The cell grid drawn as pixel art: each fragment finds its cell (integer
// scale, nearest by construction), looks up (material, variant) in the
// palette, darkens water with depth and tints by the time of day. The interact overlay (palette
// and brush outline) is drawn on top. Creature sprites are instanced quads
// drawn after the grid (vs_sprite / fs_sprite), one atlas texel per cell.
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
};

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var cells: texture_2d<f32>;
@group(0) @binding(2) var palette: texture_2d<f32>;
@group(0) @binding(3) var hud: texture_2d<f32>;
// Nutrient, CO2 and light per 8x8-cell field cell, for the overlay.
@group(0) @binding(4) var fields: texture_2d<f32>;
// Creature sprite frames (sRGB-encoded RGBA; alpha 0 is transparent).
@group(0) @binding(5) var atlas: texture_2d<f32>;

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
    let mv = textureLoad(cells, vec2<i32>(cell), 0).rg;
    let idx = vec2<i32>(round(mv * 255.0));
    let c = textureLoad(palette, idx, 0);
    let depth = clamp((cell.y - view.surface_row) / max(view.grid.y - view.surface_row, 1.0), 0.0, 1.0);
    let light = 1.0 - view.depth_dim * depth * c.a;
    var rgb = c.rgb * light * view.daylight.rgb;
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

// An sRGB-encoded colour as the target expects it. Shading happens in
// encoded space on every target, so captures, windows and the web agree.
fn out(c: vec3<f32>) -> vec4<f32> {
    let linear = select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
    return vec4<f32>(select(c, linear, view.srgb_target > 0.5), 1.0);
}

struct SpriteOut {
    @builtin(position) pos: vec4<f32>,
    // Atlas texel coordinate, interpolated across the quad.
    @location(0) texel: vec2<f32>,
};

// One creature's quad: two triangles over its sprite, the top-left snapped
// to a target pixel and each texel one cell wide.
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
    let frame = select(size, size.yx, turn % 2u == 1u);
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
