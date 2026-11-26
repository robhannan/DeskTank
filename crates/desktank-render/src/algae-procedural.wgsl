// Algae looks for the one-pass grid (see tank.wgsl `ALGAE_FILM`): the colours
// from the view uniform, the spot and slime patterns worked out per texel.
// algae.rs bakes the same patterns for the split grid (algae-baked.wgsl).

fn look_dark(id: u32) -> vec4<f32> {
    return view.looks[(id & 15u) * 2u];
}

fn look_light(id: u32) -> vec4<f32> {
    return view.looks[(id & 15u) * 2u + 1u];
}

// Smooth value noise: hashes at whole numbers (wrapping every `period`),
// blended between.
fn tile_noise(q: vec2<f32>, period: vec2<i32>, salt: vec2<i32>) -> f32 {
    let i = vec2<i32>(floor(q));
    let f = q - floor(q);
    let u = f * f * (3.0 - 2.0 * f);
    let a = look_wrap(i, period) + salt;
    let b = look_wrap(i + vec2<i32>(1), period) + salt;
    return mix(
        mix(unit_hash(a), unit_hash(vec2<i32>(b.x, a.y)), u.x),
        mix(unit_hash(vec2<i32>(a.x, b.y)), unit_hash(b), u.x),
        u.y,
    );
}

// Both patterns round to the bytes algae.rs bakes, so the two grids agree.

// One spot in each square of a lattice 2.5 cells across, with a random size,
// a random centre that keeps it inside its square and a random chance. The
// chance (x; 1 outside the spot), how far towards the dark rim (y) and 1 on
// the highlight at the top left (z). No loop: Chrome 68's SwiftShader
// miscompiled one that looked into the neighbouring squares.
fn spot_texel(p: vec2<i32>) -> vec3<f32> {
    let tpc = texel_density();
    let size = 5 * tpc / 2;
    let q = look_wrap(p, vec2<i32>(LOOK_TILE));
    let home = q / size;
    let h = hash(home + vec2<i32>(31, 7));
    let radius = f32(tpc) * (0.4 + 0.25 * f32((h >> 24u) & 3u) / 3.0) + 0.3;
    let span = max(f32(size) - 2.0 * radius, 0.0);
    let pick = vec2<f32>(f32((h >> 8u) & 255u), f32((h >> 16u) & 255u)) / 255.0;
    let off = vec2<f32>(q) + 0.5 - (vec2<f32>(home * size) + radius + pick * span);
    let d2 = dot(off, off);
    let into = d2 / (radius * radius);
    if into > 1.0 {
        return vec3<f32>(1.0, 0.0, 0.0);
    }
    let highlight = off.x < -0.5 && off.y < -0.5 && d2 > 0.5 && into < 0.75;
    let shade = round(smoothstep(0.35, 0.95, into) * 127.0) / 127.0;
    return vec3<f32>(f32(h & 255u) / 255.0, shade, select(0.0, 1.0, highlight));
}

// Broad patches six cells across and streaks six cells tall, with a little
// grain: the coverage threshold (x) and how much darker than the density
// alone makes it (y, within ±0.61).
fn slime_texel(p: vec2<i32>) -> vec2<f32> {
    let tpc = texel_density();
    let q = look_wrap(p, vec2<i32>(LOOK_TILE));
    let cells = vec2<f32>(q) / f32(tpc);
    let broad = tile_noise(cells / 6.0, vec2<i32>(LOOK_TILE / (6 * tpc)), vec2<i32>(3, 8));
    let streak = tile_noise(
        vec2<f32>(cells.x, cells.y / 6.0),
        vec2<i32>(LOOK_TILE / tpc, LOOK_TILE / (6 * tpc)),
        vec2<i32>(9, 1),
    );
    let grain = unit_hash(q);
    let threshold = broad * 0.45 + streak * 0.4 + grain * 0.15;
    let dark = (broad - 0.5) * 0.6 + (streak - 0.5) * 0.5 - (grain - 0.5) * 0.12;
    return vec2<f32>(round(threshold * 255.0) / 255.0, (round((dark / 1.22 + 0.5) * 255.0) / 255.0 - 0.5) * 1.22);
}
