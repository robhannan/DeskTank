//! The palette: a 256×256 RGBA table indexed by (material, variant) that the
//! fragment shader reads with `textureLoad`.

use desktank_sim::plants::POSITION_BITS;
use desktank_sim::{GLUED_VARIANT_BIT, MaterialTable, MovementClass, Part};

/// Table side: one column per material ID, one row per variant byte.
pub const SIDE: usize = 256;

/// Brightness of each shade relative to the material colour, in 1/256. A
/// cell picks one from its variant byte: mostly the base, sometimes a darker
/// or lighter speck.
const SHADES: [u32; 4] = [256, 240, 274, 225];

/// Colour of unknown material IDs: loud magenta, so a gap is obvious.
const UNKNOWN: [u8; 3] = [0xff, 0x00, 0xff];

/// Glued rigid cells (odd variants, D17) mix this much of [`GLUE_TINT`]
/// into their shade, in 1/256: a subtle amber cast.
const GLUE_MIX: u32 = 44;
/// The colour glued cells lean towards, sRGB.
const GLUE_TINT: [u8; 3] = [0xd9, 0xa4, 0x41];

/// Liquids at least this dense darken with depth (water, not air).
const DEPTH_TINT_DENSITY: f32 = 0.5;

/// Palette alpha marking a plant part (T6b): the shader tints these cells
/// by their plant's texel. 255 marks depth-tinted liquids, 0 the rest.
pub const PLANT_ALPHA: u8 = 128;

/// Which shade a plant cell's variant byte picks: its top two bits are
/// colour noise (the low bits are its position in its part,
/// `desktank_sim::plants`), mostly the base shade.
#[must_use]
pub fn plant_shade(variant: u8) -> usize {
    [0, 0, 1, 2][usize::from(variant >> POSITION_BITS)]
}

/// Which shade a variant byte picks. Every range starts on an even byte,
/// so a rigid cell's glued bit ([`GLUED_VARIANT_BIT`]) never changes it.
#[must_use]
pub fn shade(variant: u8) -> usize {
    match variant {
        0..=119 => 0,
        120..=191 => 1,
        192..=239 => 2,
        _ => 3,
    }
}

/// Builds the RGBA table, row-major with rows = variants. Alpha is 255 for
/// materials that darken with depth, [`PLANT_ALPHA`] for plant parts and 0
/// otherwise (the shader's flags; output is opaque). Rigid materials shade
/// odd variants (glued bodies) with an amber tint; plant parts take their
/// shade from the variant's noise bits.
#[must_use]
pub fn build(materials: &MaterialTable) -> Vec<u8> {
    let mut columns = [[UNKNOWN; 4]; SIDE];
    let mut tinted = [false; SIDE];
    let mut rigid = [false; SIDE];
    let mut plant = [false; SIDE];
    for p in Part::ALL {
        if let Some(id) = materials.id(p.material_name()) {
            plant[usize::from(id.0)] = true;
        }
    }
    for m in materials.iter() {
        let i = usize::from(m.id.0);
        columns[i] = SHADES.map(|k| m.colour.map(|c| scale(c, k)));
        tinted[i] = m.class == MovementClass::Liquid && m.density >= DEPTH_TINT_DENSITY;
        rigid[i] = m.class == MovementClass::Rigid;
    }
    let mut out = vec![0; SIDE * SIDE * 4];
    for (variant, row) in out.as_chunks_mut::<{ SIDE * 4 }>().0.iter_mut().enumerate() {
        #[allow(clippy::cast_possible_truncation)] // variant < 256
        let s = shade(variant as u8);
        #[allow(clippy::cast_possible_truncation)] // variant < 256
        let glued = variant as u8 & GLUED_VARIANT_BIT != 0;
        #[allow(clippy::cast_possible_truncation)] // variant < 256
        let ps = plant_shade(variant as u8);
        for (material, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let mut rgb = columns[material][if plant[material] { ps } else { s }];
            if glued && rigid[material] {
                rgb = glue(rgb);
            }
            let [r, g, b] = rgb;
            let flag = if plant[material] {
                PLANT_ALPHA
            } else if tinted[material] {
                255
            } else {
                0
            };
            px.copy_from_slice(&[r, g, b, flag]);
        }
    }
    out
}

