// Algae looks for the split grid (see tank.wgsl `ALGAE_FILM`), from the atlas
// algae.rs bakes: the spot and slime patterns of algae-procedural.wgsl for
// two and four texels per cell side by side, then the dark and light colours
// in two columns by variety id.

fn look_dark(id: u32) -> vec4<f32> {
    let dark = textureLoad(look_atlas, vec2<i32>(2 * LOOK_TILE, i32(id & 15u)), 0);
    return vec4<f32>(dark.rgb, dark.a * 255.0);
}

fn look_light(id: u32) -> vec4<f32> {
    return textureLoad(look_atlas, vec2<i32>(2 * LOOK_TILE + 1, i32(id & 15u)), 0);
}

fn look_texel(p: vec2<i32>) -> vec4<f32> {
    let fine = select(0, LOOK_TILE, texel_density() == 4);
    return textureLoad(look_atlas, look_wrap(p, vec2<i32>(LOOK_TILE)) + vec2<i32>(fine, 0), 0);
}

// The chance (r) and the rim shade (low 7 bits of g) and highlight (top bit).
fn spot_texel(p: vec2<i32>) -> vec3<f32> {
    let t = look_texel(p);
    let shade = u32(round(t.g * 255.0));
    return vec3<f32>(t.r, f32(shade & 127u) / 127.0, f32(shade >> 7u));
}

// The threshold (b) and the darkening, offset and scaled into 0..1 (a).
fn slime_texel(p: vec2<i32>) -> vec2<f32> {
    let t = look_texel(p);
    return vec2<f32>(t.b, (t.a - 0.5) * 1.22);
}
