//! The split grid's algae look atlas (`algae-baked.wgsl`). Chrome 68's
//! `SwiftShader` has no registers to spare for the spot and slime patterns
//! that the one-pass grid works out per texel (`algae-procedural.wgsl`), nor
//! memory for the dynamically indexed looks in the view uniform, so the
//! split grid reads both from this texture. The patterns here follow the
//! procedural shader step for step.

use desktank_sim::AlgaeLook;

/// Texels each pattern takes before it repeats (`LOOK_TILE` in `tank.wgsl`).
pub const TILE: u32 = 240;
/// The atlas: a tile of patterns at two texels per cell, one at four, then
/// two columns of colours, one row per variety id.
pub const SIZE: (u32, u32) = (2 * TILE + 2, TILE);
/// Variety ids the colour columns hold.
pub const VARIETIES: usize = 16;

/// The pattern tiles, `2 * TILE` wide and `TILE` tall in RGBA bytes: each
/// texel's spot chance (r) and rim shade and highlight (g), and slime
/// threshold (b) and darkening (a).
pub fn bake_patterns() -> Vec<u8> {
    let mut image = Vec::with_capacity((2 * TILE * TILE * 4) as usize);
    for y in 0..TILE {
        for tpc in [2, 4] {
            for x in 0..TILE {
                let [chance, shade] = spot(x, y, tpc);
                let [threshold, dark] = slime(x, y, tpc);
                image.extend([chance, shade, threshold, dark]);
            }
        }
    }
    image
}

/// The colour columns, two texels by `VARIETIES` rows: the dark shade with
/// the style id, then the light shade with the strength.
pub fn colours<'a>(looks: impl Iterator<Item = &'a AlgaeLook>) -> Vec<u8> {
    let mut image = vec![0; VARIETIES * 2 * 4];
    for look in looks {
        let at = usize::from(look.id) * 8;
        let Some(texels) = image.get_mut(at..at + 8) else {
            continue;
        };
        let [dr, dg, db] = look.dark.map(unorm);
        let [lr, lg, lb] = look.light.map(unorm);
        texels.copy_from_slice(&[
            dr,
            dg,
            db,
            look.style as u8,
            lr,
            lg,
            lb,
            unorm(look.strength),
        ]);
    }
    image
}

fn unorm(v: f32) -> u8 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to 0..=255
    let byte = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    byte
}

/// A small whole number as `f32` (all values here are under 2^24).
fn float(v: u32) -> f32 {
    #[allow(clippy::cast_precision_loss)]
    let f = v as f32;
    f
}

/// `tank.wgsl` `hash` for non-negative coordinates.
fn hash(x: u32, y: u32) -> u32 {
    let h = x.wrapping_mul(0x8da6_b343) ^ y.wrapping_mul(0xd816_3841);
    let h = (h ^ (h >> 13)).wrapping_mul(0x5bd1_e995);
    h ^ (h >> 15)
}

