// The camera's post pass (D29): resamples a view rectangle of the tank,
// drawn offscreen at its integer layout, over the whole target with a
// sharp-bilinear filter. Each source pixel stays a square block, with
// about one output pixel of blend at its edges, so a fractional zoom
// doesn't shimmer. At one output pixel per source pixel on whole-pixel
// offsets the filter picks the source pixel exactly.
// WebGL2-safe: textureLoad on one 8-bit texture, one uniform buffer.

struct Post {
    // Source pixel under the target's top-left corner (xy) and source
    // pixels per output pixel (zw).
    src: vec4<f32>,
    // Output pixels per source pixel (xy) and the source size in pixels (zw).
    k: vec4<f32>,
    // What shows outside the source, in the target's encoding (premultiplied).
    backdrop: vec4<f32>,
};

@group(0) @binding(0) var<uniform> post: Post;
@group(0) @binding(1) var source: texture_2d<f32>;

@vertex
fn vs_post(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    // One triangle covering the target.
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

fn tap(p: vec2<i32>) -> vec4<f32> {
    if any(p < vec2<i32>(0)) || any(p >= vec2<i32>(post.k.zw)) {
        return post.backdrop;
    }
    return textureLoad(source, p, 0);
}

@fragment
fn fs_post(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    // The source position of this pixel's centre, in source pixels.
    let s = post.src.xy + pos.xy * post.src.zw;
    // Sharp bilinear: keep the middle of each source pixel flat and
    // blend across the edge, over one output pixel. Zooming out (k < 1)
    // has no flat part, so it is plain bilinear.
    let k = post.k.xy;
    let flat_half = max(vec2<f32>(0.5) - vec2<f32>(0.5) / k, vec2<f32>(0.0));
    let from_centre = fract(s) - vec2<f32>(0.5);
    let blend = (from_centre - clamp(from_centre, -flat_half, flat_half)) * k + vec2<f32>(0.5);
    // The blended position, as the corner of the 2×2 taps and their weights.
    let t = floor(s) + blend - vec2<f32>(0.5);
    let corner = floor(t);
    let w = t - corner;
    let at = vec2<i32>(corner);
    let top = mix(tap(at), tap(at + vec2<i32>(1, 0)), w.x);
    let bottom = mix(tap(at + vec2<i32>(0, 1)), tap(at + vec2<i32>(1, 1)), w.x);
    return mix(top, bottom, w.y);
}
