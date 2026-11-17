// The cell grid drawn as pixel art: each fragment finds its cell (integer
// scale, nearest by construction), looks up (material, variant) in the
// palette and darkens water with depth. WebGL2-safe: textureLoad on 8-bit
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
};

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var cells: texture_2d<f32>;
@group(0) @binding(2) var palette: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    // One triangle covering the target.
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let cell = floor((pos.xy - view.offset) / view.scale);
    if any(cell < vec2<f32>(0.0)) || any(cell >= view.grid) {
        return out(view.backdrop.rgb);
    }
    let mv = textureLoad(cells, vec2<i32>(cell), 0).rg;
    let idx = vec2<i32>(round(mv * 255.0));
    let c = textureLoad(palette, idx, 0);
    let depth = clamp((cell.y - view.surface_row) / max(view.grid.y - view.surface_row, 1.0), 0.0, 1.0);
    let light = 1.0 - view.depth_dim * depth * c.a;
    return out(c.rgb * light);
}

// An sRGB-encoded colour as the target expects it. Shading happens in
// encoded space on every target, so captures, windows and the web agree.
fn out(c: vec3<f32>) -> vec4<f32> {
    let linear = select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
    return vec4<f32>(select(c, linear, view.srgb_target > 0.5), 1.0);
}