fn glue(rgb: [u8; 3]) -> [u8; 3] {
    let mut out = rgb;
    for (o, (c, t)) in out.iter_mut().zip(rgb.into_iter().zip(GLUE_TINT)) {
        let v = (u32::from(c) * (256 - GLUE_MIX) + u32::from(t) * GLUE_MIX) / 256;
        *o = u8::try_from(v).unwrap_or(u8::MAX);
    }
    out
}

fn scale(c: u8, k: u32) -> u8 {
    u8::try_from(u32::from(c) * k / 256).unwrap_or(u8::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texel(table: &[u8], material: u8, variant: u8) -> [u8; 4] {
        let i = (usize::from(variant) * SIDE + usize::from(material)) * 4;
        table[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn base_shade_is_the_material_colour() {
        let mats = MaterialTable::builtin().unwrap();
        let table = build(&mats);
        let sand = mats.get(mats.id("sand").unwrap()).unwrap();
        let [r, g, b, a] = texel(&table, sand.id.0, 0);
        assert_eq!([r, g, b], sand.colour);
        assert_eq!(a, 0);
    }

    #[test]
    fn variants_pick_lighter_and_darker_specks() {
        let mats = MaterialTable::builtin().unwrap();
        let table = build(&mats);
        let id = mats.id("gravel").unwrap().0;
        let base = texel(&table, id, 0)[0];
        assert!(texel(&table, id, 150)[0] < base);
        assert!(texel(&table, id, 200)[0] > base);
        assert!(texel(&table, id, 250)[0] < base);
    }

    #[test]
    fn glued_rigid_cells_get_a_tint_and_keep_their_shade() {
        let mats = MaterialTable::builtin().unwrap();
        let table = build(&mats);
        let rock = mats.id("rock").unwrap().0;
        let sand = mats.id("sand").unwrap().0;
        for v in (0..=254u8).step_by(2) {
            assert_eq!(shade(v), shade(v | GLUED_VARIANT_BIT));
            let (free, glued) = (texel(&table, rock, v), texel(&table, rock, v + 1));
            assert_ne!(free, glued);
            // Subtle: within 40 levels per channel, and warmer.
            assert!(free.iter().zip(glued).all(|(a, b)| a.abs_diff(b) <= 40));
            assert!(glued[0] > free[0] && glued[2] <= free[2]);
            // Other classes ignore the bit.
            assert_eq!(texel(&table, sand, v), texel(&table, sand, v + 1));
        }
    }

    #[test]
    fn only_dense_liquids_are_depth_tinted() {
        let mats = MaterialTable::builtin().unwrap();
        let table = build(&mats);
        for m in mats.iter() {
            let tinted = texel(&table, m.id.0, 0)[3] == 255;
            let dense = matches!(m.name.as_str(), "water" | "liquid_fertiliser");
            assert_eq!(tinted, dense, "{}", m.name);
        }
    }

    #[test]
    fn plant_parts_are_flagged_and_shaded_by_their_noise_bits() {
        let mats = MaterialTable::builtin().unwrap();
        let table = build(&mats);
        let leaf = mats.get(mats.id("leaf").unwrap()).unwrap();
        // Any position, noise 0: the base colour.
        for pos in [0u8, 1, 17, 63] {
            let [r, g, b, a] = texel(&table, leaf.id.0, pos);
            assert_eq!(([r, g, b], a), (leaf.colour, PLANT_ALPHA));
        }
        assert_ne!(
            texel(&table, leaf.id.0, 0b1000_0001),
            texel(&table, leaf.id.0, 1)
        );
        assert_eq!(texel(&table, mats.id("sand").unwrap().0, 0)[3], 0);
    }

    #[test]
    fn unknown_ids_are_magenta() {
        let mats = MaterialTable::builtin().unwrap();
        let table = build(&mats);
        assert_eq!(texel(&table, 255, 0), [0xff, 0, 0xff, 0]);
    }
}
