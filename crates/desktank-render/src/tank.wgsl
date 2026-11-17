// The cell grid drawn as pixel art: each fragment finds its cell (integer
// scale, nearest by construction), looks up (material, variant) in the
// palette and darkens water with depth. The interact overlay (palette strip
// and brush outline) is drawn on top. WebGL2-safe: textureLoad on 8-bit
// unorm textures, one uniform buffer, no storage buffers.

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
};

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var cells: texture_2d<f32>;
@group(0) @binding(2) var palette: texture_2d<f32>;
@group(0) @binding(3) var hud: texture_2d<f32>;
// Nutrient, CO2 and light per 8x8-cell field cell, for the overlay.
@group(0) @binding(4) var fields: texture_2d<f32>;

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
    var rgb = c.rgb * light;
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