fn unit_hash(x: u32, y: u32) -> f32 {
    float(hash(x, y) & 1023) / 1023.0
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `spot_texel`: the chance byte (255 outside a spot) and the rim shade in
/// the low 7 bits, with the top bit on the highlight.
fn spot(x: u32, y: u32, tpc: u32) -> [u8; 2] {
    let size = 5 * tpc / 2;
    let (hx, hy) = (x / size, y / size);
    let h = hash(hx + 31, hy + 7);
    let radius = float(tpc) * (0.4 + 0.25 * float((h >> 24) & 3) / 3.0) + 0.3;
    let span = (float(size) - 2.0 * radius).max(0.0);
    let pick = |shift: u32| float((h >> shift) & 255) / 255.0;
    let off_x = float(x) + 0.5 - (float(hx * size) + radius + pick(8) * span);
    let off_y = float(y) + 0.5 - (float(hy * size) + radius + pick(16) * span);
    let d2 = off_x * off_x + off_y * off_y;
    let into = d2 / (radius * radius);
    if into > 1.0 {
        return [255, 0];
    }
    let highlight = off_x < -0.5 && off_y < -0.5 && d2 > 0.5 && into < 0.75;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0..=127
    let shade = (smoothstep(0.35, 0.95, into) * 127.0).round() as u8;
    [h.to_le_bytes()[0], shade | if highlight { 128 } else { 0 }]
}

/// `tile_noise`: value noise whose lattice wraps every `period`.
fn tile_noise(q: [f32; 2], period: [u32; 2], salt: [u32; 2]) -> f32 {
    let [fx, fy] = q.map(f32::floor);
    let u = |f: f32| f * f * (3.0 - 2.0 * f);
    let (ux, uy) = (u(q[0] - fx), u(q[1] - fy));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // q >= 0
    let (ix, iy) = (fx as u32, fy as u32);
    let (ax, ay) = (ix % period[0] + salt[0], iy % period[1] + salt[1]);
    let (bx, by) = (
        (ix + 1) % period[0] + salt[0],
        (iy + 1) % period[1] + salt[1],
    );
    let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;
    mix(
        mix(unit_hash(ax, ay), unit_hash(bx, ay), ux),
        mix(unit_hash(ax, by), unit_hash(bx, by), ux),
        uy,
    )
}

/// `slime_texel`: the threshold and the darkening, offset into 0..1.
fn slime(x: u32, y: u32, tpc: u32) -> [u8; 2] {
    let cells = [float(x) / float(tpc), float(y) / float(tpc)];
    let broad = tile_noise(
        [cells[0] / 6.0, cells[1] / 6.0],
        [TILE / (6 * tpc); 2],
        [3, 8],
    );
    let streak = tile_noise(
        [cells[0], cells[1] / 6.0],
        [TILE / tpc, TILE / (6 * tpc)],
        [9, 1],
    );
    let grain = unit_hash(x, y);
    let threshold = broad * 0.45 + streak * 0.4 + grain * 0.15;
    let dark = (broad - 0.5) * 0.6 + (streak - 0.5) * 0.5 - (grain - 0.5) * 0.12;
    [unorm(threshold), unorm(dark / 1.22 + 0.5)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shader_knows_the_tile() {
        let line = format!("const LOOK_TILE: i32 = {TILE};");
        assert!(include_str!("tank.wgsl").contains(&line), "{line}");
    }

    #[test]
    fn the_lattices_divide_the_tile() {
        for tpc in [2, 4] {
            assert_eq!(TILE % (5 * tpc / 2), 0, "spots");
            assert_eq!(TILE % (6 * tpc), 0, "slime");
        }
    }

    #[test]
    fn spots_cover_part_of_each_tile_and_slime_spans_its_range() {
        let image = bake_patterns();
        assert_eq!(image.len(), (2 * TILE * TILE * 4) as usize);
        let tile = TILE as usize;
        for half in 0..2 {
            let texels = || {
                image
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .enumerate()
                    .filter(move |(i, _)| (i / tile) % 2 == half)
                    .map(|(_, t)| t)
            };
            let inside = texels().filter(|t| t[0] < 255 || t[1] != 0).count();
            let all = tile * tile;
            assert!(inside > all / 10 && inside < all / 2, "{inside} of {all}");
            assert!(texels().any(|t| t[1] & 128 != 0), "highlights");
            let (lo, hi) = texels().fold((255, 0), |(lo, hi), t| (t[2].min(lo), t[2].max(hi)));
            assert!(lo < 80 && hi > 170, "slime threshold {lo}..{hi}");
        }
    }

    #[test]
    fn colours_sit_in_the_variety_row() {
        let look = AlgaeLook {
            id: 3,
            style: desktank_sim::AlgaeStyle::Spots,
            dark: [0.0; 3],
            light: [1.0; 3],
            strength: 0.95,
        };
        let image = colours([look].iter());
        assert_eq!(image.len(), VARIETIES * 8);
        assert_eq!(&image[24..32], &[0, 0, 0, 2, 255, 255, 255, 242]);
        assert!(image[..24].iter().chain(&image[32..]).all(|&b| b == 0));
    }
}
